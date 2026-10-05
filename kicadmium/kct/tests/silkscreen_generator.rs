//! Port of upstream `tests/test_silkscreen_generator.py`.

use std::path::{Path, PathBuf};

use kct::silkscreen::generator::{gr_text_node, SilkscreenGenerator};

fn minimal_pcb(extra: &str) -> String {
    format!(
        "(kicad_pcb (version 20240108) (generator \"test\")\n  (general (thickness 1.6) (legacy_teardrops no))\n  (layers (0 \"F.Cu\" signal))\n  {extra}\n)\n"
    )
}

fn write_pcb(dir: &Path, content: &str) -> PathBuf {
    let p = dir.join("test.kicad_pcb");
    std::fs::write(&p, content).unwrap();
    p
}

fn gen_for(extra: &str) -> (tempfile::TempDir, PathBuf, SilkscreenGenerator) {
    let dir = tempfile::tempdir().unwrap();
    let p = write_pcb(dir.path(), &minimal_pcb(extra));
    let g = SilkscreenGenerator::new(&p).unwrap();
    (dir, p, g)
}

fn texts(g: &SilkscreenGenerator) -> Vec<String> {
    g.doc
        .find_all("gr_text")
        .map(|n| n.first_atom().map(|a| a.to_string()).unwrap_or_default())
        .collect()
}

fn mark(
    g: &mut SilkscreenGenerator,
    name: Option<&str>,
    rev: Option<&str>,
    date: Option<&str>,
) -> kct::silkscreen::SilkscreenResult {
    g.add_board_markings(name, rev, date, "F.SilkS", 1.0, 0.15)
}

const OUTLINE_120: &str = r#"
  (gr_line (start 100 100) (end 150 100) (stroke (width 0.1) (type default)) (layer "Edge.Cuts") (uuid "e1"))
  (gr_line (start 150 100) (end 150 120) (stroke (width 0.1) (type default)) (layer "Edge.Cuts") (uuid "e2"))
  (gr_line (start 150 120) (end 100 120) (stroke (width 0.1) (type default)) (layer "Edge.Cuts") (uuid "e3"))
  (gr_line (start 100 120) (end 100 100) (stroke (width 0.1) (type default)) (layer "Edge.Cuts") (uuid "e4"))
"#;

#[test]
fn gr_text_node_structure_and_roundtrip() {
    let s = gr_text_node("Big", 0.0, 0.0, "B.SilkS", 2.0, 0.15, "u").to_kicad_string();
    assert!(s.contains("(size 2 2)"), "{s}");
    assert!(s.contains("(layer \"B.SilkS\")"), "{s}");
    assert!(s.contains("(at 0 0)"), "{s}");
    let re = kct::parse(&s).unwrap();
    assert_eq!(re.tag(), Some("gr_text"));
}

#[test]
fn gr_text_node_matches_python_text() {
    // Captured from upstream
    // `gr_text_node('My Board Rev A', 100.555, 121.5, uuid_str='u-1').to_string()`.
    let want = "(gr_text\n\t\"My Board Rev A\"\n\t(at 100.56 121.5)\n\t(layer \"F.SilkS\")\n\t(uuid \"u-1\")\n\t(effects\n\t\t(font\n\t\t\t(size 1 1)\n\t\t\t(thickness 0.15)\n\t\t)\n\t)\n)";
    let got = gr_text_node(
        "My Board Rev A",
        100.555,
        121.5,
        "F.SilkS",
        1.0,
        0.15,
        "u-1",
    )
    .to_kicad_string();
    assert_eq!(got, want);
}

#[test]
fn unhides_hidden_fp_text_reference() {
    let (_d, _p, mut g) = gen_for(
        r#"(footprint "Package_SO:SOIC-8" (at 100 100) (layer "F.Cu")
            (fp_text reference "U1" (at 0 -2) (layer "F.SilkS") (hide yes)
              (effects (font (size 1 1) (thickness 0.15)))))"#,
    );
    let r = g.ensure_ref_des_visible();
    assert_eq!(r.refs_unhidden, 1);
    assert!(r.messages[0].contains("U1"));
    let fp = g.doc.find("footprint").unwrap();
    let ri = SilkscreenGenerator::find_ref_text(fp).unwrap();
    assert!(fp.children[ri].find("hide").is_none());
}

#[test]
fn visibility_variants() {
    for (extra, expected) in [
        (
            r#"(footprint "R:0805" (at 110 100) (layer "F.Cu")
                (fp_text reference "R1" (at 0 -2) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15)))))"#,
            0,
        ),
        (
            r#"(footprint "R:0805" (at 110 100) (layer "F.Cu")
                (fp_text reference "R2" (at 0 -2) (layer "F.SilkS") hide (effects (font (size 1 1) (thickness 0.15)))))"#,
            1,
        ),
        (
            r#"(footprint "R:0805" (at 110 100) (layer "F.Cu")
                (fp_text reference "R3" (at 0 -2) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15)) hide)))"#,
            1,
        ),
        (
            r#"(footprint "R:0805" (at 110 100) (layer "F.Cu")
                (property "Reference" "R4" (at 0 -2) (layer "F.SilkS") (hide yes) (effects (font (size 1 1) (thickness 0.15)))))"#,
            1,
        ),
        (
            r#"(footprint "R:0805" (at 110 100) (layer "F.Cu")
                (fp_text reference "R5" (at 0 -2) (layer "F.Fab") (hide yes) (effects (font (size 1 1) (thickness 0.15)))))"#,
            0,
        ),
    ] {
        let (_d, _p, mut g) = gen_for(extra);
        let r = g.ensure_ref_des_visible();
        assert_eq!(r.refs_unhidden, expected, "{extra}");
        if expected == 1 {
            let fp = g.doc.find("footprint").unwrap();
            let ri = SilkscreenGenerator::find_ref_text(fp).unwrap();
            assert!(!SilkscreenGenerator::is_hidden(&fp.children[ri]), "{extra}");
        }
    }
}

#[test]
fn adds_name_and_date_and_positions_below_outline() {
    let (_d, _p, mut g) = gen_for(OUTLINE_120);
    let r = mark(&mut g, Some("TestBoard"), Some("B"), Some("2026-04-19"));
    assert_eq!(r.markings_added, 2);
    assert_eq!(r.markings_skipped, 0);
    assert_eq!(texts(&g), ["TestBoard Rev B", "2026-04-19"]);
    let first = g.doc.find_all("gr_text").next().unwrap();
    let at = first.find("at").unwrap();
    assert_eq!(at.float_at(0), Some(100.0));
    assert_eq!(at.float_at(1), Some(121.5));
    let second = g.doc.find_all("gr_text").nth(1).unwrap();
    assert_eq!(second.find("at").unwrap().float_at(1), Some(123.0));
}

#[test]
fn idempotent_and_no_metadata() {
    let (_d, _p, mut g) = gen_for("");
    assert_eq!(
        mark(&mut g, Some("MyBoard"), Some("A"), None).markings_added,
        1
    );
    let r2 = mark(&mut g, Some("MyBoard"), Some("A"), None);
    assert_eq!((r2.markings_added, r2.markings_skipped), (0, 1));
    assert_eq!(mark(&mut g, None, None, None).markings_added, 0);
}

#[test]
fn fallback_position_without_outline() {
    let (_d, _p, mut g) = gen_for("");
    assert_eq!(g.get_marking_position(), (100.0, 115.0));
    mark(&mut g, Some("Fallback"), None, None);
    let at = g.doc.find("gr_text").unwrap().find("at").unwrap().clone();
    assert_eq!((at.float_at(0), at.float_at(1)), (Some(100.0), Some(115.0)));
}

#[test]
fn revision_bump_and_rename_replace() {
    let (_d, p, mut g) = gen_for("");
    mark(&mut g, Some("Foo"), Some("A"), None);
    g.save(None).unwrap();
    let mut g2 = SilkscreenGenerator::new(&p).unwrap();
    let r = mark(&mut g2, Some("Foo"), Some("B"), None);
    assert_eq!((r.markings_added, r.markings_skipped), (1, 0));
    assert_eq!(texts(&g2), ["Foo Rev B"]);
    g2.save(None).unwrap();
    let mut g3 = SilkscreenGenerator::new(&p).unwrap();
    mark(&mut g3, Some("Bar"), None, None);
    assert_eq!(texts(&g3), ["Bar"]);
}

#[test]
fn sidecar_survives_save_and_reload() {
    let (_d, p, mut g) = gen_for("");
    mark(&mut g, Some("Persist"), Some("1"), Some("2026-05-04"));
    g.save(None).unwrap();
    let sidecar = p.with_file_name("test.kicad_pcb.kct.json");
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
    assert_eq!(data["version"], 1);
    let mut tags: Vec<_> = data["markings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["tag"].as_str().unwrap().to_string())
        .collect();
    tags.sort();
    assert_eq!(tags, ["kct:date", "kct:name"]);
    let mut g2 = SilkscreenGenerator::new(&p).unwrap();
    let r = mark(&mut g2, Some("Persist"), Some("1"), Some("2026-05-04"));
    assert_eq!((r.markings_added, r.markings_skipped), (0, 2));
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(!text.contains("kct_"));
}

#[test]
fn missing_or_corrupt_sidecar_is_graceful() {
    let (_d, p, _g) = gen_for("");
    std::fs::write(
        p.with_file_name("test.kicad_pcb.kct.json"),
        "{this is not valid json",
    )
    .unwrap();
    let mut g = SilkscreenGenerator::new(&p).unwrap();
    assert!(g.registry().is_empty());
    assert_eq!(
        mark(&mut g, Some("AfterCorrupt"), None, None).markings_added,
        1
    );
}

#[test]
fn user_gr_text_with_colliding_text_is_not_a_marking() {
    let (_d, _p, mut g) = gen_for(
        r#"(gr_text "Foo" (at 90 90) (layer "F.SilkS") (uuid "user-added-uuid") (effects (font (size 1 1) (thickness 0.15))))"#,
    );
    assert_eq!(mark(&mut g, Some("Foo"), None, None).markings_added, 1);
    assert_eq!(texts(&g), ["Foo", "Foo"]);
}

#[test]
fn user_deleted_gr_text_re_adds_marking() {
    let (_d, p, mut g) = gen_for("");
    mark(&mut g, Some("ReAdd"), None, None);
    g.save(None).unwrap();
    g.doc.children.retain(|c| !c.has_tag("gr_text"));
    kct::core::sexp_file::save_pcb(&g.doc, &p).unwrap();
    let mut g2 = SilkscreenGenerator::new(&p).unwrap();
    let r = mark(&mut g2, Some("ReAdd"), None, None);
    assert_eq!((r.markings_added, r.markings_skipped), (1, 0));
}
