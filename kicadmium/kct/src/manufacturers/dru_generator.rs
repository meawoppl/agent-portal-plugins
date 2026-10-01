//! `.kicad_dru` generation from manufacturer design rules (port of
//! `kicad_tools.manufacturers.dru_generator`): tier-floor rules, IPC-2221
//! ampacity net-class widths, and the marker-guarded managed-block merge
//! (issue #4600).

use std::sync::LazyLock;

use regex::Regex;

use super::DesignRules;
use crate::physics::ampacity::width_for_current;
use crate::pyjson::py_float_repr;
use crate::router::rules::NetClassRouting;

pub const DRU_FLOORS_BLOCK_BEGIN: &str =
    "# BEGIN kct fab floors (Issue #4600) -- managed, do not edit";
pub const DRU_FLOORS_BLOCK_END: &str = "# END kct fab floors";
pub const DRU_VERSION_HEADER: &str = "(version 1)";
pub const SMD_PAD_CLEARANCE_MIN_KICAD_VERSION: [u64; 3] = [10, 0, 2];
pub const SMD_PAD_CLEARANCE_MIN_KICAD_VERSION_STR: &str = "10.0.2";

/// Leading dotted-numeric run of `kicad-cli version` output.
pub fn parse_kicad_cli_version(raw: Option<&str>) -> Option<Vec<u64>> {
    let first = raw.unwrap_or("").split_whitespace().next()?;
    let head = first
        .split('-')
        .next()
        .unwrap_or("")
        .split('~')
        .next()
        .unwrap_or("");
    let mut parts = Vec::new();
    for chunk in head.split('.') {
        if chunk.is_empty() || !chunk.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        parts.push(chunk.parse().ok()?);
    }
    (!parts.is_empty()).then_some(parts)
}

/// Why the emitted `SMD Pad Clearance` rule cannot fire on this kicad-cli.
pub fn smd_pad_clearance_inert_reason(raw: Option<&str>) -> Option<String> {
    let v = parse_kicad_cli_version(raw)?;
    if v.as_slice() >= &SMD_PAD_CLEARANCE_MIN_KICAD_VERSION[..] {
        return None;
    }
    let installed = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown");
    let floor = SMD_PAD_CLEARANCE_MIN_KICAD_VERSION_STR;
    Some(format!(
        "the emitted 'SMD Pad Clearance' rule is INERT on the installed kicad-cli {installed}: it \
         requires KiCad >= {floor}, where a pad first inherits its parent footprint's Reference. \
         Below that the rule's \"A.Reference != B.Reference\" scope is permanently false, so \
         'kicad-cli pcb drc' reports a clean board without raising any rule error. Upgrade KiCad \
         to >= {floor}, or rely on 'kct check' (whose engine-independent clearance rule enforces \
         the same floor), before treating a native DRC pass as a different-net SMD pad clearance \
         pass."
    ))
}

static LEGACY_RULE_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:Trace Width|Clearance|Via Drill|Via Diameter|Annular Ring|PTH Annular Ring|Copper to Edge|Hole to Edge|Silkscreen Width|Silkscreen Height|SMD Pad Clearance|Solder Mask Clearance|Solder Mask Dam|Ampacity Min Width \(.+, (?:external|internal)\))(?: - .+)?$",
    )
    .unwrap()
});
static LEGACY_VERSION_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\(version \d+\)$").unwrap());
static LEGACY_RULE_OPEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\(rule "([^"]+)"$"#).unwrap());
static LEGACY_CONDITION_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^  \(condition "[^"]*"\)$"#).unwrap());
static LEGACY_CONSTRAINT_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^  \(constraint \w+ \(min [0-9.]+mm\)\)\)$").unwrap());
static VERSION_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\(version\b").unwrap());
static VERSION_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\(version[^)]*\)\s*\n?").unwrap());
static VERSION_ANY_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*\(version\b").unwrap());

/// Python `str.splitlines()` for `\n` / `\r\n` / `\r` text.
fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        match rest.find(['\n', '\r']) {
            Some(i) => {
                out.push(&rest[..i]);
                let skip = if rest[i..].starts_with("\r\n") { 2 } else { 1 };
                rest = &rest[i + skip..];
            }
            None => {
                out.push(rest);
                break;
            }
        }
    }
    out
}

fn is_legacy_generated_dru(existing: &str) -> bool {
    if !VERSION_START_RE.is_match(existing) {
        return false;
    }
    let mut saw_rule = false;
    for line in splitlines(existing) {
        if line.trim().is_empty() || LEGACY_VERSION_LINE_RE.is_match(line) {
            continue;
        }
        if let Some(c) = LEGACY_RULE_OPEN_RE.captures(line) {
            if !LEGACY_RULE_NAME_RE.is_match(&c[1]) {
                return false;
            }
            saw_rule = true;
            continue;
        }
        if LEGACY_CONDITION_LINE_RE.is_match(line) || LEGACY_CONSTRAINT_LINE_RE.is_match(line) {
            continue;
        }
        return false;
    }
    saw_rule
}

/// Merge fresh `generate_dru` output into existing `.kicad_dru` content.
pub fn merge_dru_floors(existing: Option<&str>, dru_content: &str, path: Option<&str>) -> String {
    let body = VERSION_HEADER_RE.replace(dru_content, "");
    let body = body.trim_matches('\n');
    let block = format!("{DRU_FLOORS_BLOCK_BEGIN}\n{body}\n{DRU_FLOORS_BLOCK_END}");
    let Some(existing) = existing.filter(|e| !e.trim().is_empty()) else {
        return format!("{DRU_VERSION_HEADER}\n\n{block}\n");
    };
    let pattern = Regex::new(&format!(
        "(?s){}.*?{}",
        regex::escape(DRU_FLOORS_BLOCK_BEGIN),
        regex::escape(DRU_FLOORS_BLOCK_END)
    ))
    .unwrap();
    if pattern.is_match(existing) {
        let merged = pattern
            .replace_all(existing, regex::NoExpand(&block))
            .to_string();
        return if merged.ends_with('\n') {
            merged
        } else {
            merged + "\n"
        };
    }
    if is_legacy_generated_dru(existing) {
        let name = path.unwrap_or(".kicad_dru content");
        eprintln!(
            "Warning: {name}: pre-#4600 generated sidecar detected -- replaced with the marked \
             managed block (see Issue #4667). Hand-authored rules matching the generated grammar \
             are treated as generated; restore from VCS if this file was yours."
        );
        return format!("{DRU_VERSION_HEADER}\n\n{block}\n");
    }
    let mut prefix = if existing.ends_with('\n') {
        existing.to_string()
    } else {
        format!("{existing}\n")
    };
    if !VERSION_ANY_LINE_RE.is_match(existing) {
        prefix = format!("{DRU_VERSION_HEADER}\n{prefix}");
    }
    format!("{prefix}\n{block}\n")
}

fn f(v: f64) -> String {
    py_float_repr(v)
}

/// `generate_dru(rules, manufacturer_name, net_classes)`.
pub fn generate_dru(
    rules: &DesignRules,
    manufacturer_name: &str,
    net_classes: &[NetClassRouting],
) -> String {
    let sfx = if manufacturer_name.is_empty() {
        String::new()
    } else {
        format!(" - {manufacturer_name}")
    };
    let mut lines = vec!["(version 1)".to_string()];
    lines.push(format!(
        "(rule \"Trace Width{sfx}\"\n  (condition \"A.Type == 'track'\")\n  (constraint track_width (min {}mm)))",
        f(rules.min_trace_width_mm)
    ));
    lines.push(format!(
        "(rule \"Clearance{sfx}\"\n  (constraint clearance (min {}mm)))",
        f(rules.min_clearance_mm)
    ));
    if let Some(v) = rules.min_silk_to_pad_clearance_mm {
        lines.push(format!(
            "(rule \"Silk to Pad{sfx}\"\n  (condition \"A.Type == 'Pad' || B.Type == 'Pad'\")\n  (constraint silk_clearance (min {}mm)))",
            f(v)
        ));
    }
    if let Some(v) = rules.min_smd_pad_clearance_mm {
        lines.push(format!(
            "(rule \"SMD Pad Clearance{sfx}\"\n  (condition \"A.Type == 'Pad' && B.Type == 'Pad' && A.Pad_Type == 'SMD' && B.Pad_Type == 'SMD' && A.Net != B.Net && A.Reference != B.Reference\")\n  (constraint clearance (min {}mm)))",
            f(v)
        ));
    }
    if let Some(v) = rules.min_pth_hole_to_track_mm {
        lines.push(format!(
            "(rule \"PTH Hole to Track{sfx}\"\n  (condition \"(A.Pad_Type == 'Through-hole' && B.Type == 'Track') || (B.Pad_Type == 'Through-hole' && A.Type == 'Track')\")\n  (constraint hole_clearance (min {}mm)))",
            f(v)
        ));
    }
    if let Some(inner) = rules.min_inner_pth_hole_to_copper_mm {
        // `max(min_pth_hole_to_track_mm or 0, inner)`: Python keeps the int 0
        // only when it is strictly greater, which a non-negative floor never is.
        let base = rules
            .min_pth_hole_to_track_mm
            .filter(|v| *v != 0.0)
            .unwrap_or(0.0);
        let min = if base > inner { base } else { inner };
        lines.push(format!(
            "(rule \"Inner PTH Hole to Copper{sfx}\"\n  (layer inner)\n  (condition \"A.Pad_Type == 'Through-hole' || B.Pad_Type == 'Through-hole'\")\n  (constraint hole_clearance (min {}mm)))",
            f(min)
        ));
    }
    lines.push(format!(
        "(rule \"Via Drill{sfx}\"\n  (condition \"A.Type == 'via' && A.Via_Type != 'Micro'\")\n  (constraint hole_size (min {}mm)))",
        f(rules.min_via_drill_mm)
    ));
    lines.push(format!(
        "(rule \"Via Diameter{sfx}\"\n  (condition \"A.Type == 'via' && A.Via_Type != 'Micro'\")\n  (constraint via_diameter (min {}mm)))",
        f(rules.min_via_diameter_mm)
    ));
    lines.push(format!(
        "(rule \"Annular Ring{sfx}\"\n  (condition \"A.Via_Type != 'Micro'\")\n  (constraint annular_width (min {}mm)))",
        f(rules.min_annular_ring_mm)
    ));
    if let Some(v) = rules.min_pth_annular_ring_mm {
        lines.push(format!(
            "(rule \"PTH Annular Ring{sfx}\"\n  (condition \"A.Type == 'pad'\")\n  (constraint annular_width (min {}mm)))",
            f(v)
        ));
    }
    lines.push(format!(
        "(rule \"Copper to Edge{sfx}\"\n  (constraint edge_clearance (min {}mm)))",
        f(rules.min_copper_to_edge_mm)
    ));
    lines.push(format!(
        "(rule \"Hole to Edge{sfx}\"\n  (condition \"(A.Type == 'via' || A.Type == 'pad') && B.Layer == 'Edge.Cuts'\")\n  (constraint physical_hole_clearance (min {}mm)))",
        f(rules.min_hole_to_edge_mm)
    ));
    lines.push(format!(
        "(rule \"Silkscreen Width{sfx}\"\n  (condition \"A.Type == 'text' && A.Layer == 'F.Silkscreen'\")\n  (constraint text_thickness (min {}mm)))",
        f(rules.min_silkscreen_width_mm)
    ));
    lines.push(format!(
        "(rule \"Silkscreen Height{sfx}\"\n  (condition \"A.Type == 'text' && A.Layer == 'F.Silkscreen'\")\n  (constraint text_height (min {}mm)))",
        f(rules.min_silkscreen_height_mm)
    ));
    for nc in net_classes {
        let Some(amps) = nc.target_ampacity else {
            continue;
        };
        let ext =
            width_for_current(amps, rules.outer_copper_oz, 10.0, "external").unwrap_or(f64::NAN);
        let int =
            width_for_current(amps, rules.inner_copper_oz, 10.0, "internal").unwrap_or(f64::NAN);
        let n = &nc.name;
        lines.push(format!(
            "(rule \"Ampacity Min Width ({n}, external){sfx}\"\n  (condition \"A.NetClass == '{n}' && A.Type == 'track' && (A.Layer == 'F.Cu' || A.Layer == 'B.Cu')\")\n  (constraint track_width (min {ext:.4}mm)))"
        ));
        lines.push(format!(
            "(rule \"Ampacity Min Width ({n}, internal){sfx}\"\n  (condition \"A.NetClass == '{n}' && A.Type == 'track' && A.Layer != 'F.Cu' && A.Layer != 'B.Cu'\")\n  (constraint track_width (min {int:.4}mm)))"
        ));
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_and_merge() {
        assert_eq!(
            parse_kicad_cli_version(Some("10.0.1-1~ubuntu24.04.1 release build")),
            Some(vec![10, 0, 1])
        );
        assert_eq!(parse_kicad_cli_version(Some("")), None);
        assert!(smd_pad_clearance_inert_reason(Some("10.0.6")).is_none());
        assert!(smd_pad_clearance_inert_reason(Some("10.0.1")).is_some());
        let merged = merge_dru_floors(None, "(version 1)\n(rule \"X\")\n", None);
        assert_eq!(
            merged,
            format!(
                "(version 1)\n\n{DRU_FLOORS_BLOCK_BEGIN}\n(rule \"X\")\n{DRU_FLOORS_BLOCK_END}\n"
            )
        );
        assert_eq!(
            merge_dru_floors(Some(&merged), "(version 1)\n(rule \"X\")\n", None),
            merged
        );
    }
}
