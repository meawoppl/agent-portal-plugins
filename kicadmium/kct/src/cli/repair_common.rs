//! Shared helpers for the wave-D repair/optimize commands: manufacturer
//! choice lists (as upstream's argparse `choices=`), `pathlib` string
//! normalization, and `json.dumps(..., sort_keys=True)` output.

use crate::pyjson::{dumps_indent, Json};

/// `kicad_tools.manufacturers.get_manufacturer_ids()` (sorted canonical ids).
pub const MANUFACTURER_IDS: &[&str] = &[
    "flashpcb",
    "jlcpcb",
    "jlcpcb-tier1",
    "oshpark",
    "pcbway",
    "seeed",
];

/// `get_all_manufacturer_names()` (ids and aliases, sorted).
pub const ALL_MANUFACTURER_NAMES: &[&str] = &[
    "flash",
    "flashpcb",
    "jlc",
    "jlcpcb",
    "jlcpcb-capability-plus",
    "jlcpcb-capabilityplus",
    "jlcpcb-tier1",
    "jlcpcb_capabilityplus",
    "jlcpcb_tier1",
    "lcsc",
    "osh",
    "osh_park",
    "oshpark",
    "pcbway",
    "seeed",
    "seeed-fusion",
    "seeed_fusion",
    "seeedfusion",
    "seeedstudio",
];

/// `str(pathlib.Path(s))`: collapse `//`, drop `.` components and a
/// trailing slash (POSIX rules; a leading `//` is kept).
pub fn py_path_str(s: &str) -> String {
    if s.is_empty() {
        return ".".into();
    }
    let lead = if s.starts_with("//") && !s.starts_with("///") {
        "//"
    } else if s.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = s
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let body = parts.join("/");
    if body.is_empty() {
        if lead.is_empty() {
            ".".into()
        } else {
            lead.into()
        }
    } else {
        format!("{lead}{body}")
    }
}

/// Recursively sort object keys (`sort_keys=True`).
pub fn sort_keys(v: Json) -> Json {
    match v {
        Json::Obj(mut items) => {
            items.sort_by(|a, b| a.0.cmp(&b.0));
            Json::Obj(items.into_iter().map(|(k, v)| (k, sort_keys(v))).collect())
        }
        Json::Arr(items) => Json::Arr(items.into_iter().map(sort_keys).collect()),
        other => other,
    }
}

/// `format_options.emit_json`: `json.dumps(payload, indent=2, sort_keys=True)`.
pub fn emit_json(payload: Json) {
    println!("{}", dumps_indent(&sort_keys(payload), 2));
}

/// Python `bool` repr.
pub fn py_bool(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_normalization() {
        assert_eq!(py_path_str("./a//b/"), "a/b");
        assert_eq!(py_path_str("/x/./y"), "/x/y");
        assert_eq!(py_path_str("../z"), "../z");
        assert_eq!(py_path_str("."), ".");
    }
}

/// `manufacturers.base.load_design_rules_from_yaml(id)`: the YAML file named
/// exactly `<id>.yaml` (no alias resolution), configs in file order. `None`
/// is upstream's `FileNotFoundError`.
pub fn load_design_rules_from_yaml(
    manufacturer_id: &str,
) -> Option<Vec<(String, crate::manufacturers::DesignRules)>> {
    let text = match manufacturer_id {
        "flashpcb" => include_str!("../manufacturers/data/flashpcb.yaml"),
        "jlcpcb" => include_str!("../manufacturers/data/jlcpcb.yaml"),
        "jlcpcb_tier1" => include_str!("../manufacturers/data/jlcpcb_tier1.yaml"),
        "oshpark" => include_str!("../manufacturers/data/oshpark.yaml"),
        "pcbway" => include_str!("../manufacturers/data/pcbway.yaml"),
        "seeed" => include_str!("../manufacturers/data/seeed.yaml"),
        _ => return None,
    };
    let doc: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    let rules = doc.get("design_rules")?.as_mapping()?;
    let mut out = Vec::new();
    for (k, v) in rules {
        let name = k.as_str()?.to_string();
        let r: crate::manufacturers::DesignRules = serde_yaml::from_value(v.clone()).ok()?;
        out.push((name, r));
    }
    Some(out)
}

/// Pick `"{layers}layer_{int(copper)}oz"`, then `"{layers}layer_1oz"`, then
/// the first config (upstream fix-vias / fix-silkscreen lookup).
pub fn pick_rules(
    rules: &[(String, crate::manufacturers::DesignRules)],
    layers: i64,
    copper: f64,
) -> Option<crate::manufacturers::DesignRules> {
    let key = format!("{layers}layer_{}oz", copper.trunc() as i64);
    let one = format!("{layers}layer_1oz");
    rules
        .iter()
        .find(|(k, _)| *k == key)
        .or_else(|| rules.iter().find(|(k, _)| *k == one))
        .or_else(|| rules.first())
        .map(|(_, r)| r.clone())
}

/// Numeric atoms of a node's direct children (Python `float(atom)`).
pub fn atoms_f64(node: &crate::sexp::SExp) -> Vec<f64> {
    node.atoms().map(|v| v.as_f64().unwrap_or(0.0)).collect()
}

/// First atom as `f64` (0 when absent).
pub fn first_f64(node: Option<&crate::sexp::SExp>) -> f64 {
    node.and_then(|n| n.first_atom())
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}

/// First atom rendered as Python `str()` (empty when absent).
pub fn first_str(node: Option<&crate::sexp::SExp>) -> String {
    node.and_then(|n| n.text_at(0)).unwrap_or_default()
}

pub const DESIGN_EDIT_TARGET_ERROR: &str =
    "design edits require --output, --in-place, or --dry-run";
pub const EXIT_NO_WRITE_TARGET: i32 = 2;

pub fn require_write_target(dry_run: bool, output: Option<&str>, in_place: bool) -> Option<i32> {
    if dry_run || output.is_some() || in_place {
        return None;
    }
    eprintln!("Error: {DESIGN_EDIT_TARGET_ERROR}");
    Some(EXIT_NO_WRITE_TARGET)
}
