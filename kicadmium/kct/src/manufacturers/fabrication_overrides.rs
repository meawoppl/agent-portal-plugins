//! Validated per-board fabrication-floor overrides (port of
//! `kicad_tools.manufacturers.fabrication_overrides`, issue #5006).
//!
//! A `fabrication_overrides.json` sidecar may assert a cited, reviewed
//! floor (e.g. JLC's published 0.45 mm hole-to-hole) in place of a
//! profile's conservative default, but only for a registered
//! (manufacturer, field) pair and never below the verified floor.

use std::path::{Path, PathBuf};

use super::DesignRules;
use crate::pyjson::{self, py_float_repr, py_repr_str, Json};

pub const FABRICATION_OVERRIDES_SIDECAR_BASENAME: &str = "fabrication_overrides.json";

/// `_OVERRIDABLE_FIELDS`, sorted (as `sorted(frozenset)` prints it).
pub const OVERRIDABLE_FIELDS: [&str; 10] = [
    "min_annular_ring_mm",
    "min_clearance_mm",
    "min_copper_to_edge_mm",
    "min_hole_diameter_mm",
    "min_hole_to_edge_mm",
    "min_hole_to_hole_mm",
    "min_pth_annular_ring_mm",
    "min_trace_width_mm",
    "min_via_diameter_mm",
    "min_via_drill_mm",
];

/// Independently verified absolute capability floors.
/// <https://jlcpcb.com/capabilities/pcb-capabilities/> -- "Min Space between
/// any two holes must be >= 0.45mm" (verified 2026-09-10).
fn verified_floor(manufacturer_id: &str, field: &str) -> Option<f64> {
    match (manufacturer_id, field) {
        ("jlcpcb" | "jlcpcb-tier1", "min_hole_to_hole_mm") => Some(0.45),
        _ => None,
    }
}

/// One cited per-board override of a `DesignRules` field.
#[derive(Debug, Clone, PartialEq)]
pub struct FabricationOverride {
    pub field: String,
    pub value: f64,
    pub manufacturer_id: String,
    pub source: String,
    pub reason: String,
    pub tracking_issue: String,
}

pub fn fabrication_overrides_sidecar_candidates(pcb_path: &Path) -> Vec<PathBuf> {
    let dir = pcb_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let up = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    vec![
        dir.join(FABRICATION_OVERRIDES_SIDECAR_BASENAME),
        dir.join("output").join(FABRICATION_OVERRIDES_SIDECAR_BASENAME),
        up.join("output").join(FABRICATION_OVERRIDES_SIDECAR_BASENAME),
    ]
}

pub fn discover_fabrication_overrides_sidecar(pcb_path: &Path) -> Option<PathBuf> {
    fabrication_overrides_sidecar_candidates(pcb_path)
        .into_iter()
        .find(|c| c.is_file())
}

/// Python `str(value)` for a JSON value.
fn py_str(v: &Json) -> String {
    match v {
        Json::Str(s) => s.clone(),
        other => other.py_repr(),
    }
}

/// Python `float(value)` (error text as the raised exception's `str`).
fn py_float(v: &Json) -> Result<f64, String> {
    match v {
        Json::Int(i) => Ok(*i as f64),
        Json::Float(f) => Ok(*f),
        Json::Bool(b) => Ok(*b as i64 as f64),
        Json::Str(s) => {
            let t = s.trim();
            let num = regex::Regex::new(
                r"^[+-]?(?:\d(?:_?\d)*(?:\.(?:\d(?:_?\d)*)?)?|\.\d(?:_?\d)*)(?:[eE][+-]?\d(?:_?\d)*)?$",
            )
            .unwrap();
            let special = regex::Regex::new(r"(?i)^[+-]?(?:inf|infinity|nan)$").unwrap();
            if num.is_match(t) || special.is_match(t) {
                let lower = t.replace('_', "").to_ascii_lowercase();
                let v = match lower.trim_start_matches(['+', '-']) {
                    "inf" | "infinity" => f64::INFINITY,
                    "nan" => f64::NAN,
                    _ => lower.trim_start_matches('+').parse::<f64>().unwrap_or(f64::NAN).abs(),
                };
                return Ok(if lower.starts_with('-') { -v } else { v });
            }
            Err(format!("could not convert string to float: {}", py_repr_str(s)))
        }
        other => Err(format!(
            "float() argument must be a string or a real number, not '{}'",
            match other {
                Json::Null => "NoneType",
                Json::Arr(_) => "list",
                Json::Obj(_) => "dict",
                _ => other.py_type_name(),
            }
        )),
    }
}

/// Parse a fabrication-overrides sidecar.
pub fn load_fabrication_overrides(path: &Path) -> Result<Vec<FabricationOverride>, String> {
    let p = path.display();
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let data = match pyjson::loads(&text) {
        Ok(d) => d,
        Err(e) => {
            let msg = crate::utils::pyjsondecode::json_decode_error(&text)
                .unwrap_or_else(|| e.to_string());
            return Err(format!("{p}: not valid JSON: {msg}"));
        }
    };
    let entries = match &data {
        Json::Obj(_) => data.get("fabrication_overrides"),
        _ => None,
    };
    let Some(Json::Obj(entries)) = entries else {
        return Err(format!("{p}: missing top-level 'fabrication_overrides' object"));
    };
    let required = ["value", "manufacturer", "source", "reason", "tracking_issue"];
    let mut out = Vec::new();
    for (field, entry) in entries {
        let ok = matches!(entry, Json::Obj(_)) && required.iter().all(|k| entry.get(k).is_some());
        if !ok {
            return Err(format!(
                "{p}: override {} must be an object with keys ('value', 'manufacturer', \
                 'source', 'reason', 'tracking_issue')",
                py_repr_str(field)
            ));
        }
        let value = py_float(entry.get("value").unwrap()).map_err(|e| {
            format!(
                "{p}: override {} has non-numeric value: {e}",
                py_repr_str(field)
            )
        })?;
        let s = |k: &str| py_str(entry.get(k).unwrap());
        out.push(FabricationOverride {
            field: field.clone(),
            value,
            manufacturer_id: s("manufacturer"),
            source: s("source"),
            reason: s("reason"),
            tracking_issue: s("tracking_issue"),
        });
    }
    Ok(out)
}

/// Validate one override against the verified-floor registry.
pub fn validate_fabrication_override(o: &FabricationOverride, manufacturer_id: &str) -> Result<(), String> {
    let f = &o.field;
    if !OVERRIDABLE_FIELDS.contains(&f.as_str()) {
        let allowed: Vec<String> = OVERRIDABLE_FIELDS.iter().map(|s| py_repr_str(s)).collect();
        return Err(format!(
            "{} is not an overridable fabrication field (allowed: [{}])",
            py_repr_str(f),
            allowed.join(", ")
        ));
    }
    if o.source.trim().is_empty() {
        return Err(format!("{f}: override must cite a 'source'"));
    }
    if o.reason.trim().is_empty() {
        return Err(format!("{f}: override must state a 'reason'"));
    }
    if o.tracking_issue.trim().is_empty() {
        return Err(format!(
            "{f}: override must cite a 'tracking_issue' recording the review that approved it"
        ));
    }
    if o.manufacturer_id != manufacturer_id {
        return Err(format!(
            "{f}: override is scoped to manufacturer {}, but the active profile is {} -- an \
             override never silently carries across manufacturers",
            py_repr_str(&o.manufacturer_id),
            py_repr_str(manufacturer_id)
        ));
    }
    let Some(floor) = verified_floor(manufacturer_id, f) else {
        return Err(format!(
            "{f}: no independently verified capability floor is on record for manufacturer {} \
             -- cannot validate this override, refusing to apply it unverified",
            py_repr_str(manufacturer_id)
        ));
    };
    if o.value < floor {
        return Err(format!(
            "{f}: requested override {}mm is below the verified {} capability floor of {}mm ({}) \
             -- refusing to apply an unsafe override",
            py_float_repr(o.value),
            py_repr_str(manufacturer_id),
            py_float_repr(floor),
            o.source
        ));
    }
    Ok(())
}

fn set_field(rules: &mut DesignRules, field: &str, v: f64) {
    match field {
        "min_trace_width_mm" => rules.min_trace_width_mm = v,
        "min_clearance_mm" => rules.min_clearance_mm = v,
        "min_via_drill_mm" => rules.min_via_drill_mm = v,
        "min_via_diameter_mm" => rules.min_via_diameter_mm = v,
        "min_annular_ring_mm" => rules.min_annular_ring_mm = v,
        "min_pth_annular_ring_mm" => rules.min_pth_annular_ring_mm = Some(v),
        "min_hole_diameter_mm" => rules.min_hole_diameter_mm = v,
        "min_copper_to_edge_mm" => rules.min_copper_to_edge_mm = v,
        "min_hole_to_edge_mm" => rules.min_hole_to_edge_mm = v,
        "min_hole_to_hole_mm" => rules.min_hole_to_hole_mm = v,
        _ => {}
    }
}

/// Validate all overrides, then apply them (all-or-nothing).
pub fn apply_fabrication_overrides(
    rules: DesignRules,
    overrides: &[FabricationOverride],
    manufacturer_id: &str,
) -> Result<DesignRules, String> {
    if overrides.is_empty() {
        return Ok(rules);
    }
    for o in overrides {
        validate_fabrication_override(o, manufacturer_id)?;
    }
    let mut out = rules;
    for o in overrides {
        set_field(&mut out, &o.field, o.value);
    }
    Ok(out)
}

/// Discover, validate, and apply a board's sidecar; never fails (a bad
/// sidecar falls back to the profile defaults with an `ignoring ...` note).
pub fn resolve_pcb_fabrication_overrides(
    pcb_path: &Path,
    rules: DesignRules,
    manufacturer_id: &str,
) -> (DesignRules, Option<String>) {
    let Some(sidecar) = discover_fabrication_overrides_sidecar(pcb_path) else {
        return (rules, None);
    };
    let canonical = match super::profile(manufacturer_id) {
        Ok(p) => p.id.to_string(),
        Err(_) => manufacturer_id.to_string(),
    };
    let result = load_fabrication_overrides(&sidecar)
        .and_then(|ovs| apply_fabrication_overrides(rules.clone(), &ovs, &canonical).map(|r| (r, ovs)));
    match result {
        Err(e) => (
            rules,
            Some(format!(
                "ignoring fabrication-overrides sidecar {}: {e}. Falling back to the manufacturer \
                 profile's default floor.",
                sidecar.display()
            )),
        ),
        Ok((resolved, ovs)) => {
            let mut fields: Vec<&str> = ovs.iter().map(|o| o.field.as_str()).collect();
            fields.sort();
            fields.dedup();
            (
                resolved,
                Some(format!(
                    "applied fabrication-overrides sidecar {} ({})",
                    sidecar.display(),
                    fields.join(", ")
                )),
            )
        }
    }
}
