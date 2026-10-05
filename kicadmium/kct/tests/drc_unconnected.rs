//! `kct drc` keeps KiCad's `unconnected_items` and `schematic_parity`
//! (previously parsed and then dropped from both JSON and text output).

use std::path::Path;
use std::process::Command;

use serde_json::Value;

const CHILD_ENV: &str = "KCT_DRC_UNCONNECTED_CHILD_ARGS";
const BEGIN: &str = "<<<KCT-STDOUT-BEGIN>>>";

/// Child entry point: runs `kct::cli::run` with the JSON-encoded args.
#[test]
fn kct_drc_unconnected_child_entry() {
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

fn run(args: &[&str]) -> (i32, String) {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "kct_drc_unconnected_child_entry",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, serde_json::to_string(args).unwrap())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let body = stdout
        .split_once(BEGIN)
        .map(|(_, b)| b.trim().to_string())
        .unwrap_or_else(|| {
            panic!(
                "no marker: {stdout}\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
    (out.status.code().unwrap_or(-1), body)
}

fn item(kind: &str, severity: &str, desc: &str) -> Value {
    serde_json::json!({
        "type": kind,
        "severity": severity,
        "description": desc,
        "items": [
            {"description": "Track [GND] on B.Cu, length 1.0 mm", "pos": {"x": 101.6, "y": -29.972}},
            {"description": "Track [GND] on F.Cu, length 2.0 mm", "pos": {"x": 105.886, "y": -27.146}}
        ]
    })
}

/// A kicad-cli DRC JSON report: 1 warning violation, 2 unconnected
/// items, and (optionally) 1 schematic-parity warning.
fn write_report(dir: &Path, with_parity: bool) -> String {
    let mut report = serde_json::json!({
        "$schema": "https://schemas.kicad.org/drc.v1.json",
        "source": "board.kicad_pcb",
        "violations": [item("silk_overlap", "warning", "Silkscreen overlap")],
        "unconnected_items": [
            item("unconnected_items", "error", "Missing connection between items"),
            item("unconnected_items", "error", "Missing connection between items"),
        ],
    });
    if with_parity {
        report["schematic_parity"] = serde_json::json!([item(
            "net_conflict",
            "warning",
            "No corresponding pin found in schematic"
        )]);
    }
    let path = dir.join("board-drc.json");
    std::fs::write(&path, report.to_string()).unwrap();
    path.display().to_string()
}

#[test]
fn json_output_keeps_unconnected_items_and_parity() {
    let tmp = tempfile::tempdir().unwrap();
    let report = write_report(tmp.path(), true);
    let (code, stdout) = run(&["drc", &report, "--format", "json"]);
    let v: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["summary"]["errors"], 0);
    assert_eq!(v["summary"]["warnings"], 1);
    assert_eq!(v["summary"]["unconnected_items"], 2);
    assert_eq!(v["summary"]["schematic_parity"], 1);
    assert_eq!(v["unconnected_items"].as_array().unwrap().len(), 2);
    assert_eq!(v["schematic_parity"][0]["type_str"], "net_conflict");
    // Unconnected copper is an error, so the run fails.
    assert_eq!(code, 1);
}

#[test]
fn parity_absent_from_report_is_null_not_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let report = write_report(tmp.path(), false);
    let (_, stdout) = run(&["drc", &report, "--format", "json"]);
    let v: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["summary"]["schematic_parity"], Value::Null);
    assert_eq!(v["schematic_parity"], Value::Null);
    assert_eq!(v["summary"]["unconnected_items"], 2);
}

#[test]
fn text_output_lists_unconnected_items() {
    let tmp = tempfile::tempdir().unwrap();
    let report = write_report(tmp.path(), true);
    let (code, stdout) = run(&["drc", &report]);
    assert_eq!(code, 1);
    for want in [
        "Unconnected items: 2",
        "Schematic parity:  1",
        "UNCONNECTED ITEMS (2):",
        "SCHEMATIC PARITY (1):",
        "DRC FAILED",
    ] {
        assert!(stdout.contains(want), "missing {want:?} in:\n{stdout}");
    }
}
