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
