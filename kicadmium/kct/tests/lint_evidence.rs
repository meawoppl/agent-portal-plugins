//! Engine-3 evidence: unrelated distant edits keep reviews applied; nearby
//! geometry and the rule's own config reopen them.
#[path = "lint_common/mod.rs"]
mod common;
use kct::lint::{
    board_file::{lint_board, upsert, Action, BoardLintFile, ExceptionStatus},
    Config,
};

fn waive_all(src: &str, mut f: BoardLintFile) -> BoardLintFile {
    let r = lint_board(src, &f, 0).unwrap().report;
    for x in &r.findings {
        if x.stable_identity {
            upsert(&mut f, &r, &x.key, Action::Ignore, "ok", "agent", None).unwrap();
        }
    }
    f
}

fn statuses(src: &str, f: &BoardLintFile) -> Vec<(String, Vec<String>, ExceptionStatus)> {
    let c = lint_board(src, f, 1).unwrap();
    f.exceptions
        .iter()
        .map(|e| {
            let a = c.audit.iter().find(|a| a.key == e.key).unwrap();
            (e.rule.clone(), e.subjects.clone(), a.status)
        })
        .collect()
}

fn inline() -> BoardLintFile {
    let mut f = BoardLintFile::new("ev");
    f.config = common::config();
    waive_all(common::BOARD, f)
}

#[test]
fn distant_same_net_move_keeps_reviews_applied() {
    let f = inline();
    assert_eq!(f.exceptions.len(), 3);
    // trace-2 shares net N with trace-1 but is ~200 mm away.
    let moved = common::BOARD.replace(
        "(start 100 200) (end 100.1 200)",
        "(start 101 201) (end 101.1 201)",
    );
    for (rule, subjects, s) in statuses(&moved, &f) {
        let expect = if subjects == ["trace-2"] {
            ExceptionStatus::Changed
        } else {
            ExceptionStatus::Applied
        };
        assert_eq!(s, expect, "{rule} {subjects:?}");
    }
}

#[test]
fn distant_track_delete_keeps_reviews_applied() {
    let f = inline();
    let deleted = common::BOARD.replace(
        "(segment (start 100 200) (end 100.1 200) (width 0.2) (layer \"B.Cu\") (net 1) (uuid \"trace-2\"))\n",
        "",
    );
    for (rule, subjects, s) in statuses(&deleted, &f) {
        if subjects == ["trace-2"] {
            assert_eq!(s, ExceptionStatus::Orphaned);
        } else {
            assert_eq!(s, ExceptionStatus::Applied, "{rule} {subjects:?}");
        }
    }
}

#[test]
fn nearby_geometry_reopens() {
    let f = inline();
    let near = common::BOARD.replace(
        "(segment (start 100 200)",
        "(segment (start 11 22) (end 13 22) (width 0.2) (layer \"F.Cu\") (net 2) (uuid \"near\"))\n(segment (start 100 200)",
    );
    for (rule, subjects, s) in statuses(&near, &f) {
        let expect = if subjects == ["trace-1"] {
            ExceptionStatus::Changed
        } else {
            ExceptionStatus::Applied
        };
        assert_eq!(s, expect, "{rule} {subjects:?}");
    }
}

#[test]
fn own_threshold_reopens_unrelated_config_does_not() {
    let f = inline();
    let mut own = f.clone();
    own.config.short_segment_mm = 0.25;
    for (rule, _, s) in statuses(common::BOARD, &own) {
        let expect = if rule == "trace.short_segment" {
            ExceptionStatus::Changed
        } else {
            ExceptionStatus::Applied
        };
        assert_eq!(s, expect, "{rule}");
    }
    let mut unrelated = f.clone();
    unrelated.config.min_drill_mm = 0.3;
    unrelated.config.angle_tolerance_deg = 2.;
    unrelated.config.intent.labels = vec![];
    unrelated.config.disabled.insert("via.cluster".to_owned());
    assert!(statuses(common::BOARD, &unrelated)
        .iter()
        .all(|(_, _, s)| *s == ExceptionStatus::Applied));
    // The BOM rule's own config slice reopens only it.
    let mut bom = f;
    bom.config
        .intent
        .assembly
        .as_mut()
        .unwrap()
        .fields
        .push("Manufacturer".into());
    for (rule, _, s) in statuses(common::BOARD, &bom) {
        let expect = if rule == "bom.completeness" {
            ExceptionStatus::Changed
        } else {
            ExceptionStatus::Applied
        };
        assert_eq!(s, expect, "{rule}");
    }
}

#[test]
fn fixture_distant_delete_keeps_unrelated_reviews() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/lint/");
    let src = std::fs::read_to_string(format!("{dir}advanced-bad.kicad_pcb")).unwrap();
    let config: Config =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}advanced-bad.json")).unwrap())
            .unwrap();
    let mut f = BoardLintFile::new("advanced-bad");
    f.config = config;
    let f = waive_all(&src, f);
    // rev1 at (70,10)-(75,10) on net REV; J1 sits at (20,30).
    let edited: String = src
        .lines()
        .filter(|l| !l.contains("(uuid \"rev1\")"))
        .collect::<Vec<_>>()
        .join("\n");
    let c = lint_board(&edited, &f, 1).unwrap();
    let status = |rule: &str, subject: &str| {
        let e = f
            .exceptions
            .iter()
            .find(|e| e.rule == rule && e.subjects.iter().any(|s| s == subject))
            .unwrap();
        c.audit.iter().find(|a| a.key == e.key).unwrap().status
    };
    assert_eq!(status("bom.completeness", "J1"), ExceptionStatus::Applied);
    assert_eq!(status("placement.rotation", "U1"), ExceptionStatus::Applied);
    assert_eq!(status("silk.collisions", "txt"), ExceptionStatus::Applied);
    assert_eq!(status("trace.open_end", "rev1"), ExceptionStatus::Orphaned);
    let changed: Vec<_> = c
        .audit
        .iter()
        .filter(|a| a.status == ExceptionStatus::Changed)
        .collect();
    // Only the REV net's neighbourhood reopens.
    for a in &changed {
        let e = f.exceptions.iter().find(|e| e.key == a.key).unwrap();
        assert!(
            e.subjects.iter().all(|s| s.starts_with("rev")),
            "{} {:?}",
            e.rule,
            e.subjects
        );
    }
    let applied = c
        .audit
        .iter()
        .filter(|a| a.status == ExceptionStatus::Applied)
        .count();
    assert!(applied > 100, "{applied} applied of {}", c.audit.len());
}
