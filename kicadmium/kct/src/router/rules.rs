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
    pub diffpair_partner: Option<String>,
    pub length_match_reference: Option<String>,
    /// `clearance` (default 0.2 mm).
    pub clearance: f64,
    /// `trace_width` (default 0.2 mm).
    pub trace_width: f64,
    /// `impedance_tolerance_percent` (default 10.0).
    pub impedance_tolerance_percent: f64,
    pub raw: Json,
}

/// `{net_name: NetClassRouting}` in insertion order (upstream dict).
pub type NetClassMap = Vec<(String, NetClassRouting)>;

impl NetClassRouting {
    pub fn from_dict(data: &Json) -> Result<Self, String> {
        let Json::Obj(items) = data else {
            return Err(format!(
                "net-class entry must be a dict, got {}",
                data.py_type_name()
            ));
        };
        let Some(name) = data.get("name") else {
            let mut keys: Vec<String> = items.iter().map(|(k, _)| pyjson::py_repr_str(k)).collect();
            keys.sort();
            return Err(format!(
                "NetClassRouting.from_dict requires a 'name' field; got keys: [{}]",
                keys.join(", ")
            ));
        };
        let num = |k: &str| data.get(k).and_then(Json::as_f64);
        let text = |k: &str| data.get(k).and_then(Json::as_str).map(str::to_string);
        Ok(NetClassRouting {
            name: match name {
                Json::Str(s) => s.clone(),
                other => other.py_repr(),
            },
            target_ampacity: num("target_ampacity"),
            target_single_impedance: num("target_single_impedance"),
            target_diff_impedance: num("target_diff_impedance"),
            coupled_routing: data.get("coupled_routing").is_some_and(Json::truthy),
            skew_tolerance_mm: num("skew_tolerance_mm"),
            length_match_group: text("length_match_group"),
            length_match_tolerance_mm: num("length_match_tolerance_mm"),
            coupled_continuity_threshold: num("coupled_continuity_threshold"),
            intra_pair_clearance: num("intra_pair_clearance"),
            diffpair_partner: text("diffpair_partner"),
            length_match_reference: text("length_match_reference"),
            clearance: num("clearance").unwrap_or(0.2),
            trace_width: num("trace_width").unwrap_or(0.2),
            impedance_tolerance_percent: num("impedance_tolerance_percent").unwrap_or(10.0),
            raw: data.clone(),
        })
    }

    /// `effective_intra_pair_clearance()`.
    pub fn effective_intra_pair_clearance(&self) -> f64 {
        self.intra_pair_clearance.unwrap_or(self.clearance)
    }

    /// `effective_coupled_continuity_threshold(default)`.
    pub fn effective_coupled_continuity_threshold(&self, default: f64) -> f64 {
        self.coupled_continuity_threshold.unwrap_or(default)
    }

    /// `effective_skew_tolerance(default)`.
    pub fn effective_skew_tolerance(&self, default: f64) -> f64 {
        self.skew_tolerance_mm.unwrap_or(default)
    }

    /// `effective_length_match_tolerance(default)`.
    pub fn effective_length_match_tolerance(&self, default: f64) -> f64 {
        self.length_match_tolerance_mm.unwrap_or(default)
    }
}

/// `net_class_map_from_dict`: a dict of dicts (keys starting with `_` are
/// comments and skipped, Issue #4404).
pub fn net_class_map_from_json(data: &Json) -> Result<NetClassMap, String> {
    let Json::Obj(items) = data else {
        return Err(format!(
            "net_class_map_from_dict expects a dict, got {}",
            data.py_type_name()
        ));
    };
    let mut out: NetClassMap = Vec::new();
    for (k, v) in items {
        if k.starts_with('_') || k == "spatial_keepouts" {
            continue;
        }
        if !matches!(v, Json::Obj(_)) {
            return Err(format!(
                "net_class_map entry for {} must be a dict, got {}",
                pyjson::py_repr_str(k),
                v.py_type_name()
            ));
        }
        let nc = NetClassRouting::from_dict(v)?;
        match out.iter_mut().find(|(n, _)| n == k) {
            Some(e) => e.1 = nc,
            None => out.push((k.clone(), nc)),
        }
    }
    Ok(out)
}

/// Canonical `--net-class-map` loader (`net_class_map_from_path`).
pub fn net_class_map_from_path(
    path: &Path,
    _pcb_path: Option<&Path>,
) -> Result<NetClassMap, String> {
    if !path.exists() {
        return Err(format!("net-class-map file not found: {}", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let data = pyjson::loads(&text).map_err(|e| {
        let msg =
            crate::utils::pyjsondecode::json_decode_error(&text).unwrap_or_else(|| e.to_string());
        format!("parsing net-class-map JSON: {msg}")
    })?;
    net_class_map_from_json(&data).map_err(|e| format!("invalid net-class-map structure: {e}"))
}
