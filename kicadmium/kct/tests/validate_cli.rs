//! `kct validate --sync` / `--connectivity` must never report a pass for
//! inputs they did not compare.
//!
//! Fixture: bp-test `direction-led-tester` (KiCad DRC + schematic parity
//! clean) with its `kicad-cli sch export netlist` output. Each case runs in
//! a child process with `KICADMIUM_KICAD_CLI` pointing at a stub, so the
//! tests need no KiCad install.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const CHILD_ENV: &str = "KCT_VALIDATE_CHILD_ARGS";
const BEGIN: &str = "<<<KCT-STDOUT-BEGIN>>>";
const STEM: &str = "direction-led-tester";

/// Child entry point: runs `kct::cli::run` with the JSON-encoded args.
#[test]
fn kct_validate_child_entry() {
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

fn fixture(ext: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/validate_sync")
        .join(format!("{STEM}.{ext}"))
}

/// Copy the board project into `dir`; return `(sch, pcb)`.
fn stage(dir: &Path) -> (PathBuf, PathBuf) {
    for ext in ["kicad_sch", "kicad_pcb", "kicad_pro"] {
        std::fs::copy(fixture(ext), dir.join(format!("{STEM}.{ext}"))).unwrap();
    }
    (
        dir.join(format!("{STEM}.kicad_sch")),
        dir.join(format!("{STEM}.kicad_pcb")),
    )
}

/// A `kicad-cli` stand-in: copies the fixture netlist to `--output`, or
/// fails every invocation when `working` is false.
fn stub_kicad_cli(dir: &Path, working: bool) -> PathBuf {
    let stub = dir.join("kicad-cli");
    let body = if working {
        format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = --output ]; then cp '{}' \"$2\"; \
             shift; fi\n  shift\ndone\nexit 0\n",
            fixture("kicad_net").display()
        )
    } else {
        "#!/bin/sh\necho 'stub: unavailable' >&2\nexit 1\n".to_string()
    };
    std::fs::write(&stub, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    stub
}

fn run(args: &[&str], kicad_cli: &Path) -> (i32, String, String) {
    let exe = std::env::current_exe().unwrap();
    let out = Command::new(exe)
        .args([
            "kct_validate_child_entry",
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
        .map(|(_, b)| b.trim().to_string())
        .unwrap_or_else(|| panic!("child produced no marker: {stdout}\n{stderr}"));
    (out.status.code().unwrap_or(-1), body, stderr)
}

fn sync(sch: &Path, pcb: &Path, cli: &Path) -> (i32, Value) {
    let (code, stdout, stderr) = run(
        &[
            "validate",
            sch.to_str().unwrap(),
            pcb.to_str().unwrap(),
            "--sync",
            "--format",
            "json",
        ],
        cli,
    );
    let v = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
    (code, v)
}

fn categories(v: &Value) -> Vec<(String, String)> {
    v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["category"].as_str().unwrap().to_string(),
                i["severity"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn positional_sch_pcb_are_compared() {
    let tmp = tempfile::tempdir().unwrap();
    let (sch, pcb) = stage(tmp.path());
    let cli = stub_kicad_cli(tmp.path(), true);
    let (code, v) = sync(&sch, &pcb, &cli);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["schematic"], sch.to_str().unwrap());
    assert_eq!(v["pcb"], pcb.to_str().unwrap());
    assert_eq!(v["in_sync"], true);
    assert_eq!(v["netlist_source"], "kicad-cli");
    assert!(categories(&v).is_empty(), "{v}");
}

#[test]
fn pad_net_mismatch_is_out_of_sync() {
    let tmp = tempfile::tempdir().unwrap();
    let (sch, pcb) = stage(tmp.path());
    let cli = stub_kicad_cli(tmp.path(), true);
    // R1 pad 2 moved from /LED_DRV to /SIG- on the board only.
    let text = std::fs::read_to_string(&pcb).unwrap();
    let needle = "(net \"/LED_DRV\")\n\t\t\t(pintype \"passive\")\n\t\t\t(uuid \"7539e51a";
    assert_eq!(text.matches(needle).count(), 1);
    std::fs::write(
        &pcb,
        text.replace(needle, &needle.replace("/LED_DRV", "/SIG-")),
    )
    .unwrap();
    let (code, v) = sync(&sch, &pcb, &cli);
    assert_eq!(code, 1);
    assert_eq!(v["in_sync"], false);
    let issue = &v["issues"][0];
    assert_eq!(issue["category"], "net_mismatch");
    assert_eq!(issue["reference"], "R1");
    assert_eq!(issue["pin"], "2");
    assert_eq!(issue["net_schematic"], "/LED_DRV");
    assert_eq!(issue["net_pcb"], "/SIG-");
}

#[test]
fn renamed_footprint_reports_missing_and_orphan() {
    let tmp = tempfile::tempdir().unwrap();
    let (sch, pcb) = stage(tmp.path());
    let cli = stub_kicad_cli(tmp.path(), true);
    let text = std::fs::read_to_string(&pcb).unwrap();
    let needle = "(property \"Reference\" \"D2\"";
    assert_eq!(text.matches(needle).count(), 1);
    std::fs::write(&pcb, text.replace(needle, "(property \"Reference\" \"D9\"")).unwrap();
    let (code, v) = sync(&sch, &pcb, &cli);
    assert_eq!(code, 1);
    let cats = categories(&v);
    assert!(
        cats.contains(&("missing_on_pcb".into(), "error".into())),
        "{v}"
    );
    assert!(
        cats.contains(&("orphaned_on_pcb".into(), "warning".into())),
        "{v}"
    );
}

#[test]
fn without_netlist_net_checks_are_not_a_silent_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let (sch, pcb) = stage(tmp.path());
    let cli = stub_kicad_cli(tmp.path(), false);
    let (code, v) = sync(&sch, &pcb, &cli);
    assert_eq!(code, 0);
    assert_eq!(v["netlist_source"], "none");
    assert_eq!(categories(&v), vec![("coverage".into(), "warning".into())]);
}

#[test]
fn sync_without_both_inputs_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (sch, pcb) = stage(tmp.path());
    let cli = stub_kicad_cli(tmp.path(), true);
    for args in [
        vec![
            "validate",
            sch.to_str().unwrap(),
            "--sync",
            "--format",
            "json",
        ],
        vec!["validate", "--pcb", pcb.to_str().unwrap(), "--sync"],
        vec!["validate", "--sync", "--format", "json"],
    ] {
        let (code, stdout, stderr) = run(&args, &cli);
        assert_eq!(code, 1, "{args:?}: {stdout}");
        assert!(stdout.is_empty(), "{args:?} printed a result: {stdout}");
        assert!(stderr.contains("--sync needs a"), "{stderr}");
    }
    let missing = tmp.path().join("gone.kicad_pcb");
    let (code, stdout, stderr) = run(
        &[
            "validate",
            "--schematic",
            sch.to_str().unwrap(),
            "--pcb",
            missing.to_str().unwrap(),
            "--sync",
        ],
        &cli,
    );
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("PCB not found"), "{stderr}");
}

fn connectivity(pcb: &Path, cli: &Path) -> (i32, Value) {
    let (code, stdout, stderr) = run(
        &[
            "validate",
            pcb.to_str().unwrap(),
            "--connectivity",
            "--format",
            "json",
        ],
        cli,
    );
    let v = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
    (code, v)
}

#[test]
fn connectivity_flag_runs_the_connectivity_check() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, pcb) = stage(tmp.path());
    // A failing kicad-cli forces the internal copper graph.
    let cli = stub_kicad_cli(tmp.path(), false);
    let (code, v) = connectivity(&pcb, &cli);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["is_fully_routed"], true);
    assert_eq!(v["summary"]["total_nets"], 3);

    // Drop every /LED_DRV track: the net's pads are no longer joined.
    let text = std::fs::read_to_string(&pcb).unwrap();
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find("\t(segment\n") {
        let end = i + rest[i..].find("\n\t)\n").unwrap() + 4;
        out.push_str(&rest[..i]);
        if !rest[i..end].contains("(net \"/LED_DRV\")") {
            out.push_str(&rest[i..end]);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    assert!(out.len() < text.len());
    std::fs::write(&pcb, out).unwrap();
    let (code, v) = connectivity(&pcb, &cli);
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["is_fully_routed"], false);
    assert!(v["summary"]["errors"].as_u64().unwrap() > 0, "{v}");
    assert!(v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["net_name"] == "/LED_DRV"));
}
