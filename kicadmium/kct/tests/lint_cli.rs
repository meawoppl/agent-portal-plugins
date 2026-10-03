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

// ---- `kct lint` subcommands ported from the legacy CLI ----

fn kct_lint(args: &[&str]) -> Result<i32, String> {
    kct::cli::run(std::iter::once("lint").chain(args.iter().copied())).map_err(|e| format!("{e:#}"))
}
fn bad_board(dir: &std::path::Path) -> std::path::PathBuf {
    let board = dir.join("board.kicad_pcb");
    fs::write(&board, include_str!("fixtures/lint/advanced-bad.kicad_pcb")).unwrap();
    board
}

#[test]
fn kct_lint_inspect_dumps_model_json_and_counts() {
    let dir = tempfile::tempdir().unwrap();
    let board = bad_board(dir.path());
    let model = kct::lint::model::Board::read(include_str!("fixtures/lint/advanced-bad.kicad_pcb"))
        .unwrap();
    let json = dir.path().join("objects.json");
    assert_eq!(
        kct_lint(&[
            "inspect",
            board.to_str().unwrap(),
            "--format",
            "json",
            "-o",
            json.to_str().unwrap(),
        ]),
        Ok(0)
    );
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&json).unwrap()).unwrap();
    // Same document the legacy `inspect` emitted: the serialized model.
    assert_eq!(v, serde_json::to_value(&model).unwrap());
    assert!(!v["tracks"].as_array().unwrap().is_empty());

    let text = dir.path().join("objects.txt");
    assert_eq!(
        kct_lint(&[
            "inspect",
            board.to_str().unwrap(),
            "-o",
            text.to_str().unwrap()
        ]),
        Ok(0)
    );
    let text = fs::read_to_string(text).unwrap();
    assert!(text.contains(&format!("tracks: {}", model.tracks.len())));
    assert!(text.contains(&format!("footprints: {}", model.parts.len())));
}

#[test]
fn kct_lint_inspect_output_cannot_replace_board() {
    let dir = tempfile::tempdir().unwrap();
    let board = bad_board(dir.path());
    let err = kct_lint(&[
        "inspect",
        board.to_str().unwrap(),
        "--format",
        "json",
        "-o",
        board.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains("refusing to overwrite"));
    assert_eq!(
        fs::read_to_string(board).unwrap(),
        include_str!("fixtures/lint/advanced-bad.kicad_pcb")
    );
}

#[test]
fn kct_lint_corpus_passes_smoke_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("evaluation.json");
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/lint/corpus.json"
    );
    assert_eq!(
        kct_lint(&["corpus", manifest, "-o", out.to_str().unwrap()]),
        Ok(0)
    );
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(out).unwrap()).unwrap();
    assert_eq!(v[0]["name"], "seventy-deliberate-faults");
    assert_eq!(v[0]["passed"], true);
}

#[test]
fn kct_lint_corpus_mismatch_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    bad_board(dir.path());
    let manifest = dir.path().join("corpus.json");
    fs::write(
        &manifest,
        r#"{"schema":1,"cases":[{"name":"too-strict","board":"board.kicad_pcb",
        "board_id":"fixture","expected":[{"rule":"trace.short_segment","min":10000,"max":10000}]}]}"#,
    )
    .unwrap();
    let out = dir.path().join("evaluation.json");
    assert_eq!(
        kct_lint(&[
            "corpus",
            manifest.to_str().unwrap(),
            "-o",
            out.to_str().unwrap()
        ]),
        Ok(2)
    );
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(v[0]["passed"], false);
    assert!(!v[0]["mismatches"].as_array().unwrap().is_empty());
    // The output may not alias a case input, and malformed manifests are errors.
    let case_board = dir.path().join("board.kicad_pcb");
    assert!(kct_lint(&[
        "corpus",
        manifest.to_str().unwrap(),
        "-o",
        case_board.to_str().unwrap()
    ])
    .is_err());
    fs::write(&manifest, r#"{"schema":2,"cases":[]}"#).unwrap();
    assert!(kct_lint(&["corpus", manifest.to_str().unwrap()])
        .unwrap_err()
        .contains("unsupported corpus schema"));
}

#[test]
fn kct_lint_init_print_default_config_needs_no_board() {
    assert_eq!(kct_lint(&["init", "--print-default-config"]), Ok(0));
    // What it prints round-trips through the strict (deny_unknown_fields) parser.
    let printed = serde_json::to_string(&kct::lint::Config::default()).unwrap();
    serde_json::from_str::<kct::lint::Config>(&printed).unwrap();
}

#[test]
fn kct_lint_run_sheets_cannot_alias_board_policy_or_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let board = bad_board(dir.path());
    let policy = dir.path().join("board.lint.json");
    let sheet = dir.path().join("sheet.html");
    for (flag, target) in [("--contact-sheet", &board), ("--exceptions-sheet", &policy)] {
        fs::write(&policy, "keep").unwrap();
        let err = kct_lint(&[
            "run",
            board.to_str().unwrap(),
            flag,
            target.to_str().unwrap(),
        ])
        .unwrap_err();
        assert!(err.contains("output aliases") || err.contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(&policy).unwrap(), "keep");
    }
    fs::remove_file(&policy).unwrap();
    let err = kct_lint(&[
        "run",
        board.to_str().unwrap(),
        "--contact-sheet",
        sheet.to_str().unwrap(),
        "--exceptions-sheet",
        sheet.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains("output aliases"));
    assert!(!sheet.exists());
    assert_eq!(
        fs::read_to_string(board).unwrap(),
        include_str!("fixtures/lint/advanced-bad.kicad_pcb")
    );
}
