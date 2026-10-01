//! ERC report parsing for KiCad JSON and text formats (port of
//! `kicad_tools.erc.report`).

use std::path::Path;
use std::sync::LazyLock;

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;

use super::violation::{ERCSeverity, ERCViolation, ERCViolationType};
use crate::feedback::generate_erc_suggestions;
use crate::jobj;
use crate::pyjson::{self, py_splitlines, Json};
use crate::validate::filters::{FilterEngine, ViolationFilter};

/// Parsed ERC report from KiCad.
#[derive(Debug, Clone, PartialEq)]
pub struct ERCReport {
    pub source_file: String,
    pub kicad_version: String,
    pub coordinate_units: String,
    pub violations: Vec<ERCViolation>,
}

impl Default for ERCReport {
    fn default() -> Self {
        ERCReport {
            source_file: String::new(),
            kicad_version: String::new(),
            coordinate_units: "mm".into(),
            violations: Vec::new(),
        }
    }
}

impl ERCReport {
    fn active(&self) -> impl Iterator<Item = &ERCViolation> {
        self.violations.iter().filter(|v| !v.excluded)
    }

    /// Violations excluding excluded ones.
    pub fn violation_count(&self) -> usize {
        self.active().count()
    }

    pub fn error_count(&self) -> usize {
        self.active().filter(|v| v.is_error()).count()
    }

    pub fn warning_count(&self) -> usize {
        self.active()
            .filter(|v| v.severity == ERCSeverity::Warning)
            .count()
    }

    pub fn exclusion_count(&self) -> usize {
        self.violations.iter().filter(|v| v.excluded).count()
    }

    pub fn errors(&self) -> Vec<&ERCViolation> {
        self.active().filter(|v| v.is_error()).collect()
    }

    pub fn warnings(&self) -> Vec<&ERCViolation> {
        self.active()
            .filter(|v| v.severity == ERCSeverity::Warning)
            .collect()
    }

    pub fn exclusions(&self) -> Vec<&ERCViolation> {
        self.violations.iter().filter(|v| v.excluded).collect()
    }

    pub fn apply_filters(&self, filters: &[ViolationFilter]) -> ERCReport {
        let result = FilterEngine::new(filters.to_vec()).apply(&self.violations);
        ERCReport {
            violations: result.kept,
            ..self.clone()
        }
    }

    pub fn by_type(&self, vtype: ERCViolationType) -> Vec<&ERCViolation> {
        self.active().filter(|v| v.vtype == vtype).collect()
    }

    pub fn by_sheet(&self, sheet: &str) -> Vec<&ERCViolation> {
        self.active().filter(|v| v.sheet == sheet).collect()
    }

    /// Group non-excluded violations by type, first-seen order.
    pub fn violations_by_type(&self) -> Vec<(ERCViolationType, Vec<&ERCViolation>)> {
        let mut out: Vec<(ERCViolationType, Vec<&ERCViolation>)> = Vec::new();
        for v in self.active() {
            match out.iter_mut().find(|(t, _)| *t == v.vtype) {
                Some((_, list)) => list.push(v),
                None => out.push((v.vtype, vec![v])),
            }
        }
        out
    }

    /// Group non-excluded violations by sheet (`""` -> `"root"`).
    pub fn violations_by_sheet(&self) -> Vec<(String, Vec<&ERCViolation>)> {
        let mut out: Vec<(String, Vec<&ERCViolation>)> = Vec::new();
        for v in self.active() {
            let sheet = if v.sheet.is_empty() { "root" } else { &v.sheet };
            match out.iter_mut().find(|(s, _)| s == sheet) {
                Some((_, list)) => list.push(v),
                None => out.push((sheet.to_string(), vec![v])),
            }
        }
        out
    }

    /// Partial, case-insensitive match on type, description, or type
    /// description.
    pub fn filter_by_type(&self, type_filter: &str) -> Vec<&ERCViolation> {
        let f = type_filter.to_lowercase();
        self.active()
            .filter(|v| {
                v.type_str.to_lowercase().contains(&f)
                    || v.description.to_lowercase().contains(&f)
                    || v.type_description().to_lowercase().contains(&f)
            })
            .collect()
    }

    pub fn summary(&self) -> Json {
        let mut by_type = self.violations_by_type();
        by_type.sort_by_key(|(_, l)| std::cmp::Reverse(l.len()));
        jobj! {
            "source_file" => self.source_file.as_str(),
            "kicad_version" => self.kicad_version.as_str(),
            "total_violations" => self.violation_count(),
            "errors" => self.error_count(),
            "warnings" => self.warning_count(),
            "exclusions" => self.exclusion_count(),
            "by_type" => Json::Obj(by_type.iter().map(|(t, l)| (t.value().to_string(), Json::from(l.len()))).collect()),
        }
    }

    pub fn to_json(&self) -> Json {
        jobj! {
            "source_file" => self.source_file.as_str(),
            "kicad_version" => self.kicad_version.as_str(),
            "coordinate_units" => self.coordinate_units.as_str(),
            "summary" => jobj!{
                "errors" => self.error_count(),
                "warnings" => self.warning_count(),
                "exclusions" => self.exclusion_count(),
            },
            "violations" => Json::Arr(self.violations.iter().map(ERCViolation::to_json).collect()),
        }
    }

    /// Load a report, auto-detecting JSON vs text format.
    pub fn load(path: impl AsRef<Path>) -> Result<ERCReport> {
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

fn coord(v: Option<&Json>) -> (f64, bool) {
    match v {
        None => (0.0, true),
        Some(j) => (j.as_f64().unwrap_or(0.0), !matches!(j, Json::Float(_))),
    }
}

/// Parse `kicad-cli sch erc --format json` output.
pub fn parse_json_report(content: &str, source_file: &str) -> Result<ERCReport> {
    let data = pyjson::loads(content)?;
    if !matches!(data, Json::Obj(_)) {
        bail!("'{}' object has no attribute 'get'", data.py_type_name());
    }
    let mut report = ERCReport {
        source_file: str_or(data.get("source"), source_file),
        kicad_version: str_or(data.get("kicad_version"), ""),
        coordinate_units: str_or(data.get("coordinate_units"), "mm"),
        violations: Vec::new(),
    };
    for sheet in arr(data.get("sheets")) {
        let sheet_path = str_or(sheet.get("path"), "");
        for item in arr(sheet.get("violations")) {
            let type_str = str_or(item.get("type"), "unknown");
            let mut v = ERCViolation::new(
                ERCViolationType::from_string(&type_str),
                type_str,
                ERCSeverity::from_string(&str_or(item.get("severity"), "error")),
                str_or(item.get("description"), ""),
            );
            v.sheet = sheet_path.clone();
            let pos = item.get("pos");
            let (x, xi) = coord(pos.and_then(|p| p.get("x")));
            let (y, yi) = coord(pos.and_then(|p| p.get("y")));
            v.set_pos(x, xi, y, yi);
            v.items = arr(item.get("items"))
                .iter()
                .map(|i| str_or(i.get("description"), ""))
                .collect();
            v.excluded = item.get("excluded").is_some_and(Json::truthy);
            v.suggestions = generate_erc_suggestions(&v);
            report.violations.push(v);
        }
    }
    Ok(report)
}

static SOURCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*\* ERC report for (.+?) \*\*").expect("regex"));
static VIOLATION_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\w+)\]:\s*(.+)").expect("regex"));
static LOC_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+@\s*\(\s*([\d.]+)\s*mm\s*,\s*([\d.]+)\s*mm\s*\):\s*(.+)").expect("regex")
});
static SEVERITY_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s+severity:\s*(\w+)").expect("regex"));
static SHEET_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s+sheet:\s*(.+)").expect("regex"));

/// Parse KiCad ERC text report (`.rpt`).
pub fn parse_text_report(content: &str, source_file: &str) -> Result<ERCReport> {
    let mut report = ERCReport {
        source_file: source_file.to_string(),
        ..Default::default()
    };
    if let Some(c) = SOURCE.captures(content) {
        report.source_file = c[1].to_string();
    }
    let finish = |mut v: ERCViolation, out: &mut Vec<ERCViolation>| {
        v.suggestions = generate_erc_suggestions(&v);
        out.push(v);
    };
    let mut current: Option<ERCViolation> = None;
    for line in py_splitlines(content) {
        if line.starts_with("**") {
            continue;
        }
        if let Some(c) = VIOLATION_START.captures(line) {
            if let Some(v) = current.take() {
                finish(v, &mut report.violations);
            }
            current = Some(ERCViolation::new(
                ERCViolationType::from_string(&c[1]),
                &c[1],
                ERCSeverity::Error,
                &c[2],
            ));
            continue;
        }
        let Some(v) = current.as_mut() else {
            continue;
        };
        if let Some(c) = LOC_LINE.captures(line) {
            let parse = |s: &str| {
                s.parse::<f64>()
                    .map_err(|_| anyhow!("could not convert string to float: '{s}'"))
            };
            let (x, y) = (parse(&c[1])?, parse(&c[2])?);
            v.set_pos(x, false, y, false);
            v.items.push(c[3].to_string());
            continue;
        }
        if let Some(c) = SEVERITY_LINE.captures(line) {
            v.severity = ERCSeverity::from_string(&c[1]);
            continue;
        }
        if let Some(c) = SHEET_LINE.captures(line) {
            v.sheet = c[1].trim().to_string();
        }
    }
    if let Some(v) = current.take() {
        finish(v, &mut report.violations);
    }
    Ok(report)
}
