use std::{fs, process::Command};
#[test]
fn output_cannot_replace_board_or_config() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board.kicad_pcb");
    let config = dir.path().join("config.json");
    let content = include_str!("fixtures/advanced-bad.kicad_pcb");
    fs::write(&board, content).unwrap();
    fs::write(&config, "{}").unwrap();
    for output in [&board, &config] {
        let status = Command::new(env!("CARGO_BIN_EXE_pcb-lint"))
            .args([
                "lint",
                board.to_str().unwrap(),
                "--board-id",
                "test",
                "--config",
                config.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(!status.status.success());
        assert!(
            String::from_utf8_lossy(&status.stderr).contains("output aliases")
                || String::from_utf8_lossy(&status.stderr).contains("refusing to overwrite")
        );
    }
    assert_eq!(fs::read_to_string(board).unwrap(), content);
    assert_eq!(fs::read_to_string(config).unwrap(), "{}");
}
#[test]
fn corpus_smoke_checks_all_seventy() {
    let output = Command::new(env!("CARGO_BIN_EXE_pcb-lint"))
        .args([
            "corpus",
            concat!(env!("CARGO_MANIFEST_DIR"), "/examples/corpus.json"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn contact_sheet_and_report_written_even_when_threshold_fails() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board.kicad_pcb");
    fs::write(&board, include_str!("fixtures/advanced-bad.kicad_pcb")).unwrap();
    let config = dir.path().join("config.json");
    fs::write(&config, r#"{"min_trace_mm":1.0}"#).unwrap();
    let report = dir.path().join("report.json");
    let sheet = dir.path().join("sheet.html");
    let result = Command::new(env!("CARGO_BIN_EXE_pcb-lint"))
        .args([
            "lint",
            board.to_str().unwrap(),
            "--board-id",
            "fixture",
            "--fail-on",
            "warning",
            "--config",
            config.to_str().unwrap(),
            "--contact-sheet",
            sheet.to_str().unwrap(),
            "-o",
            report.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    let r: serde_json::Value = serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
    assert!(!r["findings"].as_array().unwrap().is_empty());
    assert!(
        fs::read_to_string(sheet)
            .unwrap()
            .starts_with("<!doctype html>")
    );
}
#[test]
fn sheet_output_cannot_alias_report() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board.kicad_pcb");
    fs::write(&board, include_str!("fixtures/advanced-bad.kicad_pcb")).unwrap();
    let out = dir.path().join("both.html");
    let result = Command::new(env!("CARGO_BIN_EXE_pcb-lint"))
        .args([
            "lint",
            board.to_str().unwrap(),
            "--board-id",
            "fixture",
            "--contact-sheet",
            out.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("output aliases"));
    assert!(!out.exists());
}
