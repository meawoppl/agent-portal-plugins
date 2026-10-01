//! Port of upstream tests/test_exceptions.py.

use kct::exceptions::{class_name_to_error_code, ErrorKind, KiCadToolsError, SourcePosition};
use serde_json::json;

fn ctx(pairs: &[(&str, serde_json::Value)]) -> Vec<(String, serde_json::Value)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn basic_message() {
    let err = KiCadToolsError::base("Something went wrong");
    assert_eq!(err.to_string(), "Something went wrong");
    assert_eq!(err.message, "Something went wrong");
    assert!(err.context.is_empty());
    assert!(err.suggestions.is_empty());
}

#[test]
fn with_context_and_suggestions() {
    let err = KiCadToolsError::base("File operation failed")
        .with_context("file", "test.kicad_sch")
        .with_context("line", 42);
    let msg = err.to_string();
    assert!(msg.contains("File operation failed"));
    assert!(msg.contains("Context:"));
    assert!(msg.contains("file: test.kicad_sch"));
    assert!(msg.contains("line: 42"));

    let err = KiCadToolsError::base("Invalid format")
        .with_suggestions(["Check file encoding", "Verify file is not corrupted"]);
    let msg = err.to_string();
    assert!(msg.contains("Suggestions:"));
    assert!(msg.contains("Check file encoding"));
    assert!(msg.contains("Verify file is not corrupted"));

    let err = KiCadToolsError::base("Operation failed")
        .with_context("operation", "export")
        .with_context("target", "gerbers")
        .with_suggestions(["Install KiCad", "Check PATH"]);
    let msg = err.to_string();
    assert!(msg.contains("operation: export"));
    assert!(msg.contains("Install KiCad"));
    assert_eq!(
        msg,
        "Operation failed\n\nContext:\n  operation: export\n  target: gerbers\n\n\
         Suggestions:\n  - Install KiCad\n  - Check PATH"
    );
}

#[test]
fn error_codes() {
    assert_eq!(KiCadToolsError::base("x").error_code, "KI_CAD_TOOLS");
    assert_eq!(
        KiCadToolsError::base("x")
            .with_error_code("CUSTOM_CODE")
            .error_code,
        "CUSTOM_CODE"
    );
    assert_eq!(class_name_to_error_code("ParseError"), "PARSE");
    assert_eq!(
        class_name_to_error_code("FileNotFoundError"),
        "FILE_NOT_FOUND"
    );
    assert_eq!(class_name_to_error_code("Configuration"), "CONFIGURATION");
    assert_eq!(class_name_to_error_code("KiCadToolsError"), "KI_CAD_TOOLS");
}

#[test]
fn to_dict() {
    let d = KiCadToolsError::base("Test message").to_dict();
    assert_eq!(d["error_code"], "KI_CAD_TOOLS");
    assert_eq!(d["message"], "Test message");
    assert_eq!(d["context"], json!({}));
    assert_eq!(d["suggestions"], json!([]));

    let d = KiCadToolsError::base("Full error")
        .with_context("key", "value")
        .with_suggestion("Try this")
        .with_error_code("CUSTOM")
        .to_dict();
    assert_eq!(d["error_code"], "CUSTOM");
    assert_eq!(d["context"], json!({"key": "value"}));
    assert_eq!(d["suggestions"], json!(["Try this"]));

    let d = KiCadToolsError::base("Test")
        .with_context("file", "test.txt")
        .with_context("line", 42)
        .with_suggestions(["Fix it", "Try again"])
        .to_dict();
    let parsed: serde_json::Value = serde_json::from_str(&d.to_string()).unwrap();
    assert_eq!(parsed["message"], "Test");
    assert_eq!(parsed["context"]["line"], 42);
}

#[test]
fn parse_error() {
    let err = KiCadToolsError::parse("Unexpected token", vec![], None, None, None);
    assert!(err.to_string().contains("Unexpected token"));
    assert_eq!(err.error_code, "PARSE");

    let err = KiCadToolsError::parse(
        "Syntax error",
        vec![],
        Some(10),
        Some(25),
        Some("/path/to/file.kicad_sch"),
    );
    let msg = err.to_string();
    assert!(msg.contains("line: 10"));
    assert!(msg.contains("column: 25"));
    assert!(msg.contains("file: /path/to/file.kicad_sch"));

    let err = KiCadToolsError::parse(
        "Error",
        ctx(&[("file", json!("explicit.txt"))]),
        None,
        None,
        Some("convenience.txt"),
    );
    assert!(err.to_string().contains("explicit.txt"));
    assert!(!err.to_string().contains("convenience.txt"));
}

#[test]
fn validation_error() {
    let err = KiCadToolsError::validation(vec!["Field 'name' is required".into()]);
    let msg = err.to_string();
    assert!(msg.contains("Validation failed with 1 error(s)"));
    assert!(msg.contains("Field 'name' is required"));
    assert_eq!(err.errors, vec!["Field 'name' is required".to_string()]);
    assert_eq!(err.error_code, "VALIDATION");

    let errors: Vec<String> = [
        "Missing required field: reference",
        "Invalid value format",
        "Duplicate symbol ID",
    ]
    .map(String::from)
    .to_vec();
    let err = KiCadToolsError::validation(errors.clone());
    let msg = err.to_string();
    assert!(msg.contains("Validation failed with 3 error(s)"));
    for e in &errors {
        assert!(msg.contains(e.as_str()));
    }

    let err = KiCadToolsError::validation(vec!["Invalid format".into()])
        .with_context("file", "config.json")
        .with_context("section", "settings");
    assert!(err.to_string().contains("file: config.json"));
    assert!(err.to_string().contains("section: settings"));

    let d = KiCadToolsError::validation(vec!["Error 1".into(), "Error 2".into(), "Error 3".into()])
        .with_context("file", "test.json")
        .to_dict();
    assert_eq!(d["error_code"], "VALIDATION");
    assert_eq!(d["errors"], json!(["Error 1", "Error 2", "Error 3"]));
    assert_eq!(d["context"], json!({"file": "test.json"}));
}

#[test]
fn subclass_messages() {
    let err = KiCadToolsError::file_format("Not a KiCad schematic")
        .with_context("file", "board.kicad_pcb")
        .with_context("expected", "kicad_sch")
        .with_context("got", "kicad_pcb")
        .with_suggestion("Use a .kicad_sch file");
    let msg = err.to_string();
    assert!(msg.contains("expected: kicad_sch"));
    assert!(msg.contains("got: kicad_pcb"));

    let err = KiCadToolsError::configuration("Unknown manufacturer")
        .with_context("manufacturer", "invalid")
        .with_context("available", json!(["jlcpcb", "pcbway"]));
    let msg = err.to_string();
    assert!(msg.contains("manufacturer: invalid"));
    assert!(msg.contains("available: ['jlcpcb', 'pcbway']"));

    let err = KiCadToolsError::routing("Cannot route net").with_context("net", "GND");
    assert!(err.to_string().contains("net: GND"));
    let err =
        KiCadToolsError::export("Gerber export failed").with_context("output_dir", "/tmp/gerbers");
    assert!(err.to_string().contains("output_dir: /tmp/gerbers"));
    let err = KiCadToolsError::file_not_found("Schematic not found")
        .with_context("file", "missing.kicad_sch");
    assert!(err.to_string().contains("file: missing.kicad_sch"));
}

#[test]
fn hierarchy_and_unique_codes() {
    let all = [
        KiCadToolsError::parse("test", vec![], None, None, None),
        KiCadToolsError::validation(vec!["test".into()]),
        KiCadToolsError::file_format("test"),
        KiCadToolsError::file_not_found("test"),
        KiCadToolsError::routing("test"),
        KiCadToolsError::configuration("test"),
        KiCadToolsError::export("test"),
    ];
    let expected = [
        "PARSE",
        "VALIDATION",
        "FILE_FORMAT",
        "FILE_NOT_FOUND",
        "ROUTING",
        "CONFIGURATION",
        "EXPORT",
    ];
    for (err, code) in all.iter().zip(expected) {
        assert_eq!(err.error_code, code);
        let d = err.to_dict();
        for key in ["error_code", "message", "context", "suggestions"] {
            assert!(d.get(key).is_some());
        }
    }
    let codes: std::collections::HashSet<_> = all.iter().map(|e| e.error_code.clone()).collect();
    assert_eq!(codes.len(), all.len());

    // "Catch by base class": every kind is one error type, downcastable
    // from anyhow.
    let any: anyhow::Error = KiCadToolsError::parse("test", vec![], None, None, None).into();
    let caught = any.downcast_ref::<KiCadToolsError>().unwrap();
    assert_eq!(caught.kind, ErrorKind::Parse);

    assert_eq!(
        KiCadToolsError::file_not_found("test")
            .with_error_code("CUSTOM_NOT_FOUND")
            .error_code,
        "CUSTOM_NOT_FOUND"
    );
    assert_eq!(
        KiCadToolsError::configuration("test")
            .with_error_code("CONFIG_INVALID")
            .error_code,
        "CONFIG_INVALID"
    );
}

#[test]
fn source_position() {
    let pos = SourcePosition::new("test.kicad_sch", 42, 5);
    assert_eq!(pos.file_path, std::path::PathBuf::from("test.kicad_sch"));
    assert_eq!((pos.line, pos.column), (42, 5));
    assert!(pos.element_type.is_empty() && pos.element_ref.is_empty());
    assert!(pos.position_mm.is_none() && pos.layer.is_none());

    assert_eq!(
        SourcePosition::new("project.kicad_sch", 42, 15).to_string(),
        "project.kicad_sch:42:15"
    );

    let pos = SourcePosition {
        element_type: "symbol".into(),
        element_ref: "U1".into(),
        ..SourcePosition::new("board.kicad_pcb", 10, 5)
    };
    let r = pos.repr();
    assert!(r.contains("SourcePosition"));
    assert!(r.contains("board.kicad_pcb"));
    assert!(r.contains("line=10"));
    assert!(r.contains("column=5"));
    assert!(r.contains("element_type='symbol'"));
    assert!(r.contains("element_ref='U1'"));

    let d = SourcePosition::new("test.kicad_sch", 42, 5).to_dict();
    assert_eq!(d["file_path"], "test.kicad_sch");
    assert_eq!(d["line"], 42);
    assert_eq!(d["column"], 5);
    for key in ["element_type", "element_ref", "position_mm", "layer"] {
        assert!(d.get(key).is_none());
    }

    let pos = SourcePosition {
        element_type: "via".into(),
        element_ref: "VIA-1".into(),
        position_mm: Some((12.7, 25.4)),
        layer: Some("F.Cu".into()),
        ..SourcePosition::new("board.kicad_pcb", 100, 10)
    };
    let d = pos.to_dict();
    assert_eq!(d["element_type"], "via");
    assert_eq!(d["element_ref"], "VIA-1");
    assert_eq!(d["position_mm"], json!({"x": 12.7, "y": 25.4}));
    assert_eq!(d["layer"], "F.Cu");
}

#[test]
fn rendering() {
    let r = KiCadToolsError::base("Test error message").render();
    assert!(r.contains("Test error message") && r.contains("KI_CAD_TOOLS"));

    let r = KiCadToolsError::base("File error")
        .with_context("file", "test.kicad_sch")
        .with_context("line", 42)
        .render();
    assert!(r.contains("test.kicad_sch") && r.contains("42") && r.contains("line"));

    let r = KiCadToolsError::base("Configuration error")
        .with_suggestions(["Check your config file", "Verify settings"])
        .render();
    assert!(r.contains("Suggestions") && r.contains("Verify settings"));

    let r = KiCadToolsError::parse(
        "Syntax error",
        ctx(&[
            ("file", json!("test.kicad_sch")),
            ("line", json!(3)),
            (
                "source_snippet",
                json!("(kicad_sch\n  (version 20230121)\n  (bad_token here)\n)"),
            ),
            ("highlight_line", json!(3)),
        ]),
        None,
        None,
        None,
    )
    .render();
    assert!(r.contains("Syntax error") && r.contains("bad_token"));

    let r = KiCadToolsError::validation(vec![
        "Missing field: name".into(),
        "Invalid value: type".into(),
    ])
    .with_context("file", "config.json")
    .render();
    assert!(r.contains("2 error") && r.contains("Missing field: name"));

    let plain = KiCadToolsError::base("Test error")
        .with_context("key", "value")
        .with_suggestion("Try this")
        .to_string();
    assert!(plain.contains("Context:") && plain.contains("key: value"));
    assert!(plain.contains("Suggestions:") && plain.contains("Try this"));
}
