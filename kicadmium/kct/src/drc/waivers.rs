//! `.kct_waivers.json` (schema v2) waivers for kicad-cli DRC reports (port of
//! `kicad_tools.drc.waivers` plus the loader half of
//! `kicad_tools.validate.rules.waivers`).
//!
//! `waiver.rule` is matched against KiCad's own `type_str` verbatim; item
//! descriptions are normalized to component refs via
//! [`extract_item_refs`] before exact-set matching.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::report::DRCReport;
use super::violation::extract_item_refs;
use crate::jobj;
use crate::pyjson::{self, py_repr_str, py_repr_str_list, Json};

/// The only schema version understood by the loader.
pub const SUPPORTED_VERSION: i64 = 2;
/// Rule id of the advisory emitted for unused waivers (`kct check`).
pub const WAIVER_UNUSED_RULE_ID: &str = "waiver_unused";
/// Sidecar filename.
pub const WAIVERS_FILENAME: &str = ".kct_waivers.json";

/// A single waiver entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiver {
    pub rule: String,
    pub items: BTreeSet<String>,
    pub nets: BTreeSet<String>,
    pub reason: String,
    pub issue: String,
}

impl Waiver {
    /// Engine-agnostic match: rule id plus exact-set item/net match (an empty
    /// waiver axis does not constrain).
    pub fn matches_normalized(
        &self,
        rule: &str,
        items: &BTreeSet<String>,
        nets: &BTreeSet<String>,
    ) -> bool {
        rule == self.rule
            && (self.items.is_empty() || *items == self.items)
            && (self.nets.is_empty() || *nets == self.nets)
    }

    /// Match a `kct check`-style finding (rule id + verbatim items/nets).
    pub fn matches(&self, rule_id: &str, items: &[String], nets: &[String]) -> bool {
        self.matches_normalized(
            rule_id,
            &items.iter().cloned().collect(),
            &nets.iter().cloned().collect(),
        )
    }

    fn sorted_items(&self) -> Vec<&str> {
        self.items.iter().map(String::as_str).collect()
    }

    fn sorted_nets(&self) -> Vec<&str> {
        self.nets.iter().map(String::as_str).collect()
    }

    fn scope_str(&self) -> String {
        let mut parts = Vec::new();
        if !self.items.is_empty() {
            parts.push(format!("items={}", py_repr_str_list(&self.sorted_items())));
        }
        if !self.nets.is_empty() {
            parts.push(format!("nets={}", py_repr_str_list(&self.sorted_nets())));
        }
        parts.join(", ")
    }
}

/// A loaded, validated collection of waiver entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Waivers {
    pub entries: Vec<Waiver>,
}

impl Waivers {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// First waiver matching a `kct check` finding.
    pub fn find_match(&self, rule_id: &str, items: &[String], nets: &[String]) -> Option<&Waiver> {
        self.entries
            .iter()
            .find(|w| w.matches(rule_id, items, nets))
    }
}

/// Malformed waivers file (upstream `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaiverError(pub String);

impl std::fmt::Display for WaiverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WaiverError {}

fn err<T>(msg: String) -> Result<T, WaiverError> {
    Err(WaiverError(msg))
}

/// Build [`Waivers`] from parsed JSON.
pub fn waivers_from_json(data: &Json) -> Result<Waivers, WaiverError> {
    if !matches!(data, Json::Obj(_)) {
        return err(format!(
            "waivers file must be a JSON object, got {}",
            data.py_type_name()
        ));
    }
    let Some(version) = data.get("version") else {
        return err("waivers file is missing the required 'version' key".into());
    };
    // Python `==` treats 2.0 and True-ish numerics by value.
    if version.as_f64() != Some(SUPPORTED_VERSION as f64) || matches!(version, Json::Bool(_)) {
        return err(format!(
            "unsupported waivers version {} (this build understands version {SUPPORTED_VERSION})",
            version.py_repr()
        ));
    }
    let raw = match data.get("waivers") {
        None => &[][..],
        Some(Json::Arr(a)) => a.as_slice(),
        Some(_) => return err("waivers 'waivers' must be a list".into()),
    };
    let mut entries = Vec::new();
    for (idx, entry) in raw.iter().enumerate() {
        entries.push(parse_entry(idx, entry)?);
    }
    Ok(Waivers { entries })
}

fn parse_str_set(
    where_: &str,
    key: &str,
    raw: Option<&Json>,
) -> Result<BTreeSet<String>, WaiverError> {
    match raw {
        None | Some(Json::Null) => Ok(BTreeSet::new()),
        Some(Json::Arr(list)) => {
            let mut out = BTreeSet::new();
            for x in list {
                match x.as_str() {
                    Some(s) if !s.is_empty() => {
                        out.insert(s.to_string());
                    }
                    _ => {
                        return err(format!(
                            "{where_} '{key}' entries must be non-empty strings"
                        ))
                    }
                }
            }
            Ok(out)
        }
        Some(_) => err(format!("{where_} '{key}' must be a list of strings")),
    }
}

fn parse_entry(idx: usize, raw: &Json) -> Result<Waiver, WaiverError> {
    let where_ = format!("waiver #{idx}");
    if !matches!(raw, Json::Obj(_)) {
        return err(format!(
            "{where_} must be an object, got {}",
            raw.py_type_name()
        ));
    }
    let rule = match raw.get("rule").and_then(Json::as_str) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => return err(format!("{where_} is missing a non-empty string 'rule'")),
    };
    let items = parse_str_set(&where_, "items", raw.get("items"))?;
    let nets = parse_str_set(&where_, "nets", raw.get("nets"))?;
    if items.is_empty() && nets.is_empty() {
        return err(format!(
            "{where_} must name at least one 'items' or 'nets' entry"
        ));
    }
    let reason = match raw.get("reason").and_then(Json::as_str) {
        Some(r) if !r.trim().is_empty() => r.to_string(),
        _ => return err(format!("{where_} is missing a non-empty 'reason'")),
    };
    let issue = match raw.get("issue").and_then(Json::as_str) {
        Some(r) if !r.trim().is_empty() => r.to_string(),
        _ => return err(format!("{where_} is missing a non-empty 'issue'")),
    };
    Ok(Waiver {
        rule,
        items,
        nets,
        reason,
        issue,
    })
}

/// Load and validate a `.kct_waivers.json` file.
pub fn load_waivers(path: &Path) -> Result<Waivers, WaiverError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| WaiverError(format!("[Errno 2] {e}: '{}'", path.display())))?;
    let data =
        pyjson::loads(&text).map_err(|e| WaiverError(format!("parsing waivers JSON: {e}")))?;
    waivers_from_json(&data)
}

/// Python `Path.parent` (empty path stands for `.`).
pub(crate) fn py_parent(p: &Path) -> PathBuf {
    match p.parent() {
        Some(parent) => parent.to_path_buf(),
        None => PathBuf::new(),
    }
}

/// Probe `<dir>/`, `<dir>/output/`, `<dir>/../output/` for the sidecar.
pub fn discover_waivers_sidecar(pcb_path: &Path) -> Option<PathBuf> {
    let dir = py_parent(pcb_path);
    let parent = if dir.as_os_str().is_empty() || dir == Path::new(".") {
        PathBuf::new()
    } else {
        py_parent(&dir)
    };
    [
        dir.join(WAIVERS_FILENAME),
        dir.join("output").join(WAIVERS_FILENAME),
        parent.join("output").join(WAIVERS_FILENAME),
    ]
    .into_iter()
    .find(|c| c.is_file())
}

/// Outcome of applying waivers to a DRC report.
#[derive(Debug, Clone, Default)]
pub struct WaiverApplication {
    /// Indices into `report.violations` that a waiver matched.
    pub waived: Vec<usize>,
    /// Entries that matched no violation (advisory only).
    pub unused: Vec<Waiver>,
}

impl WaiverApplication {
    pub fn waived_count(&self) -> usize {
        self.waived.len()
    }

    pub fn unused_messages(&self) -> Vec<String> {
        self.unused.iter().map(unused_message).collect()
    }

    /// JSON view of the unused entries (`--format json`).
    pub fn to_json_list(&self) -> Json {
        Json::Arr(
            self.unused
                .iter()
                .map(|w| {
                    jobj! {
                        "rule" => w.rule.as_str(),
                        "items" => w.sorted_items().into_iter().map(Json::from).collect::<Vec<_>>(),
                        "nets" => w.sorted_nets().into_iter().map(Json::from).collect::<Vec<_>>(),
                        "reason" => w.reason.as_str(),
                        "issue" => w.issue.as_str(),
                        "message" => unused_message(w),
                    }
                })
                .collect(),
        )
    }
}

fn unused_message(entry: &Waiver) -> String {
    format!(
        "Waiver for rule {} ({}) matched no violation (tracking {}); the underlying defect may \
         already be resolved, the refs may have changed, or the entry may be keyed to another \
         engine's rule id.",
        py_repr_str(&entry.rule),
        entry.scope_str(),
        entry.issue
    )
}

/// Mark every waiver-matched violation in `report` as waived, in place.
/// First matching entry wins; already-waived violations are skipped.
pub fn apply_waivers_to_report(report: &mut DRCReport, waivers: &Waivers) -> WaiverApplication {
    let mut result = WaiverApplication::default();
    if waivers.entries.is_empty() {
        return result;
    }
    let mut used = BTreeSet::new();
    for (vi, violation) in report.violations.iter_mut().enumerate() {
        if violation.waived {
            continue;
        }
        let refs = extract_item_refs(&violation.items);
        let nets: BTreeSet<String> = violation.nets.iter().cloned().collect();
        if let Some((idx, entry)) = waivers
            .entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.matches_normalized(&violation.type_str, &refs, &nets))
        {
            violation.waived = true;
            violation.waiver_reason = Some(entry.reason.clone());
            violation.waiver_issue = Some(entry.issue.clone());
            used.insert(idx);
            result.waived.push(vi);
        }
    }
    result.unused = waivers
        .entries
        .iter()
        .enumerate()
        .filter(|(i, _)| !used.contains(i))
        .map(|(_, e)| e.clone())
        .collect();
    result
}
