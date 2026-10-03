//! `--format json` stdout must be exactly one JSON document: progress and
//! info lines belong on stderr.
//!
//! Each case runs `kct::cli::run` in a child process (so stdout can be
//! captured) with `KICADMIUM_KICAD_CLI` pointing at a stub that copies a
//! canned KiCad report to the requested `--output`, so no KiCad install is
//! needed.

use std::path::{Path, PathBuf};
use std::process::Command;

const CHILD_ENV: &str = "KCT_JSON_STDOUT_CHILD_ARGS";
const BEGIN: &str = "<<<KCT-STDOUT-BEGIN>>>";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Child entry point: runs `kct::cli::run` with the JSON-encoded args.
#[test]
fn kct_json_stdout_child_entry() {
    let Ok(raw) = std::env::var(CHILD_ENV) else {
        return;
    };
    let args: Vec<String> = serde_json::from_str(&raw).unwrap();
    use std::io::Write;
    print!("{BEGIN}");
    std::io::stdout().flush().unwrap();
    let code = kct::cli::run(args).unwrap_or_else(|e| {
        eprintln!("Error: {e:#}");
        1
    });
    std::io::stdout().flush().unwrap();
    std::process::exit(code);
}

/// A `kicad-cli` stand-in that writes `report` to the `--output` path.
fn stub_kicad_cli(dir: &Path, report: &Path) -> PathBuf {
    let stub = dir.join("kicad-cli");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = --output ]; then cp '{}' \"$2\"; \
             shift; fi\n  shift\ndone\nexit 0\n",
            report.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    stub
}

/// Run the child; return `(exit code, stdout after the marker, stderr)`.
fn run_child(args: &[&str], kicad_cli: &Path) -> (i32, String, String) {
    let exe = std::env::current_exe().unwrap();
    let out = Command::new(exe)
        .args([
            "kct_json_stdout_child_entry",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, serde_json::to_string(args).unwrap())
        .env("KICADMIUM_KICAD_CLI", kicad_cli)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let body = stdout
        .split_once(BEGIN)
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| panic!("child produced no marker: {stdout}\n{stderr}"));
    (out.status.code().unwrap_or(-1), body, stderr)
}

/// The child exits from inside the test, so everything after the marker is
/// the command's own stdout.
fn parse_json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}):\n{stdout}"))
}

#[test]
fn drc_json_stdout_is_pure_json() {
    let tmp = tempfile::tempdir().unwrap();
    let cli = stub_kicad_cli(
        tmp.path(),
        &fixtures().join("drc/kicad_cli_drc_golden.json"),
    );
    let pcb = fixtures().join("drc/orphan_island.kicad_pcb");
    let (_, stdout, stderr) = run_child(&["drc", pcb.to_str().unwrap(), "--format", "json"], &cli);
    let v = parse_json(&stdout);
    assert!(v.get("summary").is_some(), "{v}");
    assert!(stderr.contains("Running DRC on:"), "{stderr}");
}

#[test]
fn erc_json_stdout_is_pure_json() {
    let tmp = tempfile::tempdir().unwrap();
    let cli = stub_kicad_cli(tmp.path(), &fixtures().join("sample_erc.json"));
    let sch = fixtures().join("simple_rc.kicad_sch");
    let (_, stdout, stderr) = run_child(
        &["erc", "parse", sch.to_str().unwrap(), "--format", "json"],
        &cli,
    );
    let v = parse_json(&stdout);
    assert!(v.get("summary").is_some(), "{v}");
    assert!(stderr.contains("Running ERC on:"), "{stderr}");
}

#[test]
fn check_json_stdout_is_pure_json() {
    let tmp = tempfile::tempdir().unwrap();
    let cli = stub_kicad_cli(
        tmp.path(),
        &fixtures().join("drc/kicad_cli_drc_golden.json"),
    );
    let pcb = fixtures().join("drc/orphan_island.kicad_pcb");
    let (_, stdout, _) = run_child(&["check", pcb.to_str().unwrap(), "--format", "json"], &cli);
    let v = parse_json(&stdout);
    assert!(v.get("violations").is_some(), "{v}");
}
