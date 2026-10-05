#[path = "lint_common/mod.rs"]
mod common;
use kct::lint::{
    board_file::{lint_board, BoardLintFile},
    ci::{self, CiSummary, FailOn},
};
use std::path::Path;

#[test]
fn summary_counts_and_pass_thresholds() {
    let (_, c) = common::mixed();
    let s = CiSummary::from(&c);
    // trace-1 ignored (applied); trace-2 (changed) and fp-1 stay open.
    assert_eq!(s.ignored, 1);
    assert_eq!(s.open("warning") + s.open("error") + s.open("info"), 2);
    assert_eq!(s.exceptions_by_status["orphaned"], 1);
    assert_eq!(s.exceptions_by_status["unverified"], 1);
    assert_eq!(s.stale_exceptions.len(), 3);
    assert!(s.passed(FailOn::Never));
    let any_warning = s.open("warning") + s.open("error") > 0;
    assert_eq!(s.passed(FailOn::Warning), !any_warning);
    assert_eq!(s.passed(FailOn::Error), s.open("error") == 0);
    assert_eq!("error".parse::<FailOn>().unwrap(), FailOn::Error);
    assert!("bogus".parse::<FailOn>().is_err());
}

#[test]
fn summary_markdown_contents() {
    let (f, c) = common::mixed();
    let md = ci::summary_markdown(
        &c,
        &f,
        &[
            ("findings", "findings.html"),
            ("exceptions", "exceptions.html"),
        ],
    );
    assert!(md.contains("### kct lint · `mixed-board`"));
    assert!(md.contains("| Open findings | error | warning | info | total |"));
    assert!(md.contains(
        "| Exceptions | applied | changed | resolved | orphaned | expired | unverified | total |"
    ));
    assert!(md.contains("| count | 1 | 1 | 1 | 1 | 1 | 1 | 6 |"));
    assert!(md.contains("**3 stale exceptions** (run `kct lint prune`)"));
    for st in ["orphaned", "resolved", "expired"] {
        assert!(
            md.contains(&format!("| {st} | `trace.short_segment` |")),
            "{md}"
        );
    }
    assert!(md.contains("gone-track"));
    assert!(md.contains("[findings](findings.html) · [exceptions](exceptions.html)"));
    let clean = BoardLintFile::new("clean");
    let cc = lint_board(common::BOARD, &clean, 0).unwrap();
    assert!(ci::summary_markdown(&cc, &clean, &[]).contains("No stale exceptions."));
}

#[test]
fn sarif_shape() {
    let (_, c) = common::mixed();
    let v = ci::sarif(&[(Path::new("boards/mixed.kicad_pcb"), &c)]);
    let text = serde_json::to_string(&v).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], "2.1.0");
    let run = &v["runs"][0];
    assert_eq!(run["tool"]["driver"]["name"], "kct-lint");
    let results = run["results"].as_array().unwrap();
    assert_eq!(results.len(), c.report.findings.len());
    let rules: Vec<_> = run["tool"]["driver"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect();
    for (r, f) in results.iter().zip(&c.report.findings) {
        assert_eq!(r["ruleId"], f.rule);
        assert!(rules.contains(&f.rule));
        assert_eq!(
            r["level"],
            match f.severity.as_str() {
                "error" => "error",
                "warning" => "warning",
                _ => "note",
            }
        );
        let loc = &r["locations"][0];
        assert_eq!(
            loc["physicalLocation"]["artifactLocation"]["uri"],
            "boards/mixed.kicad_pcb"
        );
        assert!(loc["logicalLocations"][0]["name"]
            .as_str()
            .unwrap()
            .ends_with(" mm"));
        assert_eq!(r["fingerprints"]["pcbLintKey/v1"], f.key);
        assert_eq!(r["suppressions"].is_array(), f.state == "ignored");
    }
}

#[test]
fn bundle_writes_all_artifacts() {
    let (f, c) = common::mixed();
    let dir = tempfile::tempdir().unwrap();
    let s = ci::write_bundle(
        dir.path(),
        Path::new("mixed.kicad_pcb"),
        common::BOARD,
        &c,
        &f,
    )
    .unwrap();
    assert_eq!(s.stale_exceptions.len(), 3);
    for name in [
        "report.json",
        "findings.html",
        "exceptions.html",
        "summary.md",
        "findings.sarif",
    ] {
        assert!(dir.path().join(name).is_file(), "{name}");
    }
    let report: kct::lint::Report =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("report.json")).unwrap())
            .unwrap();
    assert_eq!(report.board_id, "mixed-board");
}
