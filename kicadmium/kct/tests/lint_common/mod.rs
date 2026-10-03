//! Shared fixture: a small board and a lint file whose exceptions cover every
//! audit status.
#![allow(dead_code)]
use kct::lint::{
    board_file::{lint_board, upsert, Action, BoardLintFile, Checked, Exception},
    model::Point,
    Config,
};

pub const BOARD: &str = r#"(kicad_pcb
(version 20240108) (generator "test")
(layers (0 "F.Cu" signal) (31 "B.Cu" signal))
(net 1 "N")
(net 2 "M")
(footprint "Test:R" (layer "F.Cu") (at 50 50 0) (uuid "fp-1") (property "Reference" "R1") (property "Value" "10k"))
(segment (start 10 20) (end 10.1 20) (width 0.2) (layer "F.Cu") (net 1) (uuid "trace-1"))
(segment (start 100 200) (end 100.1 200) (width 0.2) (layer "B.Cu") (net 1) (uuid "trace-2"))
)"#;

pub fn config() -> Config {
    serde_json::from_str(
        r#"{"enabled_only":["trace.short_segment","bom.completeness"],
        "intent":{"assembly":{"fields":["MPN"],"excluded":[],"placements":[],
        "origin":{"x":0,"y":0},"angle_offsets":{},"tolerance_mm":0.01,"tolerance_deg":1}}}"#,
    )
    .unwrap()
}

fn fabricated(key: char, rule: &str, subjects: &[&str], expires: Option<u64>) -> Exception {
    Exception {
        key: key.to_string().repeat(64),
        rule: rule.into(),
        subjects: subjects.iter().map(|s| s.to_string()).collect(),
        evidence: "old".into(),
        action: Action::Ignore,
        reason: format!("reason <{key}>"),
        reviewer: "agent".into(),
        reviewed_at: 1_790_000_000,
        expires_at: expires,
        severity: "warning".into(),
        message: format!("stored message {key}"),
        at: Some(Point { x: 10., y: 20. }),
        nets: vec!["N".into()],
    }
}

/// One exception per status: applied, changed, orphaned, resolved, expired, unverified.
pub fn mixed() -> (BoardLintFile, Checked) {
    let mut f = BoardLintFile::new("mixed-board");
    f.config = config();
    let r = lint_board(BOARD, &f, 0).unwrap().report;
    let key = |s: &str| {
        r.findings
            .iter()
            .find(|f| f.subjects == [s])
            .unwrap()
            .key
            .clone()
    };
    upsert(
        &mut f,
        &r,
        &key("trace-1"),
        Action::Ignore,
        "accepted stub",
        "agent",
        None,
    )
    .unwrap();
    upsert(
        &mut f,
        &r,
        &key("trace-2"),
        Action::Flag,
        "watch this",
        "agent",
        None,
    )
    .unwrap();
    f.exceptions
        .iter_mut()
        .find(|e| e.key == key("trace-2"))
        .unwrap()
        .evidence = "stale".into();
    f.exceptions.push(fabricated(
        'a',
        "trace.short_segment",
        &["gone-track"],
        None,
    ));
    f.exceptions
        .push(fabricated('b', "trace.short_segment", &["fp-1"], None));
    f.exceptions.push(fabricated(
        'c',
        "trace.short_segment",
        &["trace-2"],
        Some(1),
    ));
    f.exceptions
        .push(fabricated('d', "copper.island", &["trace-1"], None));
    f.exceptions.sort_by(|a, b| a.key.cmp(&b.key));
    let c = lint_board(BOARD, &f, 1_800_000_000).unwrap();
    (f, c)
}
