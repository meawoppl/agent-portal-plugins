//! Net-class routing rules (partial port of `kicad_tools.router.rules`:
//! the `NetClassRouting` fields the DRC checker consumes and the canonical
//! `--net-class-map` sidecar loader).

use std::path::Path;

use crate::pyjson::{self, Json};

/// Upstream `NetClassRouting` (checker-facing subset; the raw sidecar entry
/// is kept for fields not modeled here).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetClassRouting {
    pub name: String,
    pub target_ampacity: Option<f64>,
    pub target_single_impedance: Option<f64>,
    pub target_diff_impedance: Option<f64>,
    pub coupled_routing: bool,
    pub skew_tolerance_mm: Option<f64>,
    pub length_match_group: Option<String>,
    pub length_match_tolerance_mm: Option<f64>,
    pub coupled_continuity_threshold: Option<f64>,
    pub intra_pair_clearance: Option<f64>,
    pub raw: Json,
}

/// `{net_name: NetClassRouting}` in insertion order (upstream dict).
pub type NetClassMap = Vec<(String, NetClassRouting)>;

impl NetClassRouting {
    pub fn from_dict(data: &Json) -> Result<Self, String> {
        if !matches!(data, Json::Obj(_)) {
            return Err(format!(
                "net-class entry must be a dict, got {}",
                data.py_type_name()
            ));
        }
        let num = |k: &str| data.get(k).and_then(Json::as_f64);
        Ok(NetClassRouting {
            name: data
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or("Default")
                .to_string(),
            target_ampacity: num("target_ampacity"),
            target_single_impedance: num("target_single_impedance"),
            target_diff_impedance: num("target_diff_impedance"),
            coupled_routing: data.get("coupled_routing").is_some_and(Json::truthy),
            skew_tolerance_mm: num("skew_tolerance_mm"),
            length_match_group: data
                .get("length_match_group")
                .and_then(Json::as_str)
                .map(str::to_string),
            length_match_tolerance_mm: num("length_match_tolerance_mm"),
            coupled_continuity_threshold: num("coupled_continuity_threshold"),
            intra_pair_clearance: num("intra_pair_clearance"),
            raw: data.clone(),
        })
    }
}

/// `net_class_map_from_dict`: a dict of dicts (keys starting with `_` are
/// comments and skipped, Issue #4404).
pub fn net_class_map_from_json(data: &Json) -> Result<NetClassMap, String> {
    let Json::Obj(items) = data else {
        return Err(format!(
            "net-class-map must be a JSON object, got {}",
            data.py_type_name()
        ));
    };
    let mut out = Vec::new();
    for (k, v) in items {
        if k.starts_with('_') {
            continue;
        }
        out.push((k.clone(), NetClassRouting::from_dict(v)?));
    }
    Ok(out)
}

/// Canonical `--net-class-map` loader (`net_class_map_from_path`).
pub fn net_class_map_from_path(path: &Path, _pcb_path: Option<&Path>) -> Result<NetClassMap, String> {
    if !path.exists() {
        return Err(format!("net-class-map file not found: {}", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let data = pyjson::loads(&text).map_err(|e| format!("parsing net-class-map JSON: {e}"))?;
    net_class_map_from_json(&data).map_err(|e| format!("invalid net-class-map structure: {e}"))
}
