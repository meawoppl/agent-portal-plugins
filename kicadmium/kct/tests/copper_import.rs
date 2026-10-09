//! `kct copper import`: JSON tracks/arcs/vias, group tagging, idempotent replace.

use std::path::Path;

fn board() -> String {
    "(kicad_pcb (version 20260206) (generator \"test\") (general (thickness 1.6)) \
     (layers (0 \"F.Cu\" signal) (2 \"B.Cu\" signal) (44 \"Edge.Cuts\" user)) \
     (net 0 \"\") \
     (gr_line (start 50 50) (end 150 50) (layer \"Edge.Cuts\") (width 0.1)) \
     (gr_line (start 150 50) (end 150 150) (layer \"Edge.Cuts\") (width 0.1)) \
     (gr_line (start 150 150) (end 50 150) (layer \"Edge.Cuts\") (width 0.1)) \
     (gr_line (start 50 150) (end 50 50) (layer \"Edge.Cuts\") (width 0.1)))"
        .to_string()
}
const ITEMS: &str = r#"{"units": "mm", "items": [
  {"layer": "F.Cu", "net": "COIL", "width": 0.5, "points": [[60, 60], [80, 60], [80, 70]]},
  {"type": "arc", "layer": "F.Cu", "net": "COIL", "width": 0.5, "start": [80, 70], "mid": [85, 75], "end": [80, 80]},
  {"type": "via", "at": [80, 80], "size": 0.8, "drill": 0.4, "net": "COIL"}
]}"#;

fn run(dir: &Path, input: &Path, items: &str, extra: &[&str]) -> (i32, String) {
    let json = dir.join("items.json");
    std::fs::write(&json, items).unwrap();
    let out = dir.join("out.kicad_pcb");
    let mut argv: Vec<String> = vec![
        "copper".into(),
        "import".into(),
        input.display().to_string(),
        "--from".into(),
        json.display().to_string(),
        "--output".into(),
        out.display().to_string(),
    ];
    argv.extend(extra.iter().map(|s| s.to_string()));
    let code = kct::cli::run(argv).unwrap();
    (code, std::fs::read_to_string(out).unwrap())
}
/// Root items are written as `(tag` followed by a space or a newline.
fn count(text: &str, tag: &str) -> usize {
    text.matches(&format!("({tag} ")).count() + text.matches(&format!("({tag}\n")).count()
}

#[test]
fn imports_segments_arcs_and_vias_into_a_group() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.kicad_pcb");
    std::fs::write(&input, board()).unwrap();
    let (code, text) = run(dir.path(), &input, ITEMS, &["--group", "coil:D0"]);
    assert_eq!(code, 0);
    assert_eq!(count(&text, "segment"), 2, "polyline of three points");
    assert_eq!(count(&text, "arc"), 1);
    assert_eq!(count(&text, "via"), 1);
    assert_eq!(count(&text, "group"), 1);
    assert!(text.contains("\"coil:D0\""));
    // Absolute coordinates land where the JSON said, and the net was created.
    assert!(
        text.contains("(start 60 60)")
            && text.contains("(mid 85 75)")
            && text.contains("(at 80 80)")
    );
    assert!(text.contains("\"COIL\""));
    let pcb = kct::schema::pcb::Pcb::load(dir.path().join("out.kicad_pcb")).unwrap();
    assert_eq!(pcb.segments().len(), 2);
    assert_eq!(pcb.arcs().len(), 1);
    assert_eq!(pcb.vias().len(), 1);
}

#[test]
fn replace_is_idempotent_and_leaves_other_copper_alone() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.kicad_pcb");
    std::fs::write(&input, board()).unwrap();
    let (_, first) = run(dir.path(), &input, ITEMS, &["--group", "coil:D0"]);
    let staged = dir.path().join("staged.kicad_pcb");
    // Hand-placed copper outside the group must survive a regeneration.
    let hand = first.replace(
        "(net 0 \"\")",
        "(net 0 \"\") (segment (start 100 100) (end 110 100) (width 0.3) (layer \"B.Cu\") (net 0) (uuid \"hand\"))",
    );
    std::fs::write(&staged, &hand).unwrap();
    let (_, again) = run(
        dir.path(),
        &staged,
        ITEMS,
        &["--group", "coil:D0", "--replace"],
    );
    assert_eq!(count(&again, "segment"), 3, "2 regenerated + 1 hand-placed");
    assert_eq!(count(&again, "arc"), 1);
    assert_eq!(count(&again, "via"), 1);
    assert_eq!(count(&again, "group"), 1);
    assert!(again.contains("\"hand\""));
    // Without --replace a second import simply adds a second group.
    let (_, doubled) = run(dir.path(), &staged, ITEMS, &["--group", "coil:D0"]);
    assert_eq!(count(&doubled, "segment"), 5);
    assert_eq!(count(&doubled, "group"), 2);
}

#[test]
fn units_and_board_coordinates() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.kicad_pcb");
    std::fs::write(&input, board()).unwrap();
    // Metres, relative to the outline origin (50, 50): 0.01 m -> (60, 60).
    let items = r#"{"units": "m", "items": [{"layer": "F.Cu", "net": "N", "width": 0.0005, "points": [[0.01, 0.01], [0.02, 0.01]]}]}"#;
    let (_, text) = run(dir.path(), &input, items, &["--coords", "board"]);
    assert!(
        text.contains("(start 60 60)") && text.contains("(end 70 60)"),
        "{text}"
    );
    assert!(text.contains("(width 0.5)"));
    // Bad input is refused with a location.
    let bad = r#"[{"layer": "F.Cu", "net": "N", "width": 0.5, "start": [0, 0], "mid": [1, 1], "end": [2, 2]}]"#;
    let json = dir.path().join("bad.json");
    std::fs::write(&json, bad).unwrap();
    let err = kct::cli::run(vec![
        "copper".into(),
        "import".into(),
        input.display().to_string(),
        "--from".into(),
        json.display().to_string(),
        "--dry-run".into(),
    ])
    .unwrap_err()
    .to_string();
    assert!(err.contains("item 0"), "{err}");
}
