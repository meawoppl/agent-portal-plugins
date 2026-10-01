//! Native `kicad-cli pcb drc` reconciliation run (port of
//! `kicad_tools.drc.geometric`). Never errors: every skip path returns a
//! `ran = false` result with an explanatory note.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::drc::report::DRCReport;

pub const REASON_OK: &str = "ok";
pub const REASON_ABSENT: &str = "kicad_cli_absent";
pub const REASON_TIMEOUT: &str = "kicad_cli_timeout";
pub const REASON_NO_REPORT: &str = "kicad_cli_no_report";
pub const REASON_CRASH: &str = "kicad_cli_crash";

pub const KICAD_CLI_ABSENT_NOTE: &str = "kicad-cli not found; geometric DRC skipped";

/// Outcome of a native kicad-cli DRC run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GeometricDRCResult {
    pub ran: bool,
    pub error_count: usize,
    /// Error-severity counts by kicad-cli type (insertion ordered).
    pub by_type: Vec<(String, usize)>,
    /// Counts across all severities.
    pub all_by_type: Vec<(String, usize)>,
    pub note: Option<String>,
    pub reason: &'static str,
}

impl GeometricDRCResult {
    fn skipped(note: impl Into<String>, reason: &'static str) -> Self {
        GeometricDRCResult {
            note: Some(note.into()),
            reason,
            ..Default::default()
        }
    }

    pub fn has_errors(&self) -> bool {
        self.ran && self.error_count > 0
    }

    /// The `n` most frequent error types (stable descending by count).
    pub fn top_types(&self, n: usize) -> Vec<(String, usize)> {
        let mut v = self.by_type.clone();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.truncate(n);
        v
    }
}

fn bump(v: &mut Vec<(String, usize)>, key: &str) {
    match v.iter_mut().find(|(k, _)| k == key) {
        Some(e) => e.1 += 1,
        None => v.push((key.to_string(), 1)),
    }
}

/// Removes the report file on drop (`report_path.unlink(missing_ok=True)`).
struct TempPath(PathBuf);

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `run_geometric_drc(pcb_path, timeout=..., kicad_cli=..., refill_zones=...)`.
pub fn run_geometric_drc(
    pcb_path: &Path,
    timeout_s: u64,
    kicad_cli: Option<PathBuf>,
    refill_zones: bool,
) -> GeometricDRCResult {
    let Some(cli) = kicad_cli.or_else(crate::cli::runner::find_kicad_cli) else {
        return GeometricDRCResult::skipped(KICAD_CLI_ABSENT_NOTE, REASON_ABSENT);
    };
    // `tempfile.NamedTemporaryFile(suffix=".json", delete=False)`.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let report_path: PathBuf =
        std::env::temp_dir().join(format!("kct-geometric-{}-{nonce}.json", std::process::id()));
    if let Err(e) = std::fs::File::create(&report_path) {
        return GeometricDRCResult::skipped(format!("geometric DRC failed: {e}"), REASON_CRASH);
    }
    let report = TempPath(report_path.clone());
    let mut cmd = Command::new(&cli);
    cmd.args(["pcb", "drc"]);
    if refill_zones {
        cmd.arg("--refill-zones");
    }
    cmd.args(["--format", "json", "--severity-all", "--units", "mm", "--output"])
        .arg(&report_path)
        .arg(pcb_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return GeometricDRCResult::skipped(format!("geometric DRC failed: {e}"), REASON_CRASH),
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(timeout_s) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return GeometricDRCResult::skipped(
                        "kicad-cli DRC timed out (geometric DRC skipped)",
                        REASON_TIMEOUT,
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return GeometricDRCResult::skipped(format!("geometric DRC failed: {e}"), REASON_CRASH),
        }
    }
    let present = report_path.exists();
    if !present {
        return GeometricDRCResult::skipped(
            "kicad-cli produced no DRC report (geometric DRC skipped)",
            REASON_NO_REPORT,
        );
    }
    let parsed = match DRCReport::load(&report_path) {
        Ok(r) => r,
        Err(e) => return GeometricDRCResult::skipped(format!("geometric DRC failed: {e}"), REASON_CRASH),
    };
    let mut out = GeometricDRCResult {
        ran: true,
        reason: REASON_OK,
        ..Default::default()
    };
    for v in &parsed.violations {
        bump(&mut out.all_by_type, &v.type_str);
        if v.is_error() {
            out.error_count += 1;
            bump(&mut out.by_type, &v.type_str);
        }
    }
    drop(report);
    out
}
