//! `kct impedance` CLI output against goldens captured from upstream Python
//! (`fixtures/physics/impedance_golden.json`: stdout, stderr and exit code of
//! `kicad_tools.cli.main(["impedance", ...])`; `@path` args are fixture paths).
//! Also ports `test_cli_parser_accepts_named_preset` and the CLI half of
//! `test_named_cli_and_explicit_pcb_priority` (test_named_stackup.py).

use std::ffi::OsString;

use kct::cli::impedance::execute;

fn run(args: &[&str]) -> kct::cli::impedance::Output {
    execute(
        args.iter()
            .map(|a| match a.strip_prefix('@') {
                Some(rel) => crate::fixture(rel).into_os_string(),
                None => OsString::from(a),
            })
            .collect(),
    )
}

#[test]
fn impedance_output_matches_upstream_goldens() {
    let text = std::fs::read_to_string(crate::fixture("physics/impedance_golden.json")).unwrap();
    let golden: serde_json::Value = serde_json::from_str(&text).unwrap();
    let cases = golden.as_object().unwrap();
    assert!(cases.len() >= 20);
    for (name, case) in cases {
        let args: Vec<&str> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let out = run(&args);
        assert_eq!(out.stdout, case["stdout"].as_str().unwrap(), "stdout of {name}");
        assert_eq!(out.stderr, case["stderr"].as_str().unwrap(), "stderr of {name}");
        assert_eq!(
            i64::from(out.code),
            case["code"].as_i64().unwrap(),
            "exit code of {name}"
        );
    }
}

#[test]
fn test_cli_parser_accepts_named_preset() {
    let out = run(&["stackup", "--preset", "jlcpcb-7628", "--format", "json"]);
    assert_eq!(out.code, 0);
    let data: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(data["construction"]["factory_id"], "JLC04161H-7628");
}

#[test]
fn test_named_cli_and_explicit_pcb_priority() {
    let out = run(&["stackup", "--preset", "jlcpcb-3313", "--format", "json"]);
    let data: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(data["layers"][1]["thickness_mm"], 0.0994);
    // An explicit board wins over --preset.
    let out = run(&[
        "stackup",
        "@physics/composite_six_layer.kicad_pcb",
        "--preset",
        "jlcpcb-7628",
        "--format",
        "json",
    ]);
    let data: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(data["construction"].is_null());
    assert_eq!(data["layers"][1]["name"], "dielectric 1");
}

#[test]
fn registered_in_command_table() {
    let spec = kct::cli::COMMANDS
        .iter()
        .find(|c| c.name == "impedance")
        .unwrap();
    assert!(spec.run.is_some());
}
