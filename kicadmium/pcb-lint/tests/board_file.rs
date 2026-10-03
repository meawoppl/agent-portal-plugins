use pcb_lint::{
    Config,
    board_file::{
        self, Action, BoardLintFile, ExceptionStatus, PruneSet, clear, diff, lint_board, prune,
        upsert,
    },
    review,
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

fn config() -> Config {
    serde_json::from_str(
        r#"{"enabled_only":["trace.short_segment","bom.completeness"],
        "intent":{"assembly":{"fields":["MPN"],"excluded":[],"placements":[],
        "origin":{"x":0,"y":0},"angle_offsets":{},"tolerance_mm":0.01,"tolerance_deg":1}}}"#,
    )
    .unwrap()
}

fn file() -> BoardLintFile {
    let mut f = BoardLintFile::new("test-board");
    f.config = config();
    f
}

fn key_for(r: &pcb_lint::Report, subject: &str) -> String {
    r.findings
        .iter()
        .find(|f| f.subjects == [subject])
        .unwrap_or_else(|| panic!("no finding on {subject}"))
        .key
        .clone()
}

/// File with ignore exceptions on trace-1, trace-2 and the footprint.
fn reviewed() -> (BoardLintFile, pcb_lint::Report) {
    let mut f = file();
    let r = lint_board(BOARD, &f, 0).unwrap().report;
    assert_eq!(r.findings.len(), 3, "{:#?}", r.findings);
    for s in ["trace-1", "trace-2", "fp-1"] {
        upsert(
            &mut f,
            &r,
            &key_for(&r, s),
            Action::Ignore,
            "intentional",
            "agent",
            None,
        )
        .unwrap();
    }
    (f, r)
}

fn status_of(c: &board_file::Checked, key: &str) -> ExceptionStatus {
    c.audit.iter().find(|a| a.key == key).unwrap().status
}

#[test]
fn path_for_sits_next_to_board() {
    assert_eq!(
        BoardLintFile::path_for(std::path::Path::new("boards/esp.kicad_pcb")),
        std::path::Path::new("boards/esp.lint.json")
    );
}

#[test]
fn applied_exceptions_set_state_and_store_metadata() {
    let (f, r) = reviewed();
    let e = &f.exceptions[0];
    assert!(!e.rule.is_empty() && !e.subjects.is_empty() && !e.message.is_empty());
    assert!(e.at.is_some() && !e.severity.is_empty());
    let c = lint_board(BOARD, &f, 1).unwrap();
    assert!(c.audit.iter().all(|a| a.status == ExceptionStatus::Applied));
    assert!(c.report.findings.iter().all(|f| f.state == "ignored"));
    assert_eq!(c.report.review_audit.len(), 3);
    assert_eq!(c.stale().count(), 0);
    assert_eq!(r.findings.len(), 3);
}

#[test]
fn orphaned_when_track_deleted() {
    let (f, r) = reviewed();
    let src = BOARD.replace(
        "(segment (start 10 20) (end 10.1 20) (width 0.2) (layer \"F.Cu\") (net 1) (uuid \"trace-1\"))\n",
        "",
    );
    let c = lint_board(&src, &f, 1).unwrap();
    let a = c
        .audit
        .iter()
        .find(|a| a.key == key_for(&r, "trace-1"))
        .unwrap();
    assert_eq!(a.status, ExceptionStatus::Orphaned);
    assert!(a.detail.contains("trace-1"), "{}", a.detail);
}

#[test]
fn orphaned_when_footprint_deleted() {
    let (f, r) = reviewed();
    let src: String = BOARD
        .lines()
        .filter(|l| !l.contains("fp-1"))
        .collect::<Vec<_>>()
        .join("\n");
    let c = lint_board(&src, &f, 1).unwrap();
    assert_eq!(
        status_of(&c, &key_for(&r, "fp-1")),
        ExceptionStatus::Orphaned
    );
    assert_eq!(c.stale().count(), 1);
}

#[test]
fn resolved_when_fixed_but_present() {
    let (f, r) = reviewed();
    let src = BOARD.replace("(end 10.1 20)", "(end 11 20)");
    let c = lint_board(&src, &f, 1).unwrap();
    let a = c
        .audit
        .iter()
        .find(|a| a.key == key_for(&r, "trace-1"))
        .unwrap();
    assert_eq!(a.status, ExceptionStatus::Resolved);
}

#[test]
fn changed_when_nearby_geometry_changes() {
    let (f, r) = reviewed();
    let src = BOARD.replace(
        "(segment (start 100 200)",
        "(segment (start 12 21) (end 14 21) (width 0.2) (layer \"F.Cu\") (net 2) (uuid \"near\"))\n(segment (start 100 200)",
    );
    let c = lint_board(&src, &f, 1).unwrap();
    let k = key_for(&r, "trace-1");
    assert_eq!(status_of(&c, &k), ExceptionStatus::Changed);
    let finding = c.report.findings.iter().find(|x| x.key == k).unwrap();
    assert_eq!(finding.state, "open");
}

#[test]
fn unverified_when_rule_not_evaluated() {
    let (mut f, r) = reviewed();
    f.config.disabled.insert("trace.short_segment".into());
    // Even with the geometry gone, a skipped rule must never look like a fix.
    let src = BOARD.replace("(uuid \"trace-1\")", "(uuid \"trace-9\")");
    let c = lint_board(&src, &f, 1).unwrap();
    let a = c
        .audit
        .iter()
        .find(|a| a.key == key_for(&r, "trace-1"))
        .unwrap();
    assert_eq!(a.status, ExceptionStatus::Unverified);
    assert_eq!(a.detail, "disabled");
    let removed = prune(
        &mut f,
        &c.audit,
        PruneSet {
            orphaned: true,
            resolved: true,
            expired: true,
            changed: true,
        },
    );
    assert!(removed.iter().all(|e| e.rule != "trace.short_segment"));
}

#[test]
fn expired_exceptions() {
    let (mut f, r) = reviewed();
    let k = key_for(&r, "trace-2");
    f.exceptions
        .iter_mut()
        .find(|e| e.key == k)
        .unwrap()
        .expires_at = Some(5);
    let c = lint_board(BOARD, &f, 10).unwrap();
    assert_eq!(status_of(&c, &k), ExceptionStatus::Expired);
    assert_eq!(
        c.report.findings.iter().find(|x| x.key == k).unwrap().state,
        "open"
    );
}

#[test]
fn prune_removes_only_selected_statuses() {
    let (mut f, r) = reviewed();
    // trace-1 orphaned, trace-2 changed.
    let src = BOARD
        .replace(
            "(segment (start 10 20) (end 10.1 20) (width 0.2) (layer \"F.Cu\") (net 1) (uuid \"trace-1\"))\n",
            "",
        )
        .replace(
            "(segment (start 100 200)",
            "(segment (start 102 201) (end 104 201) (width 0.2) (layer \"F.Cu\") (net 2) (uuid \"near\"))\n(segment (start 100 200)",
        );
    let c = lint_board(&src, &f, 1).unwrap();
    assert_eq!(
        status_of(&c, &key_for(&r, "trace-1")),
        ExceptionStatus::Orphaned
    );
    assert_eq!(
        status_of(&c, &key_for(&r, "trace-2")),
        ExceptionStatus::Changed
    );
    let removed = prune(&mut f, &c.audit, PruneSet::default());
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].key, key_for(&r, "trace-1"));
    assert_eq!(f.exceptions.len(), 2);
    let removed = prune(
        &mut f,
        &c.audit,
        PruneSet {
            orphaned: false,
            resolved: false,
            expired: false,
            changed: true,
        },
    );
    // fp-1 evidence also shifts (board-wide manufacturing context), so it is changed too.
    let changed: Vec<_> = c
        .audit
        .iter()
        .filter(|a| a.status == ExceptionStatus::Changed)
        .map(|a| a.key.clone())
        .collect();
    assert!(changed.contains(&key_for(&r, "trace-2")));
    assert_eq!(
        removed.iter().map(|e| e.key.clone()).collect::<Vec<_>>(),
        changed
    );
    assert_eq!(f.exceptions.len(), 2 - changed.len());
    // Applied exceptions are never pruned, even with every flag set.
    let mut g = reviewed().0;
    let all = lint_board(BOARD, &g, 1).unwrap();
    let set = PruneSet {
        orphaned: true,
        resolved: true,
        expired: true,
        changed: true,
    };
    assert!(prune(&mut g, &all.audit, set).is_empty());
}

#[test]
fn upsert_validates_and_replaces() {
    let (mut f, r) = reviewed();
    let k = key_for(&r, "trace-1");
    assert!(upsert(&mut f, &r, &k, Action::Flag, " ", "agent", None).is_err());
    assert!(upsert(&mut f, &r, "nonexistent", Action::Flag, "x", "agent", None).is_err());
    assert!(upsert(&mut f, &r, &k, Action::Flag, "x", "agent", Some(1)).is_err());
    // Unique prefix resolves to the full key and replaces the old entry.
    upsert(&mut f, &r, &k[..12], Action::Flag, "watch", "agent", None).unwrap();
    assert_eq!(f.exceptions.len(), 3);
    let e = f.exceptions.iter().find(|e| e.key == k).unwrap();
    assert_eq!((e.action, e.reason.as_str()), (Action::Flag, "watch"));
    let mut other = BoardLintFile::new("other-board");
    assert!(upsert(&mut other, &r, &k, Action::Flag, "x", "agent", None).is_err());
    // Unstable identities can be flagged but never ignored.
    let mut unstable = r.findings[0].clone();
    unstable.key = "f".repeat(64);
    unstable.stable_identity = false;
    unstable.subjects = vec!["fallback:abc".into()];
    let mut r2 = lint_board(BOARD, &file(), 0).unwrap().report;
    r2.findings.push(unstable);
    let mut g = file();
    let k2 = "f".repeat(64);
    assert!(upsert(&mut g, &r2, &k2, Action::Ignore, "x", "agent", None).is_err());
    upsert(&mut g, &r2, &k2, Action::Flag, "x", "agent", None).unwrap();
}

#[test]
fn clear_removes_by_key() {
    let (mut f, r) = reviewed();
    assert!(clear(&mut f, &key_for(&r, "trace-1")));
    assert!(!clear(&mut f, &key_for(&r, "trace-1")));
    assert!(!clear(&mut f, "nope"));
    assert_eq!(f.exceptions.len(), 2);
}

#[test]
fn save_load_round_trip_and_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("b.lint.json");
    let missing = BoardLintFile::load_or_default(&path, "fresh").unwrap();
    assert_eq!((missing.schema, missing.board_id.as_str()), (2, "fresh"));
    let (f, _) = reviewed();
    f.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    // Only non-default config fields are written.
    assert!(raw["config"].get("min_trace_mm").is_none());
    assert!(raw["config"].get("enabled_only").is_some());
    let back = BoardLintFile::load_or_default(&path, "ignored").unwrap();
    assert_eq!(back.board_id, "test-board");
    assert_eq!(back.exceptions, f.exceptions);
    assert_eq!(
        serde_json::to_value(&back.config).unwrap(),
        serde_json::to_value(&f.config).unwrap()
    );
    // A held lock blocks save and update; nothing is stolen or written.
    let lock = dir.path().join("b.lint.json.lock");
    std::fs::write(&lock, "pid=1").unwrap();
    assert!(f.save(&path).is_err());
    assert!(BoardLintFile::update(&path, "x", |_| Ok(())).is_err());
    assert!(lock.exists());
    std::fs::remove_file(&lock).unwrap();
    let removed = BoardLintFile::update(&path, "x", |f| {
        let k = f.exceptions[0].key.clone();
        Ok(clear(f, &k))
    })
    .unwrap();
    assert!(removed);
    assert!(!lock.exists());
    assert_eq!(
        BoardLintFile::load_or_default(&path, "x")
            .unwrap()
            .exceptions
            .len(),
        2
    );
    // A failing mutation leaves the file untouched.
    assert!(
        BoardLintFile::update(&path, "x", |f| -> anyhow::Result<()> {
            f.exceptions.clear();
            anyhow::bail!("nope")
        })
        .is_err()
    );
    assert_eq!(
        BoardLintFile::load_or_default(&path, "x")
            .unwrap()
            .exceptions
            .len(),
        2
    );
}

#[test]
fn load_rejects_unknown_fields_and_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("b.lint.json");
    std::fs::write(&path, r#"{"schema":2,"board_id":"b","bogus":1}"#).unwrap();
    assert!(BoardLintFile::load_or_default(&path, "b").is_err());
    std::fs::write(&path, r#"{"schema":1,"board_id":"b"}"#).unwrap();
    assert!(BoardLintFile::load_or_default(&path, "b").is_err());
    std::fs::write(
        &path,
        r#"{"schema":2,"board_id":"b","config":{"min_trace_mm":0.2}}"#,
    )
    .unwrap();
    let f = BoardLintFile::load_or_default(&path, "b").unwrap();
    assert_eq!(f.config.min_trace_mm, 0.2);
    assert_eq!(f.config.min_drill_mm, Config::default().min_drill_mm);
}

#[test]
fn migrate_round_trip() {
    let (f, r) = reviewed();
    // Express the reviewed file as a schema-1 ledger, including another board's entry.
    let mut decisions: Vec<review::Decision> = f
        .exceptions
        .iter()
        .map(|e| review::Decision {
            board_id: "test-board".into(),
            key: e.key.clone(),
            evidence: e.evidence.clone(),
            action: e.action.as_str().into(),
            reason: e.reason.clone(),
            reviewer: e.reviewer.clone(),
            reviewed_at: e.reviewed_at,
            expires_at: e.expires_at,
        })
        .collect();
    let mut foreign = decisions[0].clone();
    foreign.board_id = "other".into();
    decisions.push(foreign);
    let ledger = review::Ledger {
        schema: 1,
        decisions,
    };
    let mut m = BoardLintFile::migrate(Some(config()), Some(ledger), "test-board");
    assert_eq!(m.schema, 2);
    assert_eq!(m.exceptions.len(), 3);
    assert!(m.exceptions.iter().all(|e| e.rule.is_empty()));
    // Same decisions apply exactly as before.
    let c = lint_board(BOARD, &m, 1).unwrap();
    assert!(c.audit.iter().all(|a| a.status == ExceptionStatus::Applied));
    // Backfill restores card metadata from the live report.
    assert_eq!(board_file::backfill(&mut m, &r), 3);
    assert_eq!(
        m.exceptions,
        f.exceptions
            .iter()
            .map(|e| {
                let mut e = e.clone();
                e.reviewed_at = m
                    .exceptions
                    .iter()
                    .find(|x| x.key == e.key)
                    .unwrap()
                    .reviewed_at;
                e
            })
            .collect::<Vec<_>>()
    );
    // Saved and reloaded as schema 2.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.lint.json");
    m.save(&path).unwrap();
    let back = BoardLintFile::load_or_default(&path, "x").unwrap();
    assert_eq!(back.exceptions, m.exceptions);
    // Migrating nothing gives defaults.
    let empty = BoardLintFile::migrate(None, None, "b");
    assert!(empty.exceptions.is_empty());
    assert_eq!(empty.config.min_trace_mm, Config::default().min_trace_mm);
}

#[test]
fn migrated_unraised_exceptions_are_unverified() {
    let ledger = review::Ledger {
        schema: 1,
        decisions: vec![review::Decision {
            board_id: "test-board".into(),
            key: "0".repeat(64),
            evidence: "e".into(),
            action: "ignore".into(),
            reason: "r".into(),
            reviewer: "me".into(),
            reviewed_at: 1,
            expires_at: None,
        }],
    };
    let m = BoardLintFile::migrate(Some(config()), Some(ledger), "test-board");
    let c = lint_board(BOARD, &m, 1).unwrap();
    assert_eq!(c.audit[0].status, ExceptionStatus::Unverified);
}

#[test]
fn diff_by_key() {
    let f = file();
    let before = lint_board(BOARD, &f, 0).unwrap().report;
    let after_src = BOARD
        .replace("(end 10.1 20)", "(end 11 20)")
        .replace(
            "(segment (start 100 200)",
            "(segment (start 30 30) (end 30.05 30) (width 0.2) (layer \"F.Cu\") (net 2) (uuid \"new-stub\"))\n(segment (start 100 200)",
        );
    let after = lint_board(&after_src, &f, 0).unwrap().report;
    let d = diff(&before, &after);
    assert_eq!(d.new.len(), 1);
    assert_eq!(d.new[0].subjects, ["new-stub"]);
    assert_eq!(d.fixed.len(), 1);
    assert_eq!(d.fixed[0].subjects, ["trace-1"]);
    assert_eq!(d.unchanged, 2);
    let same = diff(&before, &before);
    assert!(same.new.is_empty() && same.fixed.is_empty());
    assert_eq!(same.unchanged, 3);
}
