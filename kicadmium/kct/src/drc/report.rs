//! DRC report parsing for KiCad text and JSON formats (port of
//! `kicad_tools.drc.report`).

use std::path::Path;
use std::sync::LazyLock;

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;

use super::severity::Severity;
use super::violation::{DRCViolation, Location, Suggestion, ViolationType};
use crate::feedback::generate_drc_suggestions;
use crate::jobj;
use crate::pyjson::{self, py_splitlines, Json};
use crate::validate::filters::{FilterEngine, ViolationFilter};

/// Source-qualified mask-to-copper assessment carried by kct-check JSON
/// (`validate.mask_copper.MaskCopperAssessment`); kept as its JSON payload.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskCopperAssessment {
    pub coverage: String,
    pub reasons: Vec<String>,
    pub policy: Json,
    pub binding: Json,
    pub measurements: Vec<Json>,
    pub intent_audit: Vec<Json>,
    pub geometry_provenance: Json,
    pub evaluated_pairs: Json,
}

impl MaskCopperAssessment {
    pub const SCHEMA: &'static str = "kct.mask-copper-assessment.v1";

    pub fn passed(&self) -> bool {
        self.reasons.is_empty()
            && !self.policy.is_null()
            && !self.binding.is_null()
            && self.coverage == "complete"
            && !self.measurements.iter().any(|m| {
                matches!(
                    m.get("disposition").and_then(Json::as_str),
                    Some("violation" | "uncertain")
                )
            })
    }

    pub fn from_json(data: &Json) -> Result<Self> {
        if data.get("schema").and_then(Json::as_str) != Some(Self::SCHEMA) {
            bail!("Unknown mask assessment schema");
        }
        let field = |k: &str| data.get(k).cloned().ok_or_else(|| anyhow!("'{k}'"));
        let coverage = field("coverage")?
            .as_str()
            .ok_or_else(|| anyhow!("Unknown mask-to-copper coverage status"))?
            .to_string();
        if !matches!(coverage.as_str(), "complete" | "incomplete" | "not_run") {
            bail!("Unknown mask-to-copper coverage status");
        }
        let list = |k: &str| -> Result<Vec<Json>> {
            Ok(field(k)?
                .as_array()
                .map(<[Json]>::to_vec)
                .unwrap_or_default())
        };
        let policy = field("policy")?;
        let binding = field("binding")?;
        Ok(MaskCopperAssessment {
            coverage,
            reasons: list("reasons")?
                .iter()
                .map(|r| r.as_str().map_or_else(|| r.py_repr(), str::to_string))
                .collect(),
            policy: if policy.truthy() { policy } else { Json::Null },
            binding: if binding.truthy() {
                binding
            } else {
                Json::Null
            },
            measurements: list("measurements")?,
            intent_audit: list("intent_audit")?,
            geometry_provenance: field("geometry_provenance")?,
            evaluated_pairs: field("evaluated_pairs")?,
        })
    }

    pub fn to_json(&self) -> Json {
        jobj! {
            "schema" => Self::SCHEMA,
            "coverage" => self.coverage.as_str(),
            "passed" => self.passed(),
            "reasons" => &self.reasons,
            "policy" => self.policy.clone(),
            "binding" => self.binding.clone(),
            "measurements" => Json::Arr(self.measurements.clone()),
            "intent_audit" => Json::Arr(self.intent_audit.clone()),
            "geometry_provenance" => self.geometry_provenance.clone(),
            "evaluated_pairs" => self.evaluated_pairs.clone(),
        }
    }
}

/// Parsed DRC report from KiCad.
///
/// `violations` holds geometric findings only; KiCad JSON's sibling
/// `unconnected_items` array lands in [`DRCReport::unconnected_items`]
/// (issue #4498).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DRCReport {
    pub source_file: String,
    /// `datetime.isoformat()` of the report timestamp, when parseable.
    pub created_at: Option<String>,
    pub pcb_name: String,
    pub violations: Vec<DRCViolation>,
    pub footprint_errors: i64,
    pub unconnected_items: Vec<DRCViolation>,
    /// KiCad JSON `schematic_parity`; `None` when the report carries no
    /// such array (parity was not requested).
    pub schematic_parity: Option<Vec<DRCViolation>>,
    pub mask_copper_assessments: Vec<MaskCopperAssessment>,
}

impl DRCReport {
    pub fn new(
        source_file: impl Into<String>,
        pcb_name: impl Into<String>,
        violations: Vec<DRCViolation>,
    ) -> Self {
        DRCReport {
            source_file: source_file.into(),
            pcb_name: pcb_name.into(),
            violations,
            ..Default::default()
        }
    }

    pub fn passed(&self) -> bool {
        self.error_count() == 0 && self.mask_copper_assessments.iter().all(|a| a.passed())
    }

    pub fn violation_count(&self) -> usize {
        self.violations.len()
    }

    pub fn unconnected_item_count(&self) -> usize {
        self.unconnected_items.len()
    }

    /// Connectivity entries from both the JSON `unconnected_items` array and
    /// text-format `[unconnected_items]` violations.
    pub fn connectivity_items(&self) -> Vec<&DRCViolation> {
        self.unconnected_items
            .iter()
            .chain(
                self.violations
                    .iter()
                    .filter(|v| v.vtype == ViolationType::UNCONNECTED_ITEMS),
            )
            .collect()
    }

    pub fn error_count(&self) -> usize {
        self.violations.iter().filter(|v| v.is_error()).count()
    }

    pub fn warning_count(&self) -> usize {
        self.violations.iter().filter(|v| !v.is_error()).count()
    }

    pub fn errors(&self) -> Vec<&DRCViolation> {
        self.violations.iter().filter(|v| v.is_error()).collect()
    }

    pub fn warnings(&self) -> Vec<&DRCViolation> {
        self.violations.iter().filter(|v| !v.is_error()).collect()
    }

    pub fn by_type(&self, vtype: ViolationType) -> Vec<&DRCViolation> {
        self.violations
            .iter()
            .filter(|v| v.vtype == vtype)
            .collect()
    }

    pub fn by_net(&self, net: &str) -> Vec<&DRCViolation> {
        self.violations
            .iter()
            .filter(|v| v.nets.iter().any(|n| n == net))
            .collect()
    }

    /// Group violations by type, in first-seen order.
    pub fn violations_by_type(&self) -> Vec<(ViolationType, Vec<&DRCViolation>)> {
        let mut out: Vec<(ViolationType, Vec<&DRCViolation>)> = Vec::new();
        for v in &self.violations {
            match out.iter_mut().find(|(t, _)| *t == v.vtype) {
                Some((_, list)) => list.push(v),
                None => out.push((v.vtype, vec![v])),
            }
        }
        out
    }

    pub fn violations_near(&self, x_mm: f64, y_mm: f64, radius_mm: f64) -> Vec<&DRCViolation> {
        self.violations
            .iter()
            .filter(|v| {
                v.locations.iter().any(|l| {
                    let (dx, dy) = (l.x_mm - x_mm, l.y_mm - y_mm);
                    dx * dx + dy * dy <= radius_mm * radius_mm
                })
            })
            .collect()
    }

    /// New report with `filters` applied (ignored violations dropped,
    /// reclassified ones copied with their new severity).
    pub fn apply_filters(&self, filters: &[ViolationFilter]) -> DRCReport {
        let result = FilterEngine::new(filters.to_vec()).apply(&self.violations);
        DRCReport {
            violations: result.kept,
            ..self.clone()
        }
    }

    pub fn summary(&self) -> Json {
        let mut by_type = self.violations_by_type();
        by_type.sort_by_key(|(_, list)| std::cmp::Reverse(list.len()));
        jobj! {
            "pcb_name" => self.pcb_name.as_str(),
            "total_violations" => self.violation_count(),
            "errors" => self.error_count(),
            "warnings" => self.warning_count(),
            "footprint_errors" => self.footprint_errors,
            "by_type" => Json::Obj(by_type.iter().map(|(t, l)| (t.value().to_string(), Json::from(l.len()))).collect()),
        }
    }

    pub fn to_json(&self) -> Json {
        jobj! {
            "source_file" => self.source_file.as_str(),
            "created_at" => self.created_at.clone(),
            "pcb_name" => self.pcb_name.as_str(),
            "violation_count" => self.violation_count(),
            "error_count" => self.error_count(),
            "warning_count" => self.warning_count(),
            "footprint_errors" => self.footprint_errors,
            "violations" => Json::Arr(self.violations.iter().map(DRCViolation::to_json).collect()),
            "passed" => self.passed(),
            "mask_copper_assessments" => Json::Arr(self.mask_copper_assessments.iter().map(MaskCopperAssessment::to_json).collect()),
        }
    }

    /// Load a report, auto-detecting JSON vs text format.
    pub fn load(path: impl AsRef<Path>) -> Result<DRCReport> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("No such file or directory: '{}'", path.display()))?;
        let source = path.to_string_lossy();
        if content.trim().starts_with('{') {
            parse_json_report(&content, &source)
        } else {
            parse_text_report(&content, &source)
        }
    }
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("regex"));
    };
}

re!(HEADER_PCB, r"\*\* Drc report for (.+?) \*\*");
re!(HEADER_DATE, r"\*\* Created on (.+?) \*\*");
re!(TZ_COMPACT, r"^.*[+-]\d{4}$");
re!(FP_ERRORS, r"\*\* Found (\d+) Footprint errors \*\*");
re!(VIOLATION_START, r"^\[(\w+)\]:\s*(.+)");
re!(RULE_LINE, r"(?i)^\s+Rule:\s*(.+);\s*(error|warning)");
re!(
    LOC_LINE,
    r"^\s+@\s*\(\s*([\d.]+)\s*mm\s*,\s*([\d.]+)\s*mm\s*\):\s*(.+)"
);
re!(LAYER_IN_DESC, r"on\s+(F\.Cu|B\.Cu|[\w.]+)");
re!(NET_IN_DESC, r"\[([^\]]+)\]");
re!(
    CLEARANCE_VALUES,
    r"(?i)clearance\s+([\d.]+)\s*mm.*?actual\s+(-?[\d.]+)\s*mm"
);
re!(
    MINIMUM_VALUES,
    r"(?i)minimum\s+([\d.]+)\s*mm.*?actual\s+(-?[\d.]+)\s*mm"
);
re!(
    WIDTH_VALUES,
    r"(?i)width\s+([\d.]+)\s*mm.*?actual\s+(-?[\d.]+)\s*mm"
);
re!(
    MIN_VALUES,
    r"(?i)\bmin(?:imum)?\s+([\d.]+)\s*mm.*?actual\s+(-?[\d.]+)\s*mm"
);

fn py_float(s: &str) -> Result<f64> {
    s.parse::<f64>()
        .map_err(|_| anyhow!("could not convert string to float: '{s}'"))
}

/// Python `datetime.fromisoformat(s).isoformat()`; `None` if unparseable.
pub fn parse_isoformat(s: &str) -> Option<String> {
    static ISO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2})(?::?(\d{2})(?::?(\d{2})(?:[.,](\d{1,6}))?)?)?(Z|[+-]\d{2}(?::?\d{2}(?::?\d{2})?)?)?)?$",
        )
        .expect("regex")
    });
    let c = ISO.captures(s)?;
    let num = |i: usize| c.get(i).map_or(Some(0), |m| m.as_str().parse::<u32>().ok());
    let (year, month, day) = (num(1)?, num(2)?, num(3)?);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let mdays = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&month) || day == 0 || day > mdays[month as usize - 1] || year == 0 {
        return None;
    }
    let (h, mi, sec) = (num(4)?, num(5)?, num(6)?);
    if h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let mut out = format!("{year:04}-{month:02}-{day:02}T{h:02}:{mi:02}:{sec:02}");
    if let Some(frac) = c.get(7) {
        let micros: u32 = format!("{:0<6}", frac.as_str()).parse().ok()?;
        if micros != 0 {
            out.push_str(&format!(".{micros:06}"));
        }
    }
    if let Some(tz) = c.get(8) {
        let tz = tz.as_str();
        if tz == "Z" {
            out.push_str("+00:00");
        } else {
            let sign = &tz[..1];
            let digits: String = tz[1..].chars().filter(char::is_ascii_digit).collect();
            let th: u32 = digits[..2].parse().ok()?;
            let tm: u32 = digits.get(2..4).map_or(Some(0), |d| d.parse().ok())?;
            let ts: u32 = digits.get(4..6).map_or(Some(0), |d| d.parse().ok())?;
            if th > 23 || tm > 59 || ts > 59 {
                return None;
            }
            out.push_str(&format!("{sign}{th:02}:{tm:02}"));
            if ts != 0 {
                out.push_str(&format!(":{ts:02}"));
            }
        }
    }
    Some(out)
}

/// Extract required/actual values from a violation message (first matching
/// wording wins; leaves the fields untouched otherwise).
pub fn extract_values(v: &mut DRCViolation, message: &str) -> Result<()> {
    for re in [
        &*CLEARANCE_VALUES,
        &*MINIMUM_VALUES,
        &*WIDTH_VALUES,
        &*MIN_VALUES,
    ] {
        if let Some(c) = re.captures(message) {
            v.required_value_mm = Some(py_float(&c[1])?);
            v.actual_value_mm = Some(py_float(&c[2])?);
            return Ok(());
        }
    }
    Ok(())
}

/// Refine generic CLEARANCE to CLEARANCE_SEGMENT_VIA when one item is a
/// Track and another a Via.
pub fn infer_segment_via_type(vtype: ViolationType, items: &[String]) -> ViolationType {
    if vtype != ViolationType::CLEARANCE {
        return vtype;
    }
    let (mut has_track, mut has_via) = (false, false);
    for item in items {
        let lower = item.to_lowercase();
        if lower.starts_with("track ") || lower.starts_with("track\t") {
            has_track = true;
        } else if lower.starts_with("via ") || lower.starts_with("via\t") {
            has_via = true;
        }
    }
    if has_track && has_via {
        ViolationType::CLEARANCE_SEGMENT_VIA
    } else {
        vtype
    }
}

fn push_nets_from_desc(nets: &mut Vec<String>, desc: &str) {
    for c in NET_IN_DESC.captures_iter(desc) {
        let net = &c[1];
        if net != "<no net>" && !nets.iter().any(|n| n == net) {
            nets.push(net.to_string());
        }
    }
}

fn attach_suggestions(v: &mut DRCViolation) {
    v.suggestions = generate_drc_suggestions(v)
        .into_iter()
        .map(Suggestion::Text)
        .collect();
}

/// Parse KiCad text-format DRC report (`.rpt`).
pub fn parse_text_report(content: &str, source_file: &str) -> Result<DRCReport> {
    let mut report = DRCReport {
        source_file: source_file.to_string(),
        ..Default::default()
    };
    if let Some(c) = HEADER_PCB.captures(content) {
        report.pcb_name = c[1].to_string();
    }
    if let Some(c) = HEADER_DATE.captures(content) {
        let mut date = c[1].trim().to_string();
        if TZ_COMPACT.is_match(&date) {
            let n = date.len();
            date = format!("{}:{}", &date[..n - 2], &date[n - 2..]);
        }
        report.created_at = parse_isoformat(&date);
    }
    if let Some(c) = FP_ERRORS.captures(content) {
        report.footprint_errors = c[1].parse().unwrap_or(0);
    }

    let mut current: Option<DRCViolation> = None;
    let finish = |v: DRCViolation, out: &mut Vec<DRCViolation>| {
        let mut v = v;
        v.vtype = infer_segment_via_type(ViolationType::from_string(&v.type_str), &v.items);
        attach_suggestions(&mut v);
        out.push(v);
    };
    for line in py_splitlines(content) {
        if line.starts_with("**") {
            continue;
        }
        if let Some(c) = VIOLATION_START.captures(line) {
            if let Some(v) = current.take() {
                finish(v, &mut report.violations);
            }
            let mut v = DRCViolation::new(ViolationType::UNKNOWN, &c[1], Severity::Error, &c[2]);
            let message = v.message.clone();
            extract_values(&mut v, &message)?;
            current = Some(v);
            continue;
        }
        let Some(v) = current.as_mut() else {
            continue;
        };
        if let Some(c) = RULE_LINE.captures(line) {
            v.rule = c[1].to_string();
            v.severity = Severity::from_string(&c[2]);
            continue;
        }
        if let Some(c) = LOC_LINE.captures(line) {
            let x = py_float(&c[1])?;
            let y = py_float(&c[2])?;
            let desc = c[3].to_string();
            let layer = LAYER_IN_DESC
                .captures(&desc)
                .map(|m| m[1].to_string())
                .unwrap_or_default();
            v.locations.push(Location::new(x, y, layer));
            push_nets_from_desc(&mut v.nets, &desc);
            v.items.push(desc);
        }
    }
    if let Some(v) = current.take() {
        finish(v, &mut report.violations);
    }
    Ok(report)
}

/// Parse a JSON DRC report: kicad-cli format, or kct-check format (detected
/// by a top-level `summary` or `manufacturer` key).
pub fn parse_json_report(content: &str, source_file: &str) -> Result<DRCReport> {
    let data = pyjson::loads(content)?;
    if !matches!(data, Json::Obj(_)) {
        bail!("'{}' object has no attribute 'get'", data.py_type_name());
    }
    if data.contains_key("summary") || data.contains_key("manufacturer") {
        parse_kct_check_json(&data, source_file)
    } else {
        parse_kicad_cli_json(&data, source_file)
    }
}

fn str_or(v: Option<&Json>, default: &str) -> String {
    match v {
        None => default.to_string(),
        Some(Json::Str(s)) => s.clone(),
        Some(other) => other.py_repr(),
    }
}

fn arr(v: Option<&Json>) -> &[Json] {
    v.and_then(Json::as_array).unwrap_or(&[])
}

/// `str(x)` for list entries.
fn py_str(v: &Json) -> String {
    match v {
        Json::Str(s) => s.clone(),
        other => other.py_repr(),
    }
}

fn parse_kct_check_json(data: &Json, source_file: &str) -> Result<DRCReport> {
    let mut report = DRCReport {
        source_file: source_file.to_string(),
        pcb_name: str_or(data.get("file"), ""),
        ..Default::default()
    };
    for item in arr(data.get("violations")) {
        let type_str = str_or(item.get("rule_id"), "unknown");
        let message = str_or(item.get("message"), "");
        let severity = Severity::from_string(&str_or(item.get("severity"), "error"));
        let mut locations = Vec::new();
        if let Some(loc) = item.get("location").and_then(Json::as_array) {
            if loc.len() >= 2 {
                let layer = str_or(item.get("layer"), "");
                locations.push(Location::from_json(Some(&loc[0]), Some(&loc[1]), &layer));
            }
        }
        let items: Vec<String> = arr(item.get("items")).iter().map(py_str).collect();
        let vtype = infer_segment_via_type(ViolationType::from_string(&type_str), &items);
        let mut v = DRCViolation::new(vtype, type_str.clone(), severity, message.clone());
        v.rule = type_str;
        v.locations = locations;
        v.items = items;
        v.nets = arr(item.get("nets")).iter().map(py_str).collect();
        v.required_value_mm = item.get("required_value").and_then(Json::as_f64);
        v.actual_value_mm = item.get("actual_value").and_then(Json::as_f64);
        extract_values(&mut v, &message)?;
        attach_suggestions(&mut v);
        report.violations.push(v);
    }
    for a in arr(data.get("mask_copper_assessments")) {
        report
            .mask_copper_assessments
            .push(MaskCopperAssessment::from_json(a)?);
    }
    Ok(report)
}

fn parse_kicad_cli_json(data: &Json, source_file: &str) -> Result<DRCReport> {
    let mut report = DRCReport {
        source_file: source_file.to_string(),
        pcb_name: str_or(data.get("source"), ""),
        ..Default::default()
    };
    if let Some(date) = data.get("date") {
        report.created_at = date.as_str().and_then(parse_isoformat);
    }
    if data.get("schematic_parity").is_some() {
        report.schematic_parity = Some(Vec::new());
    }
    // 0: geometric violation, 1: unconnected item, 2: schematic parity.
    let entries = arr(data.get("violations"))
        .iter()
        .map(|v| (v, 0))
        .chain(arr(data.get("unconnected_items")).iter().map(|u| (u, 1)))
        .chain(arr(data.get("schematic_parity")).iter().map(|u| (u, 2)));
    for (item, kind) in entries {
        let type_str = str_or(item.get("type"), "unknown");
        let message = str_or(item.get("description"), "");
        let severity = Severity::from_string(&str_or(item.get("severity"), "error"));
        let rule = str_or(item.get("rule"), "");
        let mut locations = Vec::new();
        let mut items = Vec::new();
        let mut nets = Vec::new();
        if let Some(pos) = item.get("pos") {
            locations.push(Location::from_json(pos.get("x"), pos.get("y"), ""));
        }
        for item_data in arr(item.get("items")) {
            let desc = str_or(item_data.get("description"), "");
            if let Some(pos) = item_data.get("pos") {
                locations.push(Location::from_json(pos.get("x"), pos.get("y"), ""));
            }
            push_nets_from_desc(&mut nets, &desc);
            if let Some(net) = item_data.get("net") {
                if net.truthy() {
                    let net = py_str(net);
                    if !nets.contains(&net) {
                        nets.push(net);
                    }
                }
            }
            items.push(desc);
        }
        let vtype = infer_segment_via_type(ViolationType::from_string(&type_str), &items);
        let mut v = DRCViolation::new(vtype, type_str, severity, message.clone());
        v.rule = rule;
        v.locations = locations;
        v.items = items;
        v.nets = nets;
        extract_values(&mut v, &message)?;
        attach_suggestions(&mut v);
        match kind {
            1 => report.unconnected_items.push(v),
            2 => report.schematic_parity.get_or_insert_with(Vec::new).push(v),
            _ => report.violations.push(v),
        }
    }
    report.footprint_errors = data
        .get("footprint_errors")
        .and_then(Json::as_i64)
        .unwrap_or(0);
    Ok(report)
}
