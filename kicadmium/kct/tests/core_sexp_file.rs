//! Ports of upstream tests/test_atomic_write.py, test_file_formats.py,
//! test_write_verification.py (verify_pcb_write), test_kicad_lock.py, and
//! test_kicad_format_version.py (constants).

use std::path::Path;
use std::sync::Mutex;

use kct::core::atomic_write::atomic_write_text;
use kct::core::kicad_lock::{check_kicad_lock, probe_kicad_lock, KiCadLockError, LOCK_POLICY_ENV};
use kct::core::sexp_file::{
    load_design_rules, load_footprint, load_pcb, load_schematic, save_design_rules, save_footprint,
    save_pcb, save_schematic, verify_pcb_write, WriteVerificationError,
};
use kct::core::version::{
    KICAD_BOARD_FORMAT_VERSION, KICAD_GENERATOR_VERSION, KICAD_SCH_FORMAT_VERSION,
    KICAD_SYM_FORMAT_VERSION,
};
use kct::exceptions::{ErrorKind, KiCadToolsError};
use kct::{parse, SExp};

/// Serializes tests that touch `KCT_KICAD_LOCK_POLICY`.
static ENV: Mutex<()> = Mutex::new(());

fn with_policy<T>(policy: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
    match policy {
        Some(p) => std::env::set_var(LOCK_POLICY_ENV, p),
        None => std::env::remove_var(LOCK_POLICY_ENV),
    }
    let out = f();
    std::env::remove_var(LOCK_POLICY_ENV);
    out
}

fn kind(err: &anyhow::Error) -> Option<ErrorKind> {
    err.downcast_ref::<KiCadToolsError>().map(|e| e.kind)
}

fn tmp_sibling(p: &Path) -> std::path::PathBuf {
    p.with_file_name(format!("{}.tmp", p.file_name().unwrap().to_string_lossy()))
}

const MINIMAL_FOOTPRINT: &str = r#"(footprint "R_0402_1005Metric"
  (version 20240108)
  (generator "test")
  (generator_version "8.0")
  (layer "F.Cu")
  (descr "Resistor SMD 0402 (1005 Metric)")
  (tags "resistor 0402")
  (property "Reference" "REF**" (at 0 -1.1 0) (layer "F.SilkS") (uuid "ref-uuid"))
  (property "Value" "R_0402_1005Metric" (at 0 1.1 0) (layer "F.Fab") (uuid "val-uuid"))
  (fp_line (start -0.153641 -0.38) (end 0.153641 -0.38) (stroke (width 0.12) (type solid)) (layer "F.SilkS"))
  (fp_line (start -0.153641 0.38) (end 0.153641 0.38) (stroke (width 0.12) (type solid)) (layer "F.SilkS"))
  (pad "1" smd roundrect (at -0.51 0) (size 0.54 0.64) (layers "F.Cu" "F.Paste" "F.Mask") (roundrect_rratio 0.25))
  (pad "2" smd roundrect (at 0.51 0) (size 0.54 0.64) (layers "F.Cu" "F.Paste" "F.Mask") (roundrect_rratio 0.25))
)
"#;

const MINIMAL_FOOTPRINT_KICAD5: &str = r#"(module "R_0402_1005Metric"
  (layer "F.Cu")
  (fp_text reference "REF**" (at 0 -1.1) (layer "F.SilkS"))
  (pad 1 smd roundrect (at -0.51 0) (size 0.54 0.64) (layers "F.Cu" "F.Paste" "F.Mask"))
)
"#;

const MINIMAL_DESIGN_RULES: &str = r#"(version 1)
(rule "Trace Width"
  (constraint track_width (min 0.127mm)))
(rule "Clearance"
  (constraint clearance (min 0.127mm)))
(rule "Via Drill"
  (constraint hole_size (min 0.3mm)))
(rule "Via Diameter"
  (constraint via_diameter (min 0.6mm)))
(rule "Annular Ring"
  (constraint annular_width (min 0.15mm)))
(rule "Copper to Edge"
  (constraint edge_clearance (min 0.3mm)))
"#;

const MINIMAL_PCB: &str = r#"(kicad_pcb
  (version 20240108)
  (generator "test")
  (general (thickness 1.6))
  (layers
    (0 "F.Cu" signal)
    (31 "B.Cu" signal)
    (44 "Edge.Cuts" user)
  )
  (net 0 "")
  (net 1 "GND")
)"#;

// ------------------------------------------------------------------ atomic write

#[test]
fn atomic_write_text_writes_without_litter() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.txt");
    atomic_write_text(&dest, "hello world").unwrap();
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello world");
    assert!(!tmp_sibling(&dest).exists());
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
}

#[test]
fn atomic_write_failure_leaves_destination_unchanged() {
    // Rename onto a non-empty directory fails: the old content survives.
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("occupied");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("keep"), "sentinel").unwrap();
    assert!(atomic_write_text(&dest, "new content").is_err());
    assert_eq!(
        std::fs::read_to_string(dest.join("keep")).unwrap(),
        "sentinel"
    );
}

// ------------------------------------------------------------------ writers

#[test]
fn hardened_writers_roundtrip() {
    with_policy(Some("ignore"), || {
        let tmp = tempfile::tempdir().unwrap();
        let pcb = tmp.path().join("board.kicad_pcb");
        save_pcb(&parse(MINIMAL_PCB).unwrap(), &pcb).unwrap();
        assert!(std::fs::read_to_string(&pcb).unwrap().contains("kicad_pcb"));
        assert!(!tmp_sibling(&pcb).exists());
        assert!(load_pcb(&pcb).unwrap().has_tag("kicad_pcb"));

        let sch = tmp.path().join("board.kicad_sch");
        save_schematic(&parse("(kicad_sch (version 20240108))").unwrap(), &sch).unwrap();
        assert!(load_schematic(&sch).unwrap().has_tag("kicad_sch"));
    });
}

#[test]
fn writers_reject_wrong_root() {
    let tmp = tempfile::tempdir().unwrap();
    let e = save_pcb(
        &parse("(kicad_sch)").unwrap(),
        tmp.path().join("x.kicad_pcb"),
    )
    .unwrap_err();
    assert_eq!(kind(&e), Some(ErrorKind::FileFormat));
    let e =
        save_footprint(&SExp::list("kicad_sch", []), tmp.path().join("w.kicad_mod")).unwrap_err();
    assert_eq!(kind(&e), Some(ErrorKind::FileFormat));
    let e = save_design_rules(
        &SExp::list("footprint", [SExp::atom("test")]),
        tmp.path().join("w.kicad_dru"),
    )
    .unwrap_err();
    assert_eq!(kind(&e), Some(ErrorKind::FileFormat));
}

// ------------------------------------------------------------------ footprints

#[test]
fn load_footprints_both_formats() {
    let tmp = tempfile::tempdir().unwrap();
    let f6 = tmp.path().join("R.kicad_mod");
    std::fs::write(&f6, MINIMAL_FOOTPRINT).unwrap();
    let sexp = load_footprint(&f6).unwrap();
    assert!(sexp.has_tag("footprint"));
    assert_eq!(sexp.string_at(0), Some("R_0402_1005Metric"));
    assert_eq!(sexp.find_children("pad").len(), 2);

    let f5 = tmp.path().join("R5.kicad_mod");
    std::fs::write(&f5, MINIMAL_FOOTPRINT_KICAD5).unwrap();
    let sexp = load_footprint(&f5).unwrap();
    assert!(sexp.has_tag("module"));
    assert_eq!(sexp.string_at(0), Some("R_0402_1005Metric"));

    let e = load_footprint(tmp.path().join("nonexistent.kicad_mod")).unwrap_err();
    assert_eq!(kind(&e), Some(ErrorKind::FileNotFound));

    let wrong = tmp.path().join("wrong.kicad_mod");
    std::fs::write(&wrong, "(kicad_sch (version 1))").unwrap();
    let e = load_footprint(&wrong).unwrap_err();
    assert_eq!(kind(&e), Some(ErrorKind::FileFormat));
    assert!(e.to_string().contains("Not a KiCad footprint"));
}

#[test]
fn save_footprint_roundtrip() {
    with_policy(Some("ignore"), || {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("R.kicad_mod");
        std::fs::write(&src, MINIMAL_FOOTPRINT).unwrap();
        let sexp = load_footprint(&src).unwrap();
        let out = tmp.path().join("output.kicad_mod");
        save_footprint(&sexp, &out).unwrap();
        let reloaded = load_footprint(&out).unwrap();
        assert_eq!(reloaded.tag(), sexp.tag());
        assert_eq!(reloaded.string_at(0), sexp.string_at(0));
        assert_eq!(reloaded.find_children("pad").len(), 2);
    });
}

// ------------------------------------------------------------------ design rules

#[test]
fn load_design_rules_cases() {
    let tmp = tempfile::tempdir().unwrap();
    let dru = tmp.path().join("test.kicad_dru");
    std::fs::write(&dru, MINIMAL_DESIGN_RULES).unwrap();
    let sexp = load_design_rules(&dru).unwrap();
    assert!(sexp.has_tag("design_rules"));
    assert_eq!(sexp.find_child("version").unwrap().int_at(0), Some(1));
    assert_eq!(sexp.find_children("rule").len(), 6);

    let e = load_design_rules(tmp.path().join("nonexistent.kicad_dru")).unwrap_err();
    assert_eq!(kind(&e), Some(ErrorKind::FileNotFound));

    let empty = tmp.path().join("empty.kicad_dru");
    std::fs::write(&empty, "").unwrap();
    let e = load_design_rules(&empty).unwrap_err();
    assert_eq!(kind(&e), Some(ErrorKind::FileFormat));
    assert!(e.to_string().contains("Empty design rules"));

    let nov = tmp.path().join("no_version.kicad_dru");
    std::fs::write(&nov, "(rule \"Test\" (constraint clearance))").unwrap();
    let e = load_design_rules(&nov).unwrap_err();
    assert!(e.to_string().contains("Invalid design rules"));
}

#[test]
fn save_design_rules_roundtrip() {
    with_policy(Some("ignore"), || {
        let tmp = tempfile::tempdir().unwrap();
        let dru = tmp.path().join("test.kicad_dru");
        std::fs::write(&dru, MINIMAL_DESIGN_RULES).unwrap();
        let sexp = load_design_rules(&dru).unwrap();
        let out = tmp.path().join("output.kicad_dru");
        save_design_rules(&sexp, &out).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(!text.contains("design_rules"));
        assert_eq!(
            load_design_rules(&out).unwrap().find_children("rule").len(),
            6
        );
    });
}

// ------------------------------------------------------------------ verify_pcb_write

#[test]
fn verify_pcb_write_cases() {
    with_policy(Some("ignore"), || {
        let tmp = tempfile::tempdir().unwrap();
        let pcb = tmp.path().join("test.kicad_pcb");
        std::fs::write(&pcb, MINIMAL_PCB).unwrap();
        verify_pcb_write(&pcb, 0, 0, 0).unwrap();

        let msg = |z, v, s| {
            let e = verify_pcb_write(&pcb, z, v, s).unwrap_err();
            assert!(e.downcast_ref::<WriteVerificationError>().is_some());
            e.to_string()
        };
        assert!(msg(3, 0, 0).contains("zone"));
        assert!(msg(0, 10, 0).contains("via"));
        assert!(msg(0, 0, 5).contains("segment"));
        let all = msg(1, 1, 1);
        assert!(all.contains("zone") && all.contains("via") && all.contains("segment"));

        let mut sexp = load_pcb(&pcb).unwrap();
        sexp.push(parse("(zone (net 1) (net_name \"GND\") (layer \"B.Cu\"))").unwrap());
        sexp.push(
            parse("(via (at 50 50) (size 0.45) (drill 0.2) (layers \"F.Cu\" \"B.Cu\") (net 1))")
                .unwrap(),
        );
        save_pcb(&sexp, &pcb).unwrap();
        verify_pcb_write(&pcb, 1, 1, 0).unwrap();
    });
}

// ------------------------------------------------------------------ kicad lock

fn marked(dir: &Path, name: &str, marker: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"original").unwrap();
    std::fs::write(dir.join(format!("~{name}.lck")), marker).unwrap();
    path
}

#[test]
fn all_core_writers_honour_policy() {
    for policy in ["warn", "error", "ignore"] {
        for (ext, root) in [
            ("kicad_pcb", "(kicad_pcb)"),
            ("kicad_sch", "(kicad_sch)"),
            ("kicad_mod", "(footprint \"x\")"),
            ("kicad_dru", "(design_rules (version 1))"),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let name = format!("my board.rev2.{ext}");
            let path = marked(
                tmp.path(),
                &name,
                r#"{"username":"alice","hostname":"workstation"}"#,
            );
            let marker = tmp.path().join(format!("~{name}.lck"));
            let original_marker = std::fs::read(&marker).unwrap();
            let sexp = parse(root).unwrap();
            let result = with_policy(Some(policy), || match ext {
                "kicad_pcb" => save_pcb(&sexp, &path),
                "kicad_sch" => save_schematic(&sexp, &path),
                "kicad_mod" => save_footprint(&sexp, &path),
                _ => save_design_rules(&sexp, &path),
            });
            if policy == "error" {
                let e = result.unwrap_err();
                let lock = e.downcast_ref::<KiCadLockError>().unwrap();
                assert!(matches!(lock, KiCadLockError::Locked(_)));
                assert!(e.to_string().contains("may be open in KiCad"));
                assert!(e
                    .to_string()
                    .contains("username='alice', hostname='workstation'"));
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
            } else {
                result.unwrap();
                assert_ne!(std::fs::read(&path).unwrap(), b"original");
            }
            assert_eq!(std::fs::read(&marker).unwrap(), original_marker);
            assert!(!tmp_sibling(&path).exists());
        }
    }
}

#[test]
fn probe_metadata() {
    for (content, owner) in [
        (
            r#"{"username":"alice","hostname":"host"}"#,
            (Some("alice"), Some("host")),
        ),
        ("", (None, None)),
        ("not-json PRIVATE", (None, None)),
        ("[]", (None, None)),
        (r#"{"username":12,"hostname":false}"#, (None, None)),
        (
            r#"{"username":"","hostname":"host"}"#,
            (Some(""), Some("host")),
        ),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("board.kicad_pcb");
        let marker = tmp.path().join("~board.kicad_pcb.lck");
        std::fs::write(&marker, content).unwrap();
        let p = probe_kicad_lock(&path);
        assert!(p.present);
        assert_eq!(p.path, marker);
        assert_eq!(
            (p.username.as_deref(), p.hostname.as_deref()),
            owner,
            "{content}"
        );
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), content);
    }
}

#[test]
fn unrelated_markers_do_not_count() {
    for name in [
        "board.kicad_pcb.lck",
        ".board.kicad_pcb.lck",
        "~other.kicad_pcb.lck",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(name), "").unwrap();
        let r = with_policy(None, || {
            check_kicad_lock(tmp.path().join("board.kicad_pcb"), None)
        });
        assert!(!r.unwrap().unwrap().present);
    }
}

#[test]
fn policy_precedence_and_invalid_values() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("board.kicad_pcb");
    std::fs::write(tmp.path().join("~board.kicad_pcb.lck"), "").unwrap();
    with_policy(Some("error"), || {
        assert_eq!(check_kicad_lock(&path, Some("ignore")).unwrap(), None);
        assert!(matches!(
            check_kicad_lock(&path, None),
            Err(KiCadLockError::Locked(_))
        ));
    });
    with_policy(Some("invalid"), || {
        let e = check_kicad_lock(&path, None).unwrap_err();
        assert!(e.to_string().contains("expected warn, error or ignore"));
        assert_eq!(check_kicad_lock(&path, Some("ignore")).unwrap(), None);
        assert!(matches!(
            check_kicad_lock(&path, Some("")),
            Err(KiCadLockError::InvalidPolicy(_))
        ));
    });
}

#[test]
fn default_policy_warns_and_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("board.kicad_sch");
    std::fs::write(tmp.path().join("~board.kicad_sch.lck"), "MALFORMED PRIVATE").unwrap();
    with_policy(None, || {
        save_schematic(&parse("(kicad_sch)").unwrap(), &path).unwrap();
    });
    assert!(path.exists());
}

#[cfg(unix)]
#[test]
fn nonregular_markers_are_present() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("board.kicad_pcb");
    let marker = tmp.path().join("~board.kicad_pcb.lck");
    std::fs::create_dir(&marker).unwrap();
    let p = probe_kicad_lock(&path);
    assert!(p.present && p.username.is_none());
    std::fs::remove_dir(&marker).unwrap();
    std::os::unix::fs::symlink(tmp.path().join("absent"), &marker).unwrap();
    let p = probe_kicad_lock(&path);
    assert!(p.present && p.username.is_none());
}

#[test]
fn invalid_policy_preserves_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("board.kicad_pcb");
    std::fs::write(&path, b"original").unwrap();
    let e = with_policy(Some("WARN"), || {
        save_pcb(&parse("(kicad_pcb)").unwrap(), &path)
    })
    .unwrap_err();
    assert!(e.to_string().contains("Invalid KCT_KICAD_LOCK_POLICY"));
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert!(!tmp_sibling(&path).exists());
}

#[test]
fn control_characters_in_metadata_and_paths_are_escaped() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("board\n\x1b[31m.kicad_pcb");
    let marker = tmp.path().join("~board\n\x1b[31m.kicad_pcb.lck");
    std::fs::write(
        &marker,
        serde_json::json!({"username": "\x1b[31m", "hostname": "host\nsecond"}).to_string(),
    )
    .unwrap();
    let e = check_kicad_lock(&path, Some("error"))
        .unwrap_err()
        .to_string();
    assert!(!e.contains('\x1b'));
    assert!(!e.contains('\n'));
    assert!(e.contains("\\x1b[31m") && e.contains("host\\nsecond"));
}

#[test]
fn deeply_malformed_json_remains_present() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("~board.kicad_pcb.lck"), "[".repeat(1500)).unwrap();
    assert!(probe_kicad_lock(tmp.path().join("board.kicad_pcb")).present);
}

// ------------------------------------------------------------------ version

#[test]
fn format_version_constants() {
    assert_eq!(KICAD_BOARD_FORMAT_VERSION, 20241229);
    assert_eq!(KICAD_SCH_FORMAT_VERSION, 20231120);
    assert_eq!(KICAD_SYM_FORMAT_VERSION, 20231120);
    assert_eq!(KICAD_GENERATOR_VERSION, "10.0");
    const { assert!(KICAD_BOARD_FORMAT_VERSION < 20260206) };
}
