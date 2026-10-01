//! Pair-level courtyard-overlap waivers (port of
//! `kicad_tools.validate.rules.courtyard_waivers`, Issue #4137).

use std::path::{Path, PathBuf};

use crate::pyjson::{self, py_repr_str, Json};

pub const SUPPORTED_VERSION: i64 = 1;
const VALID_RULES: &[&str] = &["courtyards_overlap"];

/// One waiver entry; `refs` is stored sorted (order-insensitive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourtyardWaiver {
    pub rule: String,
    pub refs: (String, String),
    pub reason: String,
    pub issue: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CourtyardWaivers {
    pub entries: Vec<CourtyardWaiver>,
}

impl CourtyardWaivers {
    /// Waiver matching the unordered pair.
    pub fn find(&self, a: &str, b: &str) -> Option<&CourtyardWaiver> {
        let key = if a <= b { (a, b) } else { (b, a) };
        self.entries
            .iter()
            .find(|e| e.refs.0 == key.0 && e.refs.1 == key.1)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn nonempty_str<'a>(v: Option<&'a Json>) -> Option<&'a str> {
    v.and_then(Json::as_str).filter(|s| !s.is_empty())
}

fn parse_entry(idx: usize, raw: &Json) -> Result<CourtyardWaiver, String> {
    let w = format!("courtyard waiver #{idx}");
    if !matches!(raw, Json::Obj(_)) {
        return Err(format!("{w} must be an object, got {}", raw.py_type_name()));
    }
    let Some(rule) = nonempty_str(raw.get("rule")) else {
        return Err(format!("{w} is missing a non-empty string 'rule'"));
    };
    if !VALID_RULES.contains(&rule) {
        return Err(format!(
            "{w} has unsupported rule {} (expected one of ['courtyards_overlap'])",
            py_repr_str(rule)
        ));
    }
    let refs = match raw.get("refs") {
        Some(Json::Arr(a)) if a.len() == 2 => a,
        _ => return Err(format!("{w} 'refs' must be a list of exactly 2 references")),
    };
    let (Some(a), Some(b)) = (nonempty_str(Some(&refs[0])), nonempty_str(Some(&refs[1]))) else {
        return Err(format!("{w} 'refs' entries must be non-empty strings"));
    };
    if a == b {
        return Err(format!("{w} 'refs' must name two distinct components"));
    }
    let reason = match raw.get("reason").and_then(Json::as_str) {
        Some(r) if !r.trim().is_empty() => r,
        _ => return Err(format!("{w} is missing a non-empty 'reason'")),
    };
    let issue = match raw.get("issue").and_then(Json::as_str) {
        Some(r) if !r.trim().is_empty() => r,
        _ => return Err(format!("{w} is missing a non-empty 'issue'")),
    };
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    Ok(CourtyardWaiver {
        rule: rule.into(),
        refs: (lo.into(), hi.into()),
        reason: reason.into(),
        issue: issue.into(),
    })
}

/// `courtyard_waivers_from_dict`.
pub fn courtyard_waivers_from_json(data: &Json) -> Result<CourtyardWaivers, String> {
    if !matches!(data, Json::Obj(_)) {
        return Err(format!(
            "courtyard-waivers file must be a JSON object, got {}",
            data.py_type_name()
        ));
    }
    let Some(version) = data.get("version") else {
        return Err("courtyard-waivers file is missing the required 'version' key".into());
    };
    if version.as_f64() != Some(SUPPORTED_VERSION as f64) || matches!(version, Json::Bool(_)) {
        return Err(format!(
            "unsupported courtyard-waivers version {} (this build understands version {SUPPORTED_VERSION})",
            version.py_repr()
        ));
    }
    let raw = match data.get("waivers") {
        None => &[][..],
        Some(Json::Arr(a)) => a.as_slice(),
        Some(_) => return Err("courtyard-waivers 'waivers' must be a list".into()),
    };
    let mut entries = Vec::new();
    for (i, r) in raw.iter().enumerate() {
        entries.push(parse_entry(i, r)?);
    }
    Ok(CourtyardWaivers { entries })
}

/// `load_courtyard_waivers` (errors are upstream `ValueError` messages).
pub fn load_courtyard_waivers(path: &Path) -> Result<CourtyardWaivers, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let data = pyjson::loads(&text).map_err(|e| format!("parsing courtyard-waivers JSON: {e}"))?;
    courtyard_waivers_from_json(&data)
}

/// Probe `<dir>`, `<dir>/output`, `<dir>/../output`.
pub fn discover_courtyard_waivers_sidecar(pcb_path: &Path) -> Option<PathBuf> {
    let dir = pcb_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let up = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = ".courtyard_waivers.json";
    [
        dir.join(name),
        dir.join("output").join(name),
        up.join("output").join(name),
    ]
    .into_iter()
    .find(|c| c.is_file())
}
