//! `kct report generate --data-dir`: the advertised option is accepted and
//! its pre-collected snapshots replace live kicad-cli collection.

use std::path::PathBuf;

fn board() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drc/orphan_island.kicad_pcb")
}

fn generate(extra: &[&str], out: &std::path::Path) -> (i32, String) {
    let mut args: Vec<String> = vec![
        "report".into(),
        "generate".into(),
        board().display().to_string(),
        "--no-figures".into(),
        "--output".into(),
        out.display().to_string(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    let code = kct::cli::run(args).unwrap();
    let md = std::fs::read_to_string(out.join("v1/report.md")).unwrap();
    (code, md)
}

#[test]
fn data_dir_snapshots_feed_the_report() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir(&data).unwrap();
    // Collector envelope and flat JSON are both accepted.
    std::fs::write(
        data.join("drc_summary.json"),
        r#"{"schema_version": 1, "generated_at": "x", "data": {"error_count": 3, "warning_count": 1}}"#,
    )
    .unwrap();
    std::fs::write(
        data.join("erc_summary.json"),
        r#"{"error_count": 0, "warning_count": 2}"#,
    )
    .unwrap();
    std::fs::write(data.join("notes.txt"), "Reviewed by bench test.\n").unwrap();
    std::fs::write(data.join("metadata.json"), r#"{"revision": "B"}"#).unwrap();
    let out = tmp.path().join("reports");
    let (code, md) = generate(&["--data-dir", data.to_str().unwrap()], &out);
    assert_eq!(code, 0);
    assert!(md.contains("| DRC | 3 error(s), 1 warning(s) |"), "{md}");
    assert!(md.contains("| ERC | 0 error(s), 2 warning(s) |"), "{md}");
    assert!(md.contains("Reviewed by bench test."), "{md}");
    assert!(md.contains("| Revision | B |"), "{md}");
}

#[test]
fn missing_data_dir_is_reported_as_unknown_not_a_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("reports");
    let missing = tmp.path().join("no-such-dir");
    let (code, md) = generate(&["--data-dir", missing.to_str().unwrap()], &out);
    assert_eq!(code, 0);
    assert!(
        md.contains("| DRC | unknown (no usable drc_summary.json in --data-dir) |"),
        "{md}"
    );
}
