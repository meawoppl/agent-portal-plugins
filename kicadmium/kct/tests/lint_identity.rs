//! Content IDs must follow the offending geometry, not filenames or handles.
#[path = "lint_common/mod.rs"]
mod common;
use kct::lint::{board_file, lint, Config};

fn keys(source: &str, name: &str) -> Vec<String> {
    lint(source, name, common::config())
        .unwrap()
        .findings
        .into_iter()
        .map(|f| f.key)
        .collect()
}

#[test]
fn copied_board_and_regenerated_uuids_have_identical_ids() {
    let original = keys(common::BOARD, "working");
    let copy = common::BOARD
        .replace("trace-1", "new-track-a")
        .replace("trace-2", "new-track-b")
        .replace("fp-1", "new-footprint");
    assert_eq!(original, keys(&copy, "baseline-copy"));
    let without_ids = copy
        .replace("(uuid \"new-track-a\")", "")
        .replace("(uuid \"new-track-b\")", "")
        .replace("(uuid \"new-footprint\")", "");
    assert_eq!(original, keys(&without_ids, "no-uuids"));
}

#[test]
fn moving_one_subject_changes_only_its_id() {
    let before = lint(common::BOARD, "before", common::config()).unwrap();
    let moved = common::BOARD.replace("(start 10 20) (end 10.1 20)", "(start 11 20) (end 11.1 20)");
    let after = lint(&moved, "after", common::config()).unwrap();
    let delta = board_file::diff(&before, &after);
    assert_eq!(delta.unchanged, 2);
    assert_eq!(delta.new.len(), 1);
    assert_eq!(delta.fixed.len(), 1);
    assert_eq!(delta.new[0].subjects, ["trace-1"]);
    assert_ne!(delta.new[0].key, delta.fixed[0].key);
}

#[test]
fn intrinsic_geometry_ignores_nearby_copper() {
    let source = common::BOARD.replace(
        "(start 100 200) (end 100.1 200)",
        "(start 10 21) (end 10.1 21)",
    );
    let before = lint(&source, "board", common::config()).unwrap();
    let moved = source.replace("(start 10 21) (end 10.1 21)", "(start 10 22) (end 10.1 22)");
    let after = lint(&moved, "board", common::config()).unwrap();
    let key = |r: &kct::lint::Report| {
        r.findings
            .iter()
            .find(|f| f.subjects == ["trace-1"])
            .unwrap()
            .key
            .clone()
    };
    assert_eq!(key(&before), key(&after));
}

#[test]
fn distinct_missing_contracts_do_not_collapse() {
    let mut config = Config::default();
    config.enabled_only.insert("contract.pin_net".into());
    for reference in ["U1", "U2"] {
        config.pin_nets.push(kct::lint::PinNet {
            reference: reference.into(),
            pad: "1".into(),
            net: "N".into(),
        });
    }
    let report = lint(common::BOARD, "board", config).unwrap();
    assert_eq!(report.findings.len(), 2);
    assert_ne!(report.findings[0].key, report.findings[1].key);
}

#[test]
fn old_waiver_files_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("board.lint.json");
    std::fs::write(&path, r#"{"schema":2,"board_id":"board","exceptions":[]}"#).unwrap();
    let error = board_file::BoardLintFile::load_or_default(&path, "board").unwrap_err();
    assert!(error.to_string().contains("old waivers are unsupported"));
}

#[test]
fn via_attachment_id_ignores_distant_same_net_edits() {
    let source = common::BOARD.replace("(net 1 \"N\")", "(net 1 \"N\")\n(via (at 0 0) (size 0.6) (drill 0.3) (layers \"F.Cu\" \"B.Cu\") (net 1) (uuid \"v\"))");
    let mut config = Config::default();
    config.enabled_only.insert("via.low_attachment".into());
    let before = lint(&source, "before", config.clone()).unwrap();
    assert_eq!(before.findings.len(), 1);
    let after = lint(
        &source.replace(
            "(start 100 200) (end 100.1 200)",
            "(start 101 200) (end 101.1 200)",
        ),
        "after",
        config,
    )
    .unwrap();
    assert_eq!(before.findings[0].key, after.findings[0].key);
}
