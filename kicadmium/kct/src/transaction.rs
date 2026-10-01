//! File-scoped snapshot/rollback transactions for in-place board mutations
//! (port of `kicad_tools.transaction`, issue #4541).
//!
//! * **Byte-identical restore** on failure or explicit [`BoardTransaction::rollback`];
//!   paths absent on entry are removed again.
//! * **Atomic restore** via a `<file>.txn-restore.tmp` sibling + fsync + rename.
//! * **Forensic sidecar**: the failed attempt is kept as
//!   `<file>.failed-<UTC-timestamp><suffix>` (unless `keep_failed` is false).
//! * **No litter on success**; rollback is idempotent and skips unchanged files.
//! * **Loud restore failures**: [`TransactionRestoreError`].
//!
//! Python's `with` block maps to [`BoardTransaction::run`] (rolls back and
//! announces on `Err`) plus a `Drop` guard that rolls back if the scope
//! unwinds from a panic (the `KeyboardInterrupt`/`BaseException` analogue).

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::exceptions::ValueError;

/// High-water mark into a transaction's rollback record (issue #4752).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RollbackMark {
    pub restored: usize,
    pub sidecars: usize,
}

impl From<RollbackMark> for (usize, usize) {
    fn from(m: RollbackMark) -> Self {
        (m.restored, m.sidecars)
    }
}

/// A rollback could not fully restore the pre-command state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRestoreError(pub String);

impl fmt::Display for TransactionRestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TransactionRestoreError {}

/// File-scoped snapshot/rollback transaction (see module docs). Snapshots
/// are taken by [`BoardTransaction::begin`] / [`board_transaction`].
#[derive(Debug)]
pub struct BoardTransaction {
    pub paths: Vec<PathBuf>,
    /// `false`: a complete no-op (no snapshot, no rollback).
    pub enabled: bool,
    pub keep_failed: bool,
    /// Paths actually restored (unchanged paths are skipped).
    pub restored: Vec<PathBuf>,
    /// Forensic sidecars written, chronologically.
    pub sidecars: Vec<PathBuf>,
    snapshots: Vec<Option<Vec<u8>>>,
    entered: bool,
    finished: bool,
}

impl BoardTransaction {
    /// Construct without snapshotting (`BoardTransaction(*paths)`).
    pub fn new<I, P>(paths: I, enabled: bool, keep_failed: bool) -> Result<Self, ValueError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .map(|p| p.as_ref().to_path_buf())
            .collect();
        if paths.is_empty() {
            return Err(ValueError::new(
                "BoardTransaction requires at least one path",
            ));
        }
        Ok(Self {
            paths,
            enabled,
            keep_failed,
            restored: Vec::new(),
            sidecars: Vec::new(),
            snapshots: Vec::new(),
            entered: false,
            finished: false,
        })
    }

    /// Snapshot every path (`__enter__`).
    pub fn begin(&mut self) -> std::io::Result<()> {
        if self.enabled {
            self.snapshots = self
                .paths
                .iter()
                .map(|p| {
                    if p.exists() {
                        std::fs::read(p).map(Some)
                    } else {
                        Ok(None)
                    }
                })
                .collect::<std::io::Result<_>>()?;
        }
        self.entered = true;
        Ok(())
    }

    /// Run `body` as the `with` block: on `Err`, roll back, print
    /// [`report_lines`](Self::report_lines) for that rollback to stderr, and
    /// return the original error (or the restore error, with the original
    /// as context, if rollback failed).
    pub fn run<T>(
        &mut self,
        body: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        if !self.entered {
            self.begin()?;
        }
        let result = body(self);
        self.finished = true;
        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                let mark = self.mark();
                if let Err(restore) = self.rollback() {
                    return Err(anyhow::Error::new(restore).context(format!("{e:#}")));
                }
                for line in self.report_lines(Some(mark)) {
                    eprintln!("{line}");
                }
                Err(e)
            }
        }
    }

    /// Mark the transaction complete so dropping it never rolls back.
    pub fn commit(mut self) {
        self.finished = true;
    }

    /// Restore every changed path to its entry snapshot (idempotent; cheap
    /// when nothing changed). All paths are attempted before reporting.
    pub fn rollback(&mut self) -> Result<(), TransactionRestoreError> {
        if !self.enabled || !self.entered {
            return Ok(());
        }
        let mut errors = Vec::new();
        for (i, path) in self.paths.clone().iter().enumerate() {
            let snapshot = self.snapshots.get(i).cloned().flatten();
            let current = if path.exists() {
                match std::fs::read(path) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        errors.push(format!(
                            "could not read current state of {}: {e}",
                            path.display()
                        ));
                        continue;
                    }
                }
            } else {
                None
            };
            if current == snapshot {
                continue;
            }
            if self.keep_failed {
                if let Some(bytes) = &current {
                    let sidecar = sidecar_path(path);
                    match std::fs::write(&sidecar, bytes) {
                        Ok(()) => self.sidecars.push(sidecar),
                        Err(e) => errors.push(format!(
                            "could not write forensic sidecar for {}: {e}",
                            path.display()
                        )),
                    }
                }
            }
            match restore(path, snapshot.as_deref()) {
                Ok(()) => self.restored.push(path.clone()),
                Err(e) => errors.push(format!("could not restore {}: {e}", path.display())),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(TransactionRestoreError(format!(
                "transactional rollback failed to fully restore the pre-command state:\n  {}",
                errors.join("\n  ")
            )))
        }
    }

    /// Current rollback high-water mark.
    pub fn mark(&self) -> RollbackMark {
        RollbackMark {
            restored: self.restored.len(),
            sidecars: self.sidecars.len(),
        }
    }

    /// Human-readable rollback summary since `since` (whole lifetime when
    /// `None`).
    pub fn report_lines(&self, since: Option<RollbackMark>) -> Vec<String> {
        let start = since.unwrap_or_default();
        let mut lines = Vec::new();
        for path in self.restored.iter().skip(start.restored) {
            lines.push(format!(
                "--transactional: rolled back {} to its pre-command state",
                path.display()
            ));
        }
        for sidecar in self.sidecars.iter().skip(start.sidecars) {
            lines.push(format!(
                "--transactional: failed attempt preserved at {}",
                sidecar.display()
            ));
        }
        lines
    }
}

impl Drop for BoardTransaction {
    /// Unwinding through an active transaction rolls it back (and announces
    /// it) like Python's `BaseException` path; a failed restore is reported
    /// on stderr since it cannot propagate from `Drop`.
    fn drop(&mut self) {
        if self.finished || !self.entered || !std::thread::panicking() {
            return;
        }
        let mark = self.mark();
        match self.rollback() {
            Ok(()) => {
                for line in self.report_lines(Some(mark)) {
                    eprintln!("{line}");
                }
            }
            Err(e) => eprintln!("{e}"),
        }
    }
}

fn restore(path: &Path, snapshot: Option<&[u8]>) -> std::io::Result<()> {
    let Some(bytes) = snapshot else {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.txn-restore.tmp"));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// `YYYYmmddTHHMMSSZ` for now (UTC).
fn utc_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Non-clobbering `<file>.failed-<UTC-timestamp>[-N]<suffix>`.
fn sidecar_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suffix = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let stamp = utc_stamp();
    let mut candidate = path.with_file_name(format!("{name}.failed-{stamp}{suffix}"));
    let mut counter = 1;
    while candidate.exists() {
        candidate = path.with_file_name(format!("{name}.failed-{stamp}-{counter}{suffix}"));
        counter += 1;
    }
    candidate
}

/// Create and enter (snapshot) a transaction over `paths`.
pub fn board_transaction<I, P>(
    paths: I,
    enabled: bool,
    keep_failed: bool,
) -> anyhow::Result<BoardTransaction>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut txn = BoardTransaction::new(paths, enabled, keep_failed)?;
    txn.begin()?;
    Ok(txn)
}
