//! `kct check` honours the board's own `.kicad_pro` / `.kicad_dru` minima
//! over an auto-selected manufacturer profile.
//!
//! Fixture: bp-test `boards/esp32-fpga-module` (KiCad DRC clean), trimmed to
//! the B.Cu GND pour. Its project sets min clearance/track 0.1 mm, hole to
//! hole 0.25 mm, via 0.45/0.2 mm. Before the fix the default `jlcpcb`
//! 4-layer profile (0.1016 mm, 0.5 mm hole-to-hole, 0.25 mm pads) produced
//! ~1000 false errors on this trimmed board.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Rule classes that fired only because the profile overrode the project.
const FALSE_POSITIVE_CLASSES: &[&str] = &[
    "dimension_trace_width",
    "min_pad_size",
    "hole_to_hole_clearance",
    "clearance_via_zone",
    "clearance_segment_zone",
    "clearance_pad_zone",
    "clearance_segment_via",
    "clearance_segment_segment",
    "clearance_pad_segment",
    "clearance_pad_via",
];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project_rules")
}

/// Copy the fixture board (and optionally its project files) into `dir`.
fn stage(dir: &Path, with_project: bool) -> PathBuf {
    let src = fixture_dir();
    std::fs::copy(src.join("module.kicad_pcb"), dir.join("module.kicad_pcb")).unwrap();
    if with_project {
        for f in ["module.kicad_pro", "module.kicad_dru"] {
            std::fs::copy(src.join(f), dir.join(f)).unwrap();
        }
    }
    dir.join("module.kicad_pcb")
}

/// Run `kct check --drc-only --format json` and return error counts by rule
/// plus the `required_value`s seen for each rule.
fn errors(pcb: &Path, extra: &[&str]) -> (BTreeMap<String, usize>, BTreeMap<String, Vec<f64>>) {
    let out = pcb.with_file_name("report.json");
    let mut args: Vec<String> = vec![
        "check".into(),
        pcb.display().to_string(),
        "--format".into(),
        "json".into(),
        "--drc-only".into(),
        "--output".into(),
        out.display().to_string(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    kct::cli::run(args).unwrap();
    let report: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let mut counts = BTreeMap::new();
    let mut required: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for v in report["violations"].as_array().unwrap() {
        if v["severity"] != "error" {
            continue;
        }
        let rule = v["rule_id"].as_str().unwrap().to_string();
        *counts.entry(rule.clone()).or_insert(0) += 1;
        if let Some(r) = v["required_value"].as_f64() {
            required.entry(rule).or_default().push(r);
        }
    }
    (counts, required)
}

fn false_positives(counts: &BTreeMap<String, usize>) -> Vec<(&String, &usize)> {
    counts
        .iter()
        .filter(|(k, _)| FALSE_POSITIVE_CLASSES.contains(&k.as_str()))
        .collect()
}

#[test]
fn project_minima_override_default_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let pcb = stage(tmp.path(), true);
    let (counts, _) = errors(&pcb, &[]);
    assert!(
        false_positives(&counts).is_empty(),
        "profile minima leaked past the project rules: {counts:?}"
    );
}

#[test]
fn without_project_file_profile_defaults_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let pcb = stage(tmp.path(), false);
    let (counts, required) = errors(&pcb, &[]);
    assert!(counts.get("dimension_trace_width").copied().unwrap_or(0) > 0);
    assert!(counts.get("min_pad_size").copied().unwrap_or(0) > 0);
    assert!(required["hole_to_hole_clearance"]
        .iter()
        .all(|r| (*r - 0.5).abs() < 1e-9));
}

#[test]
fn explicit_mfr_is_a_fab_capability_check() {
    let tmp = tempfile::tempdir().unwrap();
    let pcb = stage(tmp.path(), true);
    let (counts, _) = errors(&pcb, &["--mfr", "jlcpcb"]);
    assert!(counts.get("dimension_trace_width").copied().unwrap_or(0) > 0);
    assert!(counts.get("hole_to_hole_clearance").copied().unwrap_or(0) > 0);
}

#[test]
fn real_violations_below_project_minima_still_fire() {
    let tmp = tempfile::tempdir().unwrap();
    let pcb = stage(tmp.path(), true);
    // One 0.15 mm +3.3V track narrowed to 0.08 mm (< project 0.1 mm).
    let text = std::fs::read_to_string(&pcb).unwrap();
    let needle = "(end 119 139)\n\t\t(width 0.15)";
    assert_eq!(text.matches(needle).count(), 1);
    std::fs::write(
        &pcb,
        text.replace(needle, "(end 119 139)\n\t\t(width 0.08)"),
    )
    .unwrap();
    let (counts, required) = errors(&pcb, &[]);
    assert_eq!(counts.get("dimension_trace_width"), Some(&1), "{counts:?}");
    assert_eq!(required["dimension_trace_width"], vec![0.1]);
}

#[test]
fn stricter_project_minima_are_enforced_too() {
    let tmp = tempfile::tempdir().unwrap();
    let pcb = stage(tmp.path(), true);
    let pro = pcb.with_extension("kicad_pro");
    let text = std::fs::read_to_string(&pro).unwrap();
    let needle = "\"min_track_width\": 0.1,";
    assert_eq!(text.matches(needle).count(), 1);
    std::fs::write(&pro, text.replace(needle, "\"min_track_width\": 0.12,")).unwrap();
    let (counts, required) = errors(&pcb, &[]);
    assert!(counts.get("dimension_trace_width").copied().unwrap_or(0) > 0);
    assert!(required["dimension_trace_width"]
        .iter()
        .all(|r| (*r - 0.12).abs() < 1e-9));
}

#[test]
fn unconditional_dru_rule_tightens_project_minimum() {
    let tmp = tempfile::tempdir().unwrap();
    let pcb = stage(tmp.path(), true);
    let dru = pcb.with_extension("kicad_dru");
    let mut text = std::fs::read_to_string(&dru).unwrap();
    text.push_str("\n(rule \"global hole spacing\"\n  (constraint hole_to_hole (min 0.6mm)))\n");
    std::fs::write(&dru, text).unwrap();
    let (counts, required) = errors(&pcb, &[]);
    assert!(counts.get("hole_to_hole_clearance").copied().unwrap_or(0) > 0);
    assert!(required["hole_to_hole_clearance"]
        .iter()
        .all(|r| (*r - 0.6).abs() < 1e-9));
}
