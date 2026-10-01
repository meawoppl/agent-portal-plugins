//! Port of upstream tests/test_transaction.py (`with` blocks map to
//! `BoardTransaction::run`; `KeyboardInterrupt` to a panic unwinding
//! through the guard).

use std::path::{Path, PathBuf};

use kct::transaction::{board_transaction, BoardTransaction, TransactionRestoreError};

const ORIGINAL: &[u8] = b"(kicad_pcb (version 20240108) (net 0 \"\") (net 1 \"GND\"))\n";

fn board(dir: &Path) -> PathBuf {
    let f = dir.join("board.kicad_pcb");
    std::fs::write(&f, ORIGINAL).unwrap();
    f
}

fn sidecars(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".failed-")
        })
        .collect();
    v.sort();
    v
}

fn fail_with(path: &Path, bytes: &'static [u8], keep_failed: bool) -> anyhow::Error {
    let mut txn = board_transaction([path], true, keep_failed).unwrap();
    txn.run(|_| -> anyhow::Result<()> {
        std::fs::write(path, bytes)?;
        anyhow::bail!("boom")
    })
    .unwrap_err()
}

#[test]
fn exception_rollback_restores_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    let e = fail_with(&b, b"(kicad", true);
    assert_eq!(e.to_string(), "boom");
    assert_eq!(std::fs::read(&b).unwrap(), ORIGINAL);
    let mut appended = ORIGINAL.to_vec();
    appended.extend_from_slice(b"(segment ...)\n");
    let leaked: &'static [u8] = Box::leak(appended.into_boxed_slice());
    fail_with(&b, leaked, true);
    assert_eq!(std::fs::read(&b).unwrap(), ORIGINAL);
}

#[test]
fn custom_error_propagates_unchanged() {
    #[derive(Debug)]
    struct CustomError;
    impl std::fmt::Display for CustomError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("custom")
        }
    }
    impl std::error::Error for CustomError {}
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    let mut txn = board_transaction([&b], true, true).unwrap();
    let e = txn
        .run(|_| -> anyhow::Result<()> {
            std::fs::write(&b, b"garbage")?;
            Err(CustomError.into())
        })
        .unwrap_err();
    assert!(e.downcast_ref::<CustomError>().is_some());
}

#[test]
fn panic_rolls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    let b2 = b.clone();
    let result = std::panic::catch_unwind(move || {
        let _txn = board_transaction([&b2], true, true).unwrap();
        std::fs::write(&b2, b"half-written").unwrap();
        panic!("interrupt");
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read(&b).unwrap(), ORIGINAL);
}

#[test]
fn explicit_rollback_and_idempotence() {
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    let mut txn = board_transaction([&b], true, true).unwrap();
    std::fs::write(&b, b"mutated").unwrap();
    txn.rollback().unwrap();
    assert_eq!(std::fs::read(&b).unwrap(), ORIGINAL);
    txn.rollback().unwrap();
    txn.commit();
    assert_eq!(sidecars(tmp.path()).len(), 1);

    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    let mut txn = board_transaction([&b], true, true).unwrap();
    txn.rollback().unwrap();
    assert!(txn.restored.is_empty() && txn.sidecars.is_empty());
    assert!(sidecars(tmp.path()).is_empty());
}

#[test]
fn report_lines_mention_restore_and_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    let mut txn = board_transaction([&b], true, true).unwrap();
    std::fs::write(&b, b"mutated").unwrap();
    txn.rollback().unwrap();
    let lines = txn.report_lines(None);
    assert!(lines
        .iter()
        .any(|l| l.contains(&b.display().to_string()) && l.contains("rolled back")));
    assert!(lines.iter().any(|l| l.contains("failed attempt preserved")));
}

#[test]
fn report_scoping() {
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    let mut txn = board_transaction([&b], true, true).unwrap();
    assert_eq!(<(usize, usize)>::from(txn.mark()), (0, 0));
    std::fs::write(&b, b"first attempt").unwrap();
    txn.rollback().unwrap();
    let mark = txn.mark();
    std::fs::write(&b, b"second attempt").unwrap();
    txn.rollback().unwrap();
    let (first, second) = (txn.sidecars[0].clone(), txn.sidecars[1].clone());
    assert_ne!(first, second);
    let scoped = txn.report_lines(Some(mark));
    assert_eq!(
        scoped.iter().filter(|l| l.contains("rolled back")).count(),
        1
    );
    assert!(scoped
        .iter()
        .any(|l| l.contains(&second.display().to_string())));
    assert!(!scoped
        .iter()
        .any(|l| l.contains(&first.display().to_string())));
    let all = txn.report_lines(None);
    assert_eq!(all.iter().filter(|l| l.contains("rolled back")).count(), 2);
    assert_eq!(
        all.iter()
            .filter(|l| l.contains("failed attempt preserved"))
            .count(),
        2
    );
}

#[test]
fn forensic_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    fail_with(&b, b"the failed mutation attempt", true);
    let s = sidecars(tmp.path());
    assert_eq!(s.len(), 1);
    assert_eq!(
        std::fs::read(&s[0]).unwrap(),
        b"the failed mutation attempt"
    );
    let name = s[0].file_name().unwrap().to_string_lossy().into_owned();
    let re = regex_lite(&name);
    assert!(re, "{name}");

    fail_with(&b, b"second failure", true);
    let s = sidecars(tmp.path());
    assert_eq!(s.len(), 2);
    let contents: std::collections::HashSet<Vec<u8>> =
        s.iter().map(|p| std::fs::read(p).unwrap()).collect();
    assert!(contents.contains(b"second failure".as_slice()));
}

/// `board\.kicad_pcb\.failed-\d{8}T\d{6}Z(-\d+)?\.kicad_pcb`
fn regex_lite(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("board.kicad_pcb.failed-") else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(".kicad_pcb") else {
        return false;
    };
    let b = rest.as_bytes();
    if b.len() < 16 {
        return false;
    }
    let digits = |s: &[u8]| s.iter().all(u8::is_ascii_digit);
    digits(&b[..8])
        && b[8] == b'T'
        && digits(&b[9..15])
        && b[15] == b'Z'
        && (b.len() == 16 || (b[16] == b'-' && b.len() > 17 && digits(&b[17..])))
}

#[test]
fn keep_failed_false_suppresses_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    fail_with(&b, b"garbage", false);
    assert_eq!(std::fs::read(&b).unwrap(), ORIGINAL);
    assert!(sidecars(tmp.path()).is_empty());
}

#[test]
fn success_leaves_no_litter() {
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    let mut txn = board_transaction([&b], true, true).unwrap();
    txn.run(|_| {
        std::fs::write(&b, b"(via ...)")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(std::fs::read(&b).unwrap(), b"(via ...)");
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);

    fail_with(&b, b"garbage", true);
    assert!(!std::fs::read_dir(tmp.path()).unwrap().any(|e| e
        .unwrap()
        .path()
        .extension()
        .is_some_and(|x| x == "tmp")));
}

#[test]
fn multi_path_and_missing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (
        tmp.path().join("a.kicad_pcb"),
        tmp.path().join("b.kicad_pcb"),
    );
    std::fs::write(&a, b"contents of a").unwrap();
    std::fs::write(&b, b"contents of b").unwrap();
    let mut txn = board_transaction([&a, &b], true, true).unwrap();
    let _ = txn.run(|_| -> anyhow::Result<()> {
        std::fs::write(&a, b"mutated a")?;
        anyhow::bail!("x")
    });
    assert_eq!(std::fs::read(&a).unwrap(), b"contents of a");
    let s = sidecars(tmp.path());
    assert_eq!(s.len(), 1);
    assert!(s[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("a.kicad_pcb.failed-"));

    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("new-output.kicad_pcb");
    fail_with(&out, b"partial output", true);
    assert!(!out.exists());
    let s = sidecars(tmp.path());
    assert_eq!(std::fs::read(&s[0]).unwrap(), b"partial output");

    assert!(BoardTransaction::new(Vec::<PathBuf>::new(), true, true).is_err());
}

#[test]
fn disabled_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    let mut txn = board_transaction([&b], false, true).unwrap();
    let _ = txn.run(|t| -> anyhow::Result<()> {
        std::fs::write(&b, b"mutated without protection")?;
        t.rollback()?;
        anyhow::bail!("x")
    });
    assert_eq!(std::fs::read(&b).unwrap(), b"mutated without protection");
    assert!(sidecars(tmp.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn read_only_directory_raises_loudly() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("boards");
    std::fs::create_dir(&sub).unwrap();
    let b = board(&sub);
    let mut txn = board_transaction([&b], true, true).unwrap();
    std::fs::write(&b, b"mutated").unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root bypasses directory permissions; only assert when it applies.
    let probe = std::fs::write(sub.join("probe"), b"x").is_err();
    let r = txn.rollback();
    let e = txn.run(|_| -> anyhow::Result<()> { anyhow::bail!("original failure") });
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
    if probe {
        assert!(matches!(r, Err(TransactionRestoreError(_))));
        assert_eq!(std::fs::read(&b).unwrap(), b"mutated");
        let e = e.unwrap_err();
        assert!(e.downcast_ref::<TransactionRestoreError>().is_some());
        assert!(format!("{e:#}").contains("original failure"));
    }
}

#[test]
fn subprocess_style_mutation_is_covered() {
    let tmp = tempfile::tempdir().unwrap();
    let b = board(tmp.path());
    let replacement = tmp.path().join("external-write.tmp");
    std::fs::write(&replacement, b"externally rewritten").unwrap();
    let mut txn = board_transaction([&b], true, true).unwrap();
    let _ = txn.run(|_| -> anyhow::Result<()> {
        std::fs::rename(&replacement, &b)?;
        anyhow::bail!("x")
    });
    assert_eq!(std::fs::read(&b).unwrap(), ORIGINAL);
}
