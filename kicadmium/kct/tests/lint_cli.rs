use std::{ffi::OsString, fs};

/// Run the legacy CLI in-process: `Ok(code)` or the error message.
fn legacy(args: &[&str]) -> Result<i32, String> {
    kct::lint::cli::run(args.iter().map(OsString::from)).map_err(|e| format!("{e:#}"))
}

#[test]
fn output_cannot_replace_board_or_config() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board.kicad_pcb");
    let config = dir.path().join("config.json");
    let content = include_str!("fixtures/lint/advanced-bad.kicad_pcb");
    fs::write(&board, content).unwrap();
    fs::write(&config, "{}").unwrap();
    for output in [&board, &config] {
        let err = legacy(&[
            "lint",
            board.to_str().unwrap(),
            "--board-id",
            "test",
            "--config",
            config.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ])
        .unwrap_err();
        assert!(err.contains("output aliases") || err.contains("refusing to overwrite"));
    }
    assert_eq!(fs::read_to_string(board).unwrap(), content);
    assert_eq!(fs::read_to_string(config).unwrap(), "{}");
}
#[test]
fn corpus_smoke_checks_all_seventy() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("evaluation.json");
    let code = legacy(&[
        "corpus",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lint/corpus.json"
        ),
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        code,
        Ok(0),
        "{}",
        fs::read_to_string(&out).unwrap_or_default()
    );
}
#[test]
fn contact_sheet_and_report_written_even_when_threshold_fails() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board.kicad_pcb");
    fs::write(&board, include_str!("fixtures/lint/advanced-bad.kicad_pcb")).unwrap();
    let config = dir.path().join("config.json");
    fs::write(&config, r#"{"min_trace_mm":1.0}"#).unwrap();
    let report = dir.path().join("report.json");
    let sheet = dir.path().join("sheet.html");
    let code = legacy(&[
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
    ]);
    assert_eq!(code, Ok(2));
    let r: serde_json::Value = serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
    assert!(!r["findings"].as_array().unwrap().is_empty());
    assert!(fs::read_to_string(sheet)
        .unwrap()
        .starts_with("<!doctype html>"));
}
#[test]
fn sheet_output_cannot_alias_report() {
    let dir = tempfile::tempdir().unwrap();
    let board = dir.path().join("board.kicad_pcb");
    fs::write(&board, include_str!("fixtures/lint/advanced-bad.kicad_pcb")).unwrap();
    let out = dir.path().join("both.html");
    let err = legacy(&[
        "lint",
        board.to_str().unwrap(),
        "--board-id",
        "fixture",
        "--contact-sheet",
        out.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains("output aliases"));
    assert!(!out.exists());
}
