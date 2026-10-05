//! `kicad-cli` discovery and ERC/DRC invocation (port of the
//! `find_kicad_cli` / `run_erc` / `run_drc` / `get_kicad_version` parts of
//! `kicad_tools.cli.runner`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Locate `kicad-cli`: `KICADMIUM_KICAD_CLI`, `KICAD_CLI`, `PATH`, then the
/// stock install locations upstream probes.
pub fn find_kicad_cli() -> Option<PathBuf> {
    for var in ["KICADMIUM_KICAD_CLI", "KICAD_CLI"] {
        if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            let p = PathBuf::from(v);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in ["kicad-cli", "kicad-cli.exe"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let locations = [
        PathBuf::from("/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli"),
        home.join("Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli"),
        PathBuf::from("/opt/homebrew/bin/kicad-cli"),
        PathBuf::from("/usr/local/bin/kicad-cli"),
        PathBuf::from("/usr/bin/kicad-cli"),
        PathBuf::from("C:/Program Files/KiCad/8.0/bin/kicad-cli.exe"),
        PathBuf::from("C:/Program Files/KiCad/7.0/bin/kicad-cli.exe"),
        PathBuf::from("C:/Program Files/KiCad/bin/kicad-cli.exe"),
    ];
    locations.into_iter().find(|p| p.exists())
}

/// Result from running kicad-cli.
#[derive(Debug, Clone, Default)]
pub struct KiCadCLIResult {
    pub success: bool,
    pub output_path: Option<PathBuf>,
    pub stdout: String,
    pub stderr: String,
    pub return_code: i32,
}

impl KiCadCLIResult {
    fn failure(stderr: impl Into<String>) -> Self {
        KiCadCLIResult {
            success: false,
            stderr: stderr.into(),
            ..Default::default()
        }
    }
}

const NOT_FOUND: &str = "kicad-cli not found. Install KiCad 8 from https://www.kicad.org/download/";

/// `tempfile.mkstemp(suffix, prefix)` equivalent: create an empty file.
fn make_temp(prefix: &str, suffix: &str) -> std::io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir();
    loop {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "{prefix}{:x}{nanos:x}{n:x}{suffix}",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    kicad_cli: Option<&Path>,
    sub: &str,
    prefix: &str,
    input: &Path,
    output_path: Option<&Path>,
    format: &str,
    extra_flag: Option<&str>,
    what: &str,
) -> (KiCadCLIResult, Option<PathBuf>) {
    let cli = match kicad_cli.map(Path::to_path_buf).or_else(find_kicad_cli) {
        Some(c) => c,
        None => return (KiCadCLIResult::failure(NOT_FOUND), None),
    };
    let output = match output_path {
        Some(p) => p.to_path_buf(),
        None => {
            let suffix = if format == "json" { ".json" } else { ".rpt" };
            match make_temp(prefix, suffix) {
                Ok(p) => p,
                Err(e) => {
                    return (
                        KiCadCLIResult::failure(format!("Failed to run {what}: {e}")),
                        None,
                    )
                }
            }
        }
    };
    let mut cmd = Command::new(&cli);
    cmd.args([sub, if sub == "sch" { "erc" } else { "drc" }])
        .arg("--output")
        .arg(&output)
        .args(["--format", format, "--units", "mm"]);
    if let Some(flag) = extra_flag {
        cmd.arg(flag);
    }
    cmd.arg(input);
    match cmd.output() {
        Ok(out) => {
            let res = KiCadCLIResult {
                success: false,
                output_path: None,
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                return_code: out.status.code().unwrap_or(-1),
            };
            (res, Some(output))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            KiCadCLIResult::failure(format!("kicad-cli not found: {e}")),
            None,
        ),
        Err(e) => (
            KiCadCLIResult::failure(format!("Failed to run {what}: {e}")),
            None,
        ),
    }
}

/// Run `kicad-cli sch erc` on a schematic.
pub fn run_erc(
    schematic_path: &Path,
    output_path: Option<&Path>,
    format: &str,
    severity_all: bool,
    kicad_cli: Option<&Path>,
) -> KiCadCLIResult {
    let (mut res, out) = run(
        kicad_cli,
        "sch",
        "erc_",
        schematic_path,
        output_path,
        format,
        severity_all.then_some("--severity-all"),
        "ERC",
    );
    let Some(out) = out else {
        return res;
    };
    // A load failure exits non-zero and leaves the pre-created file empty
    // (issue #2780): report it instead of false-passing.
    let load_failed = res.stderr.contains("Failed to load");
    let empty = std::fs::metadata(&out).map_or(true, |m| m.len() == 0);
    if load_failed || (res.return_code != 0 && empty) {
        if res.stderr.is_empty() {
            res.stderr = "ERC failed to load schematic".into();
        }
        res.stdout.clear();
        return res;
    }
    if out.exists() {
        res.success = true;
        res.output_path = Some(out);
    } else if res.stderr.is_empty() {
        res.stderr = "ERC produced no output".into();
    }
    res
}

/// Run `kicad-cli pcb drc` on a board.
pub fn run_drc(
    pcb_path: &Path,
    output_path: Option<&Path>,
    format: &str,
    schematic_parity: bool,
    kicad_cli: Option<&Path>,
) -> KiCadCLIResult {
    let (mut res, out) = run(
        kicad_cli,
        "pcb",
        "drc_",
        pcb_path,
        output_path,
        format,
        schematic_parity.then_some("--schematic-parity"),
        "DRC",
    );
    let Some(out) = out else {
        return res;
    };
    if out.exists() {
        res.success = true;
        res.output_path = Some(out);
    } else if res.stderr.is_empty() {
        res.stderr = "DRC produced no output".into();
    }
    res
}

/// `kicad-cli version` output, trimmed.
pub fn get_kicad_version(kicad_cli: Option<&Path>) -> Option<String> {
    let cli = kicad_cli.map(Path::to_path_buf).or_else(find_kicad_cli)?;
    let out = Command::new(cli).arg("version").output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
