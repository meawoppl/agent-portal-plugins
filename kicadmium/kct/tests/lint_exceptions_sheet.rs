#[path = "lint_common/mod.rs"]
mod common;
use kct::lint::{
    board_file::{BoardLintFile, ExceptionStatus},
    contact_sheet::{self, ORPHANED_NOTE},
};

#[test]
fn mixed_fixture_covers_every_status() {
    let (_, c) = common::mixed();
    for s in ExceptionStatus::ALL {
        assert_eq!(
            c.audit.iter().filter(|a| a.status == s).count(),
            1,
            "{s}: {:#?}",
            c.audit
        );
    }
}

#[test]
fn exceptions_sheet_has_every_card_and_status() {
    let (f, c) = common::mixed();
    let html = contact_sheet::render_exceptions(common::BOARD, &c.report, &f, &c.audit).unwrap();
    assert_eq!(html.matches("<article ").count(), f.exceptions.len());
    for e in &f.exceptions {
        assert!(html.contains(&format!("data-key=\"{}\"", e.key)));
    }
    for s in ExceptionStatus::ALL {
        assert!(
            html.contains(&format!("data-status=\"{s}\"")),
            "missing card status {s}"
        );
        assert!(html.contains(&format!("1 {s}</span>")), "missing count {s}");
    }
    // Orphaned card: stored subjects, prune hint, no crop.
    let orphan = html
        .split("<article ")
        .find(|c| c.starts_with("data-status=\"orphaned\""))
        .unwrap();
    assert!(orphan.contains(ORPHANED_NOTE));
    assert!(orphan.contains("<code>gone-track</code>"));
    assert!(!orphan.contains("<svg"));
    // Live geometry gets a highlighted crop.
    let applied = html
        .split("<article ")
        .find(|c| c.starts_with("data-status=\"applied\""))
        .unwrap();
    assert!(applied.contains("<svg class=\"drawing\""));
    assert!(applied.contains("highlight"));
    assert!(applied.contains("accepted stub") && applied.contains("agent"));
    // User text is escaped, review metadata present, no unresolved tokens.
    assert!(html.contains("reason &lt;a&gt;"));
    assert!(!html.contains("reason <a>"));
    assert!(html.contains("2026-09-21")); // reviewed_at 1_790_000_000
    assert!(html.contains("1970-01-01")); // expiry of the expired exception
    assert!(html.contains("stale exceptions"));
    assert!(!html.contains("%%"));
}

#[test]
fn empty_exceptions_sheet() {
    let f = BoardLintFile::new("x");
    let c = kct::lint::board_file::lint_board(common::BOARD, &f, 0).unwrap();
    let html = contact_sheet::render_exceptions(common::BOARD, &c.report, &f, &c.audit).unwrap();
    assert!(html.contains("No exceptions recorded"));
    assert!(contact_sheet::render_exceptions("(kicad_pcb)", &c.report, &f, &c.audit).is_err());
}

#[test]
fn dates() {
    assert_eq!(contact_sheet::date(0), "1970-01-01");
    assert_eq!(contact_sheet::date(951_782_400), "2000-02-29");
    assert_eq!(contact_sheet::date(1_790_000_000), "2026-09-21");
}
