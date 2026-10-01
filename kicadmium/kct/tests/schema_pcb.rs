//! Ports of upstream `tests/test_pcb.py`, `test_pcb_attr_roundtrip.py`,
//! `test_footprint_locked_form.py`, `test_added_footprint_pad_mutations.py`,
//! `test_pcb_net_class.py`, `test_pcb_edit_outline.py` and the schema-level
//! parts of `test_pcb_copper_arcs.py`.

use std::path::{Path, PathBuf};

use kct::schema::pcb::*;
use kct::SExp;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fx(name: &str) -> PathBuf {
    fixtures().join("schema_pcb").join(name)
}

fn load(name: &str) -> Pcb {
    Pcb::load(fx(name)).unwrap()
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn save_reload(pcb: &mut Pcb, dir: &Path, name: &str) -> Pcb {
    let out = dir.join(name);
    pcb.save(Some(&out)).unwrap();
    Pcb::load(&out).unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn close2(a: (f64, f64), b: (f64, f64)) -> bool {
    close(a.0, b.0) && close(a.1, b.1)
}

fn create(w: f64, h: f64) -> Pcb {
    Pcb::create(CreateOptions::size(w, h)).unwrap()
}

fn net_number(pcb: &Pcb, name: &str) -> i64 {
    pcb.get_net_by_name(name).unwrap().number
}

fn edge_lines(pcb: &Pcb) -> Vec<((f64, f64), (f64, f64))> {
    pcb.sexp()
        .children
        .iter()
        .filter(|c| c.has_tag("gr_line"))
        .filter(|c| c.find("layer").and_then(|l| l.text_at(0)).as_deref() == Some("Edge.Cuts"))
        .map(|c| {
            let s = c.find("start").unwrap();
            let e = c.find("end").unwrap();
            (
                (s.float_at(0).unwrap(), s.float_at(1).unwrap()),
                (e.float_at(0).unwrap(), e.float_at(1).unwrap()),
            )
        })
        .collect()
}

fn outline_bbox(pcb: &Pcb) -> ((f64, f64), (f64, f64)) {
    let segs = edge_lines(pcb);
    let xs: Vec<f64> = segs.iter().flat_map(|s| [s.0 .0, s.1 .0]).collect();
    let ys: Vec<f64> = segs.iter().flat_map(|s| [s.0 .1, s.1 .1]).collect();
    let min = |v: &[f64]| v.iter().cloned().fold(f64::MAX, f64::min);
    let max = |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max);
    ((min(&xs), min(&ys)), (max(&xs), max(&ys)))
}

fn trace(pcb: &mut Pcb, s: (f64, f64), e: (f64, f64), layer: &str, net: &str) -> Vec<Segment> {
    pcb.add_trace(
        s,
        e,
        TraceOptions {
            layer: layer.into(),
            net: Some(net.into()),
            ..Default::default()
        },
    )
    .unwrap()
}

fn via(pcb: &mut Pcb, x: f64, y: f64, layers: &[&str], net: &str) -> Option<Via> {
    pcb.add_via(
        x,
        y,
        ViaOptions {
            layers: layers.iter().map(|s| s.to_string()).collect(),
            net: Some(net.into()),
            ..Default::default()
        },
    )
}

// ------------------------------------------------------------------ basics

#[test]
fn parse_layers_nets_footprints_pads_traces() {
    let pcb = load("minimal.kicad_pcb");
    let names: Vec<&str> = pcb.layers().iter().map(|l| l.name.as_str()).collect();
    assert!(names.contains(&"F.Cu") && names.contains(&"B.Cu"));
    assert!(pcb.nets().len() >= 2);
    assert!(pcb.get_net_by_name("GND").is_some());
    assert!(pcb.get_net_by_name("+3.3V").is_some());
    assert!(pcb.get_net_by_name("NONEXISTENT_NET").is_none());
    assert_eq!(pcb.footprints().len(), 1);
    let fp = &pcb.footprints()[0];
    assert_eq!(fp.name, "Resistor_SMD:R_0402_1005Metric");
    assert_eq!(fp.pads.len(), 2);
    assert_eq!(fp.pads[0].number, "1");
    assert_eq!(fp.pads[0].net(), 1);
    assert_eq!(fp.pads[0].net_name, "GND");
    assert_eq!(fp.pads[1].net_number, 2);
    assert_eq!(fp.pads[1].net_name, "+3.3V");
    assert_eq!(pcb.segments().len(), 1);
    assert_eq!(pcb.segments()[0].net_number, 1);
    assert_eq!(pcb.segments()[0].layer, "F.Cu");
    assert_eq!(pcb.segments()[0].net_name, "GND"); // number-only ref resolved
    assert!(!pcb.net_name_only_dialect());
    assert!(pcb.parse_warnings.is_empty());
    let setup = pcb.setup().unwrap();
    assert_eq!(setup.stackup.len(), 9);
    assert_eq!(setup.stackup[4].material, "FR4");
    assert!(close(setup.stackup[4].epsilon_r, 4.5));
    assert_eq!(pcb.copper_layers().len(), 2);
}

#[test]
fn zone_parsing() {
    let pcb = load("zone_test.kicad_pcb");
    let z = pcb.zones();
    assert_eq!(z.len(), 3);
    assert_eq!(z[0].net_number, 1);
    assert_eq!(z[0].net_name, "GND");
    assert_eq!(z[0].layer, "F.Cu");
    assert_eq!(z[0].uuid, "zone-uuid-1");
    assert_eq!(z[0].name, "GND_Zone");
    assert_eq!((z[0].priority, z[1].priority, z[2].priority), (1, 0, 0));
    assert!(close(z[0].min_thickness, 0.15) && close(z[1].min_thickness, 0.2));
    assert!(close(z[0].clearance, 0.25) && close(z[1].clearance, 0.2));
    assert!(close(z[0].thermal_gap, 0.4) && close(z[0].thermal_bridge_width, 0.35));
    assert!(close(z[1].thermal_gap, 0.3) && close(z[1].thermal_bridge_width, 0.3));
    assert_eq!(z[0].connect_pads, "thermal_reliefs");
    assert_eq!(z[1].connect_pads, "solid");
    assert_eq!(z[2].connect_pads, "none");
    assert!(z[0].is_filled && !z[1].is_filled && z[2].is_filled);
    assert_eq!(z[0].polygon, vec![(100.0, 100.0), (130.0, 100.0), (130.0, 120.0), (100.0, 120.0)]);
    assert_eq!(z[2].polygon.len(), 6);
    assert_eq!(z[2].polygon[5], (140.0, 120.0));
    assert_eq!(z[0].filled_polygons.len(), 1);
    assert_eq!(z[0].filled_polygons[0][0], (100.1, 100.1));
    assert_eq!(z[0].filled_polygon_layer(0), "F.Cu");
    assert!(z[1].filled_polygons.is_empty());
    assert_eq!(z[2].fill_type, "solid");
    // filled_areas_thickness no forces solid even on a legacy version.
    assert_eq!(z[0].filled_areas_thickness, Some(false));
    assert!(!z[0].is_stroked_fill());
    // Absent token at version 20240108 (<= 20250209) means stroked.
    assert!(z[1].is_stroked_fill());
    assert!(close(z[1].fill_inflation(), 0.1));
    assert_eq!(pcb.zone_count(), 3);
    assert!(pcb.rule_areas().is_empty());
}

#[test]
fn rule_area_keepout() {
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (version 20260206)
        (zone (net 0) (net_name "") (layers "F.Cu" "B.Cu") (name "KO")
          (keepout (tracks not_allowed) (vias allowed) (pads not_allowed))
          (polygon (pts (xy 0 0) (xy 1 0) (xy 1 1)))))"#,
    )
    .unwrap();
    let areas = pcb.rule_areas();
    assert_eq!(areas.len(), 1);
    let k = areas[0].keepout.as_ref().unwrap();
    assert!(!k.tracks_allowed && k.vias_allowed && !k.pads_allowed && k.copperpour_allowed);
    assert_eq!(areas[0].layers, vec!["F.Cu", "B.Cu"]);
    assert_eq!(areas[0].layer, "F.Cu");
    assert!(!areas[0].is_stroked_fill());
}

#[test]
fn footprint_queries() {
    let pcb = load("routing_test.kicad_pcb");
    assert_eq!(pcb.get_footprint("R1").unwrap().reference, "R1");
    assert!(pcb.get_footprint("U99").is_none());
    // gr_rect outline at (100,100): positions are board-relative.
    assert_eq!(pcb.board_origin(), (100.0, 100.0));
    assert_eq!(pcb.get_footprint("R1").unwrap().position, (35.0, 15.0));
    assert!(close2(pcb.board_size().unwrap(), (50.0, 40.0)));
    let j1 = pcb.get_footprint("J1").unwrap();
    assert_eq!(j1.pads[0].pad_type, "thru_hole");
    assert!(close(j1.pads[0].drill, 1.0));
    assert_eq!(pcb.footprints_on_layer("F.Cu").count(), 3);
}

// ------------------------------------------------------------ positions

#[test]
fn update_footprint_position_persists() {
    let dir = tmp();
    let mut pcb = load("routing_test.kicad_pcb");
    assert!(pcb.update_footprint_position("R1", 140.0, 120.0, None));
    assert!(close2(pcb.get_footprint("R1").unwrap().position, (140.0, 120.0)));
    let pcb2 = save_reload(&mut pcb, dir.path(), "out.kicad_pcb");
    assert!(close2(pcb2.get_footprint("R1").unwrap().position, (140.0, 120.0)));
    assert!(!pcb.update_footprint_position("NONEXISTENT", 1.0, 1.0, None));
}

#[test]
fn update_footprint_rotation_existing_and_new() {
    let dir = tmp();
    let mut pcb = load("routing_test.kicad_pcb");
    assert!(close(pcb.get_footprint("R1").unwrap().rotation, 0.0));
    pcb.update_footprint_position("R1", 135.0, 115.0, Some(45.0));
    let mut pcb2 = save_reload(&mut pcb, dir.path(), "a.kicad_pcb");
    assert!(close(pcb2.get_footprint("R1").unwrap().rotation, 45.0));
    pcb2.update_footprint_position("R1", 140.0, 120.0, Some(90.0));
    let pcb3 = save_reload(&mut pcb2, dir.path(), "b.kicad_pcb");
    let fp = pcb3.get_footprint("R1").unwrap();
    assert!(close2(fp.position, (140.0, 120.0)));
    assert!(close(fp.rotation, 90.0));
}

#[test]
fn direct_setters_persist() {
    let dir = tmp();
    let mut pcb = load("routing_test.kicad_pcb");
    let orig = pcb.get_footprint("R1").unwrap().position;
    {
        let mut fm = pcb.footprint_mut("R1").unwrap();
        fm.set_position((orig.0 + 5.0, orig.1 + 3.0));
        fm.set_rotation(90.0);
        fm.set_layer("B.Cu");
    }
    {
        let mut u1 = pcb.footprint_mut("U1").unwrap();
        u1.set_position((120.0, 120.0));
        u1.set_rotation(45.0);
    }
    let pcb2 = save_reload(&mut pcb, dir.path(), "direct.kicad_pcb");
    let r1 = pcb2.get_footprint("R1").unwrap();
    assert!(close2(r1.position, (orig.0 + 5.0, orig.1 + 3.0)));
    assert!(close(r1.rotation, 90.0));
    assert_eq!(r1.layer, "B.Cu");
    let u1 = pcb2.get_footprint("U1").unwrap();
    assert!(close2(u1.position, (120.0, 120.0)));
    assert!(close(u1.rotation, 45.0));
}

#[test]
fn rotation_round_trip_leaves_no_residue() {
    let mut pcb = load("routing_test.kicad_pcb");
    let mut fm = pcb.footprint_mut("R1").unwrap();
    fm.set_rotation(90.0);
    fm.set_rotation(0.0);
    let text = pcb.sexp().to_kicad_string();
    assert!(text.contains("(at 135 115)"), "{text}");
}

#[test]
fn unlinked_footprint_fields_are_plain() {
    let mut fp = Footprint::new("Test", "F.Cu", (10.0, 20.0), 0.0, "R1", "10k");
    fp.position = (30.0, 40.0);
    fp.rotation = 90.0;
    fp.layer = "B.Cu".into();
    assert_eq!((fp.position, fp.rotation, fp.layer.as_str()), ((30.0, 40.0), 90.0, "B.Cu"));
}

// ------------------------------------------------------------ references

#[test]
fn rename_reference_kicad8_and_kicad7() {
    let dir = tmp();
    let mut pcb = load("minimal.kicad_pcb");
    assert!(pcb.update_footprint_reference("R1", "R100"));
    assert!(pcb.get_footprint("R1").is_none());
    let fp = pcb.get_footprint("R100").unwrap();
    assert_eq!(fp.texts.iter().find(|t| t.text_type == "reference").unwrap().text, "R100");
    let pcb2 = save_reload(&mut pcb, dir.path(), "a.kicad_pcb");
    assert!(pcb2.get_footprint("R1").is_none());
    assert!(pcb2.get_footprint("R100").is_some());

    let mut pcb = load("routing_test.kicad_pcb");
    assert!(pcb.update_footprint_reference("R1", "R50"));
    let pcb2 = save_reload(&mut pcb, dir.path(), "b.kicad_pcb");
    assert!(pcb2.get_footprint("R50").is_some());
    assert!(pcb2.get_footprint("R1").is_none());
}

#[test]
fn rename_reference_edge_cases() {
    let mut pcb = load("routing_test.kicad_pcb");
    assert!(!pcb.update_footprint_reference("R1", "U1"));
    assert_eq!(pcb.get_footprint("R1").unwrap().reference, "R1");
    assert!(!pcb.update_footprint_reference("NONEXISTENT", "R99"));
    assert!(pcb.update_footprint_reference("R1", "R1"));
}

#[test]
fn update_value_both_formats() {
    let dir = tmp();
    let mut pcb = load("minimal.kicad_pcb");
    let orig = pcb.get_footprint("R1").unwrap().position;
    assert_eq!(pcb.get_footprint("R1").unwrap().value, "10k");
    assert!(pcb.update_footprint_value("R1", "4.7k"));
    let fp = pcb.get_footprint("R1").unwrap();
    assert_eq!(fp.value, "4.7k");
    assert_eq!(fp.texts.iter().find(|t| t.text_type == "value").unwrap().text, "4.7k");
    let pcb2 = save_reload(&mut pcb, dir.path(), "a.kicad_pcb");
    assert_eq!(pcb2.get_footprint("R1").unwrap().value, "4.7k");
    assert!(close2(pcb2.get_footprint("R1").unwrap().position, orig));
    assert!(!pcb.update_footprint_value("NONEXISTENT", "x"));

    let k7 = r#"(kicad_pcb
  (version 20171130)
  (generator "test")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
  (net 0 "")
  (net 1 "GND")
  (gr_rect (start 100 100) (end 150 140) (stroke (width 0.1) (type default)) (fill none) (layer "Edge.Cuts"))
  (footprint "Resistor_SMD:R_0402_1005Metric"
    (layer "F.Cu")
    (uuid "00000000-0000-0000-0000-000000000001")
    (at 120 120)
    (fp_text reference "R1" (at 0 -1.5) (layer "F.SilkS"))
    (fp_text value "10k" (at 0 1.5) (layer "F.Fab"))
    (pad "1" smd roundrect (at -0.51 0) (size 0.54 0.64) (layers "F.Cu" "F.Paste" "F.Mask") (net 1 "GND"))
  )
)"#;
    let p = write(dir.path(), "k7.kicad_pcb", k7);
    let mut pcb = Pcb::load(&p).unwrap();
    assert!(pcb.update_footprint_value("R1", "4.7k"));
    let pcb2 = save_reload(&mut pcb, dir.path(), "k7out.kicad_pcb");
    assert_eq!(pcb2.get_footprint("R1").unwrap().value, "4.7k");
}

// ------------------------------------------------------------ add footprint

fn lib(name: &str) -> PathBuf {
    fixtures().join("Test_Library.pretty").join(name)
}

#[test]
fn add_footprint_from_file_basics() {
    let dir = tmp();
    let mut pcb = load("minimal.kicad_pcb");
    let n = pcb.footprints().len();
    let fp = pcb
        .add_footprint_from_file(lib("C_0402_1005Metric.kicad_mod"), "C1", 50.0, 30.0, 90.0, "B.Cu", "100nF")
        .unwrap()
        .clone();
    assert_eq!(fp.reference, "C1");
    assert_eq!(fp.value, "100nF");
    assert_eq!(fp.layer, "B.Cu");
    assert_eq!(fp.pads.len(), 2);
    assert!(close2(fp.position, (50.0, 30.0)));
    assert!(close(fp.rotation, 90.0));
    assert_eq!(pcb.footprints().len(), n + 1);
    let fp2 = pcb
        .add_footprint_from_file(lib("SOT-23-5.kicad_mod"), "U1", 80.0, 25.0, 0.0, "F.Cu", "LM317")
        .unwrap()
        .clone();
    assert_eq!(fp2.pads.len(), 5);
    assert_ne!(fp.uuid, fp2.uuid);
    assert!(!fp.uuid.is_empty());
    assert!(pcb.get_footprint("U1").is_some());
    let pcb2 = save_reload(&mut pcb, dir.path(), "out.kicad_pcb");
    let c1 = pcb2.get_footprint("C1").unwrap();
    assert_eq!(c1.value, "100nF");
    assert!(close2(c1.position, (50.0, 30.0)));
    assert!(pcb
        .add_footprint_from_file("/nonexistent/fp.kicad_mod", "C9", 0.0, 0.0, 0.0, "F.Cu", "")
        .is_err());
}

#[test]
fn add_footprint_at_node_after_layer() {
    let dir = tmp();
    let mut pcb = load("minimal.kicad_pcb");
    pcb.add_footprint_from_file(lib("C_0402_1005Metric.kicad_mod"), "C1", 50.0, 30.0, 45.0, "F.Cu", "")
        .unwrap();
    let out = dir.path().join("o.kicad_pcb");
    pcb.save(Some(&out)).unwrap();
    let content = std::fs::read_to_string(&out).unwrap();
    let start = content.find("(footprint \"C_0402_1005Metric\"").unwrap();
    let fp = &content[start..(start + 2000).min(content.len())];
    let layer = fp.find("(layer \"F.Cu\")").unwrap();
    let at = fp.find("(at 50 30 45)").unwrap();
    assert!(at > layer);
    assert!(at < fp.find("(descr").unwrap());
    assert!(at < fp.find("(tags").unwrap());
}

#[test]
fn add_footprint_pad_angles_absolute() {
    let rp = fixtures().join("RotatedPad_Test.pretty/RotatedPad_Test.kicad_mod");
    for (rot, a1, a2) in [(0.0, 0.0, 30.0), (90.0, 90.0, 120.0), (-90.0, 270.0, 300.0)] {
        let mut pcb = load("minimal.kicad_pcb");
        let fp = pcb.add_footprint_from_file(&rp, "U1", 50.0, 30.0, rot, "F.Cu", "").unwrap();
        assert!(close(fp.pads[0].rotation, a1), "{rot}");
        assert!(close(fp.pads[1].rotation, a2), "{rot}");
    }
    let dir = tmp();
    let mut pcb = load("minimal.kicad_pcb");
    pcb.add_footprint_from_file(&rp, "U1", 50.0, 30.0, 90.0, "F.Cu", "").unwrap();
    let out = dir.path().join("abs.kicad_pcb");
    pcb.save(Some(&out)).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("(at 1 0 120)"), "{text}");
    assert!(text.contains("(at -1 0 90)"), "{text}");
    let re = Pcb::load(&out).unwrap();
    let u1 = re.get_footprint("U1").unwrap();
    assert!(close(u1.pads[0].rotation, 90.0) && close(u1.pads[1].rotation, 120.0));
}

#[test]
fn add_footprint_numeric_properties_quoted() {
    let dir = tmp();
    for value in ["470", "0", "100", "-5", "3.3"] {
        let mut pcb = load("minimal.kicad_pcb");
        pcb.add_footprint_from_file(lib("C_0402_1005Metric.kicad_mod"), "1", 50.0, 30.0, 0.0, "F.Cu", value)
            .unwrap();
        let out = dir.path().join("q.kicad_pcb");
        pcb.save(Some(&out)).unwrap();
        let c = std::fs::read_to_string(&out).unwrap();
        assert!(c.contains(&format!("(property \"Value\" \"{value}\"")), "{value}");
        assert!(!c.contains(&format!("(property \"Value\" {value}")));
        assert!(c.contains("(property \"Reference\" \"1\""));
        assert!(!c.contains("(size \"1\" \"1\")") && !c.contains("(thickness \"0.15\")"));
    }
    let fpw = write(
        dir.path(),
        "R.kicad_mod",
        r#"(footprint "Test:R_Numeric" (layer "F.Cu") (uuid "00000000-0000-0000-0000-00000000aaaa")
    (property "Reference" "REF**" (at 0 -1.5 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15))))
    (property "Value" "Test:R_Numeric" (at 0 1.5 0) (layer "F.Fab") (effects (font (size 1 1) (thickness 0.15))))
    (pad "1" smd rect (at -1 0) (size 1 1) (layers "F.Cu"))
    (pad "2" smd rect (at 1 0) (size 1 1) (layers "F.Cu")))"#,
    );
    let mut pcb = load("minimal.kicad_pcb");
    pcb.add_footprint_from_file(&fpw, "R1", 50.0, 30.0, 0.0, "F.Cu", "470").unwrap();
    let out = dir.path().join("e.kicad_pcb");
    pcb.save(Some(&out)).unwrap();
    let c = std::fs::read_to_string(&out).unwrap();
    assert!(c.contains("(property \"Value\" \"470\""));
}

const REPEATED_LIB: &str = r#"(footprint "repeated" (layer "F.Cu")
      (pad "SH" smd rect (at -2 0) (size 2 1) (layers "F.Cu" "F.Paste" "F.Mask"))
      (pad "SH" smd rect (at 2 0 30) (size 2 1) (layers "F.Cu" "F.Paste" "F.Mask"))
      (pad "" np_thru_hole circle (at 0 3) (size 1 1) (drill 1) (layers "*.Cu" "*.Mask")))"#;

#[test]
fn added_pad_geometry_survives_save() {
    for reload_first in [false, true] {
        for initial in [0.0, 90.0] {
            let dir = tmp();
            let library = write(dir.path(), "repeated.kicad_mod", REPEATED_LIB);
            let mut board = create(20.0, 20.0);
            board
                .add_footprint_from_file(&library, "J1", 10.0, 10.0, initial, "F.Cu", "")
                .unwrap();
            let path = dir.path().join("board.kicad_pcb");
            if reload_first {
                board.save(Some(&path)).unwrap();
                board = Pcb::load(&path).unwrap();
            }
            let rots: Vec<f64> = board.get_footprint("J1").unwrap().pads.iter().map(|p| p.rotation).collect();
            assert_eq!(rots, vec![initial, 30.0 + initial, initial]);
            {
                let mut fm = board.footprint_mut("J1").unwrap();
                for i in 0..fm.pad_count() {
                    let mut pm = fm.pad_mut(i).unwrap();
                    let (r, p) = (pm.rotation, pm.position);
                    pm.set_rotation(r + 90.0);
                    pm.set_position((p.0, p.1 + i as f64 + 1.0));
                }
                fm.pad_mut(0).unwrap().set_layers(&["B.Cu", "B.Paste", "B.Mask"]);
            }
            board.save(Some(&path)).unwrap();
            let restored = Pcb::load(&path).unwrap();
            let fp = restored.get_footprint("J1").unwrap();
            let rots: Vec<f64> = fp.pads.iter().map(|p| p.rotation).collect();
            assert_eq!(rots, vec![90.0 + initial, 120.0 + initial, 90.0 + initial]);
            let pos: Vec<(f64, f64)> = fp.pads.iter().map(|p| p.position).collect();
            assert_eq!(pos, vec![(-2.0, 1.0), (2.0, 2.0), (0.0, 6.0)]);
            assert_eq!(fp.pads[0].layers, vec!["B.Cu", "B.Paste", "B.Mask"]);
            assert_eq!(fp.pads[1].layers, vec!["F.Cu", "F.Paste", "F.Mask"]);
        }
    }
}

#[test]
fn added_pad_ids_are_unique_and_persistent() {
    let dir = tmp();
    let old = "11111111-1111-4111-8111-111111111111";
    let library = write(
        dir.path(),
        "ids.kicad_mod",
        &format!(
            r#"(footprint "identities" (layer "F.Cu")
      (pad "SH" smd rect (at -2 0) (size 2 1) (layers "F.Cu") (uuid "{old}"))
      (pad "SH" smd rect (at 2 0) (size 2 1) (layers "F.Cu") (tstamp "{old}"))
      (pad "" np_thru_hole circle (at 0 3) (size 1 1) (drill 1) (layers "*.Cu")))"#
        ),
    );
    let mut board = create(30.0, 20.0);
    for (r, x) in [("J1", 8.0), ("J2", 22.0)] {
        board.add_footprint_from_file(&library, r, x, 10.0, 0.0, "F.Cu", "").unwrap();
    }
    let pad_ids: Vec<String> = board.footprints().iter().flat_map(|f| f.pads.iter().map(|p| p.uuid.clone())).collect();
    let fp_ids: Vec<String> = board.footprints().iter().map(|f| f.uuid.clone()).collect();
    let mut all: Vec<String> = fp_ids.iter().chain(&pad_ids).cloned().collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 8);
    assert!(!pad_ids.contains(&old.to_string()) && !pad_ids.contains(&String::new()));
    let path = dir.path().join("b.kicad_pcb");
    board.save(Some(&path)).unwrap();
    let restored = Pcb::load(&path).unwrap();
    let again: Vec<String> = restored.footprints().iter().flat_map(|f| f.pads.iter().map(|p| p.uuid.clone())).collect();
    assert_eq!(again, pad_ids);
    assert!(!std::fs::read_to_string(&path).unwrap().contains("tstamp"));
}

// ------------------------------------------------------------ silkscreen

const PCB_WITH_REFS: &str = r#"(kicad_pcb
  (version 20240108)
  (generator "test")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (37 "F.SilkS" user "F.Silkscreen") (49 "F.Fab" user) (44 "Edge.Cuts" user))
  (net 0 "")
  (net 1 "GND")
  (footprint "Resistor_SMD:R_0402" (layer "F.Cu") (uuid "00000000-0000-0000-0000-000000000001") (at 100 100)
    (property "Reference" "R1" (at 0 -1.5 0) (layer "F.SilkS") (uuid "ref-r1") (effects (font (size 1 1) (thickness 0.15))))
    (property "Value" "10k" (at 0 1.5 0) (layer "F.Fab") (uuid "val-r1"))
    (pad "1" smd roundrect (at -0.5 0) (size 0.5 0.6) (layers "F.Cu" "F.Paste" "F.Mask") (net 1 "GND")))
  (footprint "Capacitor_SMD:C_0402" (layer "F.Cu") (uuid "00000000-0000-0000-0000-000000000002") (at 110 100)
    (property "Reference" "C1" (at 0 -1.5 0) (layer "F.SilkS") (uuid "ref-c1") (effects (font (size 1 1) (thickness 0.15))))
    (property "Value" "100nF" (at 0 1.5 0) (layer "F.Fab") (uuid "val-c1")))
  (footprint "Capacitor_SMD:C_0402" (layer "F.Cu") (uuid "00000000-0000-0000-0000-000000000003") (at 120 100)
    (property "Reference" "C2" (at 0 -1.5 0) (layer "F.SilkS") (uuid "ref-c2") (effects (font (size 1 1) (thickness 0.15))))
    (property "Value" "10uF" (at 0 1.5 0) (layer "F.Fab") (uuid "val-c2")))
  (footprint "Package_SO:SOIC-8" (layer "F.Cu") (uuid "00000000-0000-0000-0000-000000000004") (at 100 120)
    (property "Reference" "U1" (at 0 -4 0) (layer "F.SilkS") (uuid "ref-u1") (effects (font (size 1 1) (thickness 0.15))))
    (property "Value" "LM358" (at 0 4 0) (layer "F.Fab") (uuid "val-u1")))
)"#;

fn ref_text(pcb: &Pcb, r: &str) -> FootprintText {
    pcb.get_footprint(r)
        .unwrap()
        .texts
        .iter()
        .find(|t| t.text_type == "reference")
        .unwrap()
        .clone()
}

#[test]
fn reference_visibility() {
    let mut pcb = Pcb::parse_str(PCB_WITH_REFS).unwrap();
    assert_eq!(pcb.set_reference_visibility(None, false, None), 4);
    assert!(["R1", "C1", "C2", "U1"].iter().all(|r| ref_text(&pcb, r).hidden));
    assert_eq!(pcb.set_reference_visibility(Some("U1"), true, None), 1);
    assert!(!ref_text(&pcb, "U1").hidden && ref_text(&pcb, "R1").hidden);

    let mut pcb = Pcb::parse_str(PCB_WITH_REFS).unwrap();
    assert_eq!(pcb.set_reference_visibility(Some("R1"), false, None), 1);
    assert!(ref_text(&pcb, "R1").hidden && !ref_text(&pcb, "C1").hidden);
    let mut pcb = Pcb::parse_str(PCB_WITH_REFS).unwrap();
    assert_eq!(pcb.set_reference_visibility(None, false, Some("C*")), 2);
    assert!(ref_text(&pcb, "C2").hidden && !ref_text(&pcb, "U1").hidden);
}

#[test]
fn move_reference_variants() {
    let mut pcb = Pcb::parse_str(PCB_WITH_REFS).unwrap();
    let orig = ref_text(&pcb, "R1").position;
    assert!(pcb.move_reference("R1", (2.0, -3.0), None, None));
    assert!(close2(ref_text(&pcb, "R1").position, (orig.0 + 2.0, orig.1 - 3.0)));
    assert!(pcb.move_reference("C1", (0.0, 0.0), Some((5.0, -4.0)), None));
    assert!(close2(ref_text(&pcb, "C1").position, (5.0, -4.0)));
    assert!(pcb.move_reference("U1", (0.0, 0.0), None, Some("F.Fab")));
    assert_eq!(ref_text(&pcb, "U1").layer, "F.Fab");
    assert!(!pcb.move_reference("NONEXISTENT", (1.0, 1.0), None, None));
}

#[test]
fn silkscreen_font_and_layer() {
    let mut pcb = Pcb::parse_str(PCB_WITH_REFS).unwrap();
    assert_eq!(pcb.set_silkscreen_font((0.8, 0.8), 0.12, None, &["reference"]), 4);
    assert!(close2(ref_text(&pcb, "U1").font_size, (0.8, 0.8)));
    assert!(close(ref_text(&pcb, "U1").font_thickness, 0.12));
    let mut pcb = Pcb::parse_str(PCB_WITH_REFS).unwrap();
    assert_eq!(pcb.set_silkscreen_font((0.6, 0.6), 0.1, Some("C*"), &["reference"]), 2);
    assert!(close2(ref_text(&pcb, "C1").font_size, (0.6, 0.6)));
    assert!(close2(ref_text(&pcb, "R1").font_size, (1.0, 1.0)));
    let mut pcb = Pcb::parse_str(PCB_WITH_REFS).unwrap();
    assert_eq!(pcb.set_silkscreen_font((0.7, 0.9), 0.15, None, &["reference"]), 4);
    assert!(close2(ref_text(&pcb, "R1").font_size, (0.7, 0.9)));
    let mut pcb = Pcb::parse_str(PCB_WITH_REFS).unwrap();
    assert_eq!(pcb.move_references_to_layer("F.Fab", None), 4);
    assert_eq!(ref_text(&pcb, "C2").layer, "F.Fab");
    let mut pcb = Pcb::parse_str(PCB_WITH_REFS).unwrap();
    assert_eq!(pcb.move_references_to_layer("F.Fab", Some("R*")), 1);
    assert_eq!(ref_text(&pcb, "R1").layer, "F.Fab");
    assert_eq!(ref_text(&pcb, "C1").layer, "F.SilkS");
}

#[test]
fn silkscreen_changes_persist() {
    let dir = tmp();
    let mut pcb = Pcb::parse_str(PCB_WITH_REFS).unwrap();
    pcb.set_reference_visibility(Some("R1"), false, None);
    pcb.move_reference("C1", (1.0, -2.0), None, None);
    pcb.set_silkscreen_font((0.9, 0.9), 0.12, Some("U*"), &["reference"]);
    let pcb2 = save_reload(&mut pcb, dir.path(), "silk.kicad_pcb");
    assert!(ref_text(&pcb2, "R1").hidden);
    assert!(close2(ref_text(&pcb2, "U1").font_size, (0.9, 0.9)));
    assert!(close2(ref_text(&pcb2, "C1").position, (1.0, -3.5)));
}

// ------------------------------------------------------------ copper API

#[test]
fn add_trace_and_via() {
    let dir = tmp();
    let mut pcb = create(100.0, 100.0);
    pcb.add_net("TestNet");
    let segs = pcb
        .add_trace(
            (10.0, 10.0),
            (50.0, 10.0),
            TraceOptions {
                width: 0.3,
                ..TraceOptions::net("TestNet")
            },
        )
        .unwrap();
    assert_eq!(segs.len(), 1);
    assert_eq!((segs[0].start, segs[0].end, segs[0].width), ((10.0, 10.0), (50.0, 10.0), 0.3));
    assert!(segs[0].net_number > 0);
    let multi = pcb
        .add_trace(
            (10.0, 20.0),
            (50.0, 60.0),
            TraceOptions {
                waypoints: vec![(30.0, 20.0), (30.0, 60.0)],
                ..TraceOptions::net("Signal1")
            },
        )
        .unwrap();
    assert_eq!(multi.len(), 3);
    assert_eq!((multi[1].start, multi[1].end), ((30.0, 20.0), (30.0, 60.0)));
    let v = pcb
        .add_via(50.0, 30.0, ViaOptions::net("VCC"))
        .unwrap();
    assert_eq!(v.layers, vec!["F.Cu", "B.Cu"]);
    assert!(v.net_number > 0);
    let pcb2 = save_reload(&mut pcb, dir.path(), "t.kicad_pcb");
    assert_eq!(pcb2.segments().len(), 4);
    assert!(close2(pcb2.segments()[0].start, (10.0, 10.0)));
    assert!(close2(pcb2.vias()[0].position, (50.0, 30.0)));
    assert!(close(pcb2.vias()[0].size, 0.6));
    // The tree carries sheet-absolute coordinates (origin (0,0) + 100mm board on A4).
    let (ox, oy) = pcb.board_origin();
    assert!(close(ox, 98.5) && close(oy, 55.0));
}

#[test]
fn routing_status_and_ratsnest() {
    let mut pcb = create(100.0, 100.0);
    let st = pcb.routing_status();
    assert_eq!((st.segments, st.vias, st.trace_length_mm), (0, 0, 0.0));
    assert!(st.nets_with_traces.is_empty());
    assert!(pcb.get_ratsnest().is_empty());
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "F.Cu", "TestNet");
    pcb.add_via(30.0, 20.0, ViaOptions::net("TestNet"));
    let st = pcb.routing_status();
    assert_eq!((st.segments, st.vias), (1, 1));
    assert!(close(st.trace_length_mm, 40.0));
    assert_eq!(st.nets_with_traces.len(), 1);

    let pcb = load("routing_test.kicad_pcb");
    let rats = pcb.get_ratsnest();
    let gnd = rats.iter().find(|r| r.net == "GND").unwrap();
    assert_eq!(gnd.pads.len(), 4);
    assert_eq!(pcb.routing_status().unrouted_pads.len(), 11);
}

#[test]
fn pad_positions() {
    let pcb = load("minimal.kicad_pcb");
    let fp = &pcb.footprints()[0];
    let pos = pcb.get_pad_position(&fp.reference, &fp.pads[0].number).unwrap();
    assert!(close2(pos, (fp.position.0 + fp.pads[0].position.0, fp.position.1)));
    assert!(pcb.get_pad_position("NONEXISTENT", "1").is_none());
    assert!(pcb.get_pad_position("R1", "999").is_none());
    // Rotated footprint uses KiCad's negated-angle transform.
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (footprint "x" (at 100 100 90) (property "Reference" "U1")
           (pad "1" smd rect (at 2 0) (size 1 1) (layers "F.Cu"))))"#,
    )
    .unwrap();
    assert!(close2(pcb.get_pad_position("U1", "1").unwrap(), (100.0, 98.0)));
}

#[test]
fn trace_between_pads_picks_up_net() {
    let mut pcb = load("routing_test.kicad_pcb");
    let segs = pcb
        .add_trace(("U1", "1"), ("R1", "1"), TraceOptions::default())
        .unwrap();
    assert_eq!(segs[0].net_name, "NET1");
    assert_eq!(segs[0].net_number, 1);
    assert!(close2(segs[0].start, (15.0 - 2.7, 15.0 - 1.905)));
    assert!(pcb.footprint_has_traces("U1"));
    assert!(!pcb.footprint_has_traces("J1"));
    assert!(pcb.add_trace(("U1", "99"), (0.0, 0.0), TraceOptions::default()).is_err());
}

#[test]
fn segment_and_via_to_sexp() {
    let mut seg = Segment::new((10.0, 20.0), (30.0, 40.0), 0.25, "F.Cu", 5);
    let s = seg.to_sexp((0.0, 0.0));
    assert!(s.has_tag("segment"));
    assert_eq!(s.find("start").unwrap().float_at(1), Some(20.0));
    assert_eq!(s.find("width").unwrap().float_at(0), Some(0.25));
    assert_eq!(s.find("layer").unwrap().string_at(0), Some("F.Cu"));
    assert_eq!(s.find("net").unwrap().int_at(0), Some(5));
    assert!(!s.find("uuid").unwrap().string_at(0).unwrap().is_empty());
    let names: Vec<&str> = s.children.iter().filter_map(|c| c.tag()).collect();
    assert!(names.iter().position(|n| *n == "uuid") < names.iter().position(|n| *n == "net"));

    let mut v = Via::new((50.0, 60.0), 0.6, 0.3, &["F.Cu", "B.Cu"], 3);
    let n = v.to_sexp((0.0, 0.0));
    assert_eq!(n.find("at").unwrap().float_at(0), Some(50.0));
    assert_eq!(n.find("layers").unwrap().string_at(1), Some("B.Cu"));
    assert_eq!(n.find("net").unwrap().int_at(0), Some(3));

    let mut empty = Segment::new((0.0, 0.0), (1.0, 0.0), 0.25, "F.Cu", 7);
    empty.net_name_only.0 = true;
    let e = empty.to_sexp((0.0, 0.0));
    assert_eq!(e.find("net").unwrap().int_at(0), Some(7));
    assert!(!e.to_kicad_string().contains("(net \"\")"));
}

// ------------------------------------------------------------ create / page fit

#[test]
fn create_centering_and_paper() {
    let pcb = create(200.0, 120.0);
    assert!(pcb.sexp().find("gr_rect").is_none());
    assert_eq!(edge_lines(&pcb).len(), 4);
    let ((x0, y0), (x1, y1)) = outline_bbox(&pcb);
    assert!(close(x0, 48.5) && close(y0, 45.0) && close(x1, 248.5) && close(y1, 165.0));
    assert!(close2(pcb.board_origin(), (48.5, 45.0)));

    let pcb = Pcb::create(CreateOptions {
        center: false,
        ..CreateOptions::size(100.0, 80.0)
    })
    .unwrap();
    assert_eq!(outline_bbox(&pcb), ((0.0, 0.0), (100.0, 80.0)));
    assert_eq!(pcb.board_origin(), (0.0, 0.0));

    let pcb = Pcb::create(CreateOptions {
        paper: "A3".into(),
        ..CreateOptions::size(200.0, 150.0)
    })
    .unwrap();
    assert_eq!(pcb.sexp().find("paper").unwrap().string_at(0), Some("A3"));
    let ((x0, y0), _) = outline_bbox(&pcb);
    assert!(close(x0, 110.0) && close(y0, 73.5));

    let pcb = Pcb::create(CreateOptions {
        paper: "A".into(),
        ..CreateOptions::size(100.0, 100.0)
    })
    .unwrap();
    let ((x0, y0), _) = outline_bbox(&pcb);
    assert!(close(x0, 89.7) && close(y0, 57.95));

    let err = Pcb::create(CreateOptions {
        paper: "InvalidSize".into(),
        ..Default::default()
    })
    .unwrap_err()
    .to_string();
    assert!(err.contains("Unknown paper size 'InvalidSize'") && err.contains("A4"));
    assert!(Pcb::create(CreateOptions {
        layers: 3,
        ..Default::default()
    })
    .is_err());

    let pcb = create(400.0, 300.0);
    let ((x0, y0), _) = outline_bbox(&pcb);
    assert!(close(x0, -51.5) && close(y0, -45.0));
}

#[test]
fn create_parameters_and_stamps() {
    let pcb = Pcb::create(CreateOptions {
        layers: 4,
        title: "Test Board".into(),
        revision: "2.0".into(),
        company: "Test Co".into(),
        paper: "A3".into(),
        ..CreateOptions::size(160.0, 100.0)
    })
    .unwrap();
    let tb = pcb.sexp().find("title_block").unwrap();
    assert_eq!(tb.find("title").unwrap().string_at(0), Some("Test Board"));
    assert_eq!(tb.find("rev").unwrap().string_at(0), Some("2.0"));
    assert_eq!(pcb.title(), "Test Board");
    assert_eq!(pcb.revision(), "2.0");
    assert_eq!(pcb.title_field("company"), "Test Co");
    let names: Vec<&str> = pcb.layers().iter().map(|l| l.name.as_str()).collect();
    for n in ["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"] {
        assert!(names.contains(&n));
    }
    assert_eq!(pcb.copper_layers().len(), 4);
    assert_eq!(pcb.setup().unwrap().stackup.len(), 13);
    assert_eq!(pcb.sexp().find("version").unwrap().int_at(0), Some(20241229));
    assert_eq!(pcb.date().len(), 10);
    assert!(close2(pcb.board_size().unwrap(), (160.0, 100.0)));

    let pcb = create(65.0, 56.0);
    assert!(close2(pcb.board_origin(), (116.0, 77.0)));
    assert!(close2(pcb.board_size().unwrap(), (65.0, 56.0)));
    let text = pcb.sexp().to_kicad_string();
    assert!(text.contains("(generator_version \"10.0\")"), "{text}");
    assert!(pcb.parse_warnings.is_empty());
    assert_eq!(pcb.footprint_count(), 0);
}

#[test]
fn create_outline_corners() {
    let pcb = Pcb::create(CreateOptions {
        center: false,
        ..CreateOptions::size(65.0, 56.0)
    })
    .unwrap();
    let mut starts: Vec<(f64, f64)> = edge_lines(&pcb).iter().map(|s| s.0).collect();
    let mut ends: Vec<(f64, f64)> = edge_lines(&pcb).iter().map(|s| s.1).collect();
    starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ends.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let corners = vec![(0.0, 0.0), (0.0, 56.0), (65.0, 0.0), (65.0, 56.0)];
    assert_eq!(starts, corners);
    assert_eq!(ends, corners);
    assert_eq!(pcb.get_board_outline().len(), 5);
}

#[test]
fn page_fit_paper_and_translation() {
    let mut pcb = create(200.0, 120.0);
    assert_eq!(pcb.sexp().find("paper").unwrap().string_at(0), Some("A4"));
    let (w, h) = pcb.page_fit(5.0).unwrap();
    let paper = pcb.sexp().find("paper").unwrap();
    assert_eq!(paper.string_at(0), Some("User"));
    assert!(close(paper.float_at(1).unwrap(), 210.0) && close(h, 130.0) && close(w, 210.0));

    let mut pcb = create(200.0, 120.0);
    let (w, h) = pcb.page_fit(7.5).unwrap();
    let ((x0, y0), (x1, y1)) = outline_bbox(&pcb);
    assert!(close(x0, 7.5) && close(y0, 7.5) && close(x1, 207.5) && close(y1, 127.5));
    assert!(close((x0 + x1) / 2.0, w / 2.0) && close((y0 + y1) / 2.0, h / 2.0));
    assert!(close2(pcb.board_origin(), (7.5, 7.5)));

    let dir = tmp();
    let p = write(
        dir.path(),
        "fp.kicad_pcb",
        "(kicad_pcb\n\t(version 20240108)\n\t(generator \"test\")\n\t(paper \"A4\")\n\t(layers\n\t\t(0 \"F.Cu\" signal)\n\t\t(44 \"Edge.Cuts\" user)\n\t)\n\t(footprint \"Test:FP\"\n\t\t(layer \"F.Cu\")\n\t\t(uuid \"11111111-1111-1111-1111-111111111111\")\n\t\t(at 10 20)\n\t\t(pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\"))\n\t)\n\t(gr_rect\n\t\t(start 0 0)\n\t\t(end 50 40)\n\t\t(layer \"Edge.Cuts\")\n\t\t(width 0.15)\n\t)\n\t(segment (start 5 5) (end 45 35) (width 0.25) (layer \"F.Cu\") (net 0))\n)\n",
    );
    let mut pcb = Pcb::load(&p).unwrap();
    pcb.page_fit(5.0).unwrap();
    let fp = pcb.sexp().find("footprint").unwrap();
    let at = fp.find_child("at").unwrap();
    assert_eq!((at.float_at(0), at.float_at(1)), (Some(15.0), Some(25.0)));
    let pad_at = fp.find_child("pad").unwrap().find_child("at").unwrap();
    assert_eq!(pad_at.float_at(0), Some(0.0));
    let seg = pcb.sexp().find("segment").unwrap();
    assert_eq!(seg.find("start").unwrap().float_at(0), Some(10.0));
    assert_eq!(seg.find("end").unwrap().float_at(1), Some(40.0));
}

#[test]
fn page_fit_preserves_geometry_exactly() {
    fn coords(node: &SExp, out: &mut Vec<(f64, f64)>) {
        if matches!(node.tag(), Some("at" | "start" | "end" | "mid" | "center" | "xy")) {
            if let (Some(x), Some(y)) = (node.float_at(0), node.float_at(1)) {
                out.push((x, y));
            }
        }
        for c in node.children.iter().filter(|c| c.is_list()) {
            coords(c, out);
        }
    }
    let all = |pcb: &Pcb| {
        let mut v = Vec::new();
        for c in pcb.sexp().children.iter().filter(|c| c.is_list()) {
            coords(c, &mut v);
        }
        v
    };
    let nm = |v: &[(f64, f64)]| -> Vec<i128> {
        let n: Vec<(i128, i128)> = v
            .iter()
            .map(|(x, y)| ((x * 1e6).round() as i128, (y * 1e6).round() as i128))
            .collect();
        let mut out = Vec::new();
        for i in 0..n.len() {
            for j in i + 1..n.len() {
                let (dx, dy) = (n[i].0 - n[j].0, n[i].1 - n[j].1);
                out.push(dx * dx + dy * dy);
            }
        }
        out
    };
    let mut pcb = create(80.077, 60.112);
    let before = all(&pcb);
    pcb.page_fit(5.0).unwrap();
    let after = all(&pcb);
    assert_eq!(before.len(), after.len());
    for (x, y) in &after {
        assert!(((x * 1e6).round() - x * 1e6).abs() < 1e-3);
        assert!(((y * 1e6).round() - y * 1e6).abs() < 1e-3);
    }
    assert_eq!(nm(&before), nm(&after));

    let dir = tmp();
    let src = write(
        dir.path(),
        "ang.kicad_pcb",
        "(kicad_pcb\n\t(version 20240108)\n\t(generator \"test\")\n\t(paper \"A4\")\n\t(layers\n\t\t(0 \"F.Cu\" signal)\n\t\t(44 \"Edge.Cuts\" user)\n\t)\n\t(gr_rect\n\t\t(start 100 100)\n\t\t(end 250 200)\n\t\t(layer \"Edge.Cuts\")\n\t\t(width 0.15)\n\t)\n\t(segment (start 243 112) (end 242.9252 112.0748) (width 0.25) (layer \"F.Cu\") (net 0))\n)\n",
    );
    let angle = |p: &Path| {
        let pcb = Pcb::load(p).unwrap();
        let seg = pcb.sexp().find("segment").unwrap();
        let s = seg.find("start").unwrap();
        let e = seg.find("end").unwrap();
        (e.float_at(1).unwrap() - s.float_at(1).unwrap())
            .atan2(e.float_at(0).unwrap() - s.float_at(0).unwrap())
            .to_degrees()
    };
    let before = angle(&src);
    assert!((before.abs() - 135.0).abs() < 1e-9);
    let mut pcb = Pcb::load(&src).unwrap();
    pcb.page_fit(5.0).unwrap();
    let out = dir.path().join("fit.kicad_pcb");
    pcb.save(Some(&out)).unwrap();
    assert!((angle(&out) - before).abs() < 1e-9);
}

#[test]
fn page_fit_idempotent_and_requires_outline() {
    let dir = tmp();
    let mut pcb = create(120.0, 90.0);
    let a = pcb.page_fit(5.0).unwrap();
    let mut re = save_reload(&mut pcb, dir.path(), "rt.kicad_pcb");
    let b = re.page_fit(5.0).unwrap();
    assert!(close2(a, b));
    let mut none = Pcb::parse_str("(kicad_pcb (version 20240108) (layers (0 \"F.Cu\" signal)))").unwrap();
    assert!(none.page_fit(5.0).is_err());
}

// ------------------------------------------------------------ strip traces

#[test]
fn strip_basic() {
    let dir = tmp();
    let mut pcb = load("minimal.kicad_pcb");
    let n_fp = pcb.footprints().len();
    let st = pcb.strip_traces(StripOptions::default());
    assert_eq!(st.segments, 1);
    assert!(pcb.segments().is_empty() && pcb.vias().is_empty());
    assert_eq!(pcb.footprints().len(), n_fp);
    let re = save_reload(&mut pcb, dir.path(), "s.kicad_pcb");
    assert!(re.segments().is_empty());

    let mut pcb = load("zone_test.kicad_pcb");
    assert_eq!(pcb.strip_traces(StripOptions::default()).zones, 0);
    assert_eq!(pcb.zones().len(), 3);
    let st = pcb.strip_traces(StripOptions {
        keep_zones: false,
        ..Default::default()
    });
    assert_eq!(st.zones, 3);
    assert!(pcb.zones().is_empty());

    let mut empty = create(100.0, 100.0);
    assert_eq!(empty.strip_traces(StripOptions::default()), StripStats::default());
}

#[test]
fn strip_by_net_including_name_dialect() {
    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "F.Cu", "GND");
    trace(&mut pcb, (10.0, 20.0), (50.0, 20.0), "F.Cu", "VCC");
    let st = pcb.strip_traces(StripOptions {
        nets: Some(vec!["GND".into()]),
        ..Default::default()
    });
    assert_eq!(st.segments, 1);
    assert_eq!(pcb.segments().len(), 1);
    assert_eq!(pcb.segments()[0].net_number, net_number(&pcb, "VCC"));

    let mut pcb = Pcb::parse_str(
        r#"(kicad_pcb (version 20260306) (generator "pcbnew")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
  (net 0 "") (net 1 "GND") (net 2 "VCC")
  (segment (start 10 10) (end 50 10) (width 0.25) (layer "F.Cu") (net "GND") (uuid "s1"))
  (segment (start 10 12) (end 50 12) (width 0.25) (layer "F.Cu") (net "GND") (uuid "s2"))
  (segment (start 10 20) (end 50 20) (width 0.25) (layer "F.Cu") (net "VCC") (uuid "s3"))
  (via (at 30 10) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net "GND") (uuid "v1"))
  (via (at 30 20) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net "VCC") (uuid "v2")))"#,
    )
    .unwrap();
    let st = pcb.strip_traces(StripOptions {
        nets: Some(vec!["GND".into()]),
        ..Default::default()
    });
    assert_eq!((st.segments, st.vias), (2, 1));
    assert_eq!(pcb.segments().len(), 1);
    assert_eq!(pcb.vias().len(), 1);
    assert_eq!(pcb.vias()[0].net_number, 2);
}

fn four_layer_board() -> Pcb {
    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "F.Cu", "SIG_A");
    trace(&mut pcb, (10.0, 20.0), (50.0, 20.0), "In1.Cu", "SIG_B");
    trace(&mut pcb, (10.0, 30.0), (50.0, 30.0), "In2.Cu", "SIG_C");
    trace(&mut pcb, (10.0, 40.0), (50.0, 40.0), "B.Cu", "SIG_D");
    pcb
}

fn layers_opt(l: &[&str]) -> StripOptions<'static> {
    StripOptions {
        layers: Some(l.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    }
}

#[test]
fn strip_by_layers() {
    let mut pcb = four_layer_board();
    assert_eq!(pcb.strip_traces(layers_opt(&["In1.Cu"])).segments, 1);
    assert_eq!(pcb.segments().len(), 3);
    assert!(pcb.segments().iter().all(|s| s.layer != "In1.Cu"));
    let mut pcb = four_layer_board();
    assert_eq!(pcb.strip_traces(layers_opt(&["In1.Cu", "In2.Cu"])).segments, 2);
    let layers: Vec<&str> = pcb.segments().iter().map(|s| s.layer.as_str()).collect();
    assert_eq!(layers, vec!["F.Cu", "B.Cu"]);

    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "In1.Cu", "AUDIO_R");
    trace(&mut pcb, (10.0, 20.0), (50.0, 20.0), "In1.Cu", "AUDIO_L");
    trace(&mut pcb, (10.0, 30.0), (50.0, 30.0), "F.Cu", "AUDIO_R");
    let st = pcb.strip_traces(StripOptions {
        nets: Some(vec!["AUDIO_R".into()]),
        ..layers_opt(&["In1.Cu"])
    });
    assert_eq!(st.segments, 1);
    assert_eq!(pcb.segments().len(), 2);

    let mut pcb = four_layer_board();
    assert_eq!(pcb.strip_traces(layers_opt(&[])).segments, 0);
    assert_eq!(pcb.strip_traces(layers_opt(&["Nonexistent.Cu"])).segments, 0);
    assert_eq!(pcb.segments().len(), 4);

    let dir = tmp();
    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "F.Cu", "SIG_A");
    trace(&mut pcb, (10.0, 20.0), (50.0, 20.0), "In1.Cu", "SIG_B");
    pcb.strip_traces(layers_opt(&["In1.Cu"]));
    let re = save_reload(&mut pcb, dir.path(), "l.kicad_pcb");
    assert_eq!(re.segments().len(), 1);
    assert_eq!(re.segments()[0].layer, "F.Cu");
}

#[test]
fn strip_power_exclusion() {
    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "In1.Cu", "GND");
    assert_eq!(pcb.strip_traces(layers_opt(&["In1.Cu"])).segments, 1);

    let mut pcb = create(100.0, 100.0);
    let names = ["GND", "+3V3", "+5V", "VCC", "VDD", "VBUS"];
    for (i, n) in names.iter().enumerate() {
        let y = 10.0 + i as f64 * 10.0;
        trace(&mut pcb, (10.0, y), (50.0, y), "In1.Cu", n);
    }
    trace(&mut pcb, (10.0, 80.0), (50.0, 80.0), "In1.Cu", "AUDIO");
    let st = pcb.strip_traces(StripOptions {
        exclude_power: true,
        ..layers_opt(&["In1.Cu"])
    });
    assert_eq!(st.segments, 1);
    assert_eq!(pcb.segments().len(), names.len());

    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "In1.Cu", "PWR_RAIL");
    trace(&mut pcb, (10.0, 20.0), (50.0, 20.0), "In1.Cu", "SIG_A");
    let pat = |n: &str| n.to_ascii_uppercase().starts_with("PWR_");
    let st = pcb.strip_traces(StripOptions {
        exclude_power: true,
        power_pattern: Some(&pat),
        ..layers_opt(&["In1.Cu"])
    });
    assert_eq!(st.segments, 1);
    assert_eq!(pcb.segments()[0].net_number, net_number(&pcb, "PWR_RAIL"));
}

#[test]
fn strip_vias_and_orphans() {
    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "In1.Cu", "SIG_A");
    via(&mut pcb, 50.0, 10.0, &["F.Cu", "B.Cu"], "SIG_A");
    let st = pcb.strip_traces(layers_opt(&["In1.Cu"]));
    assert_eq!((st.segments, st.vias), (1, 0));
    assert_eq!(pcb.vias().len(), 1);

    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "In1.Cu", "SIG_A");
    via(&mut pcb, 50.0, 10.0, &["In1.Cu", "In2.Cu"], "SIG_A");
    let st = pcb.strip_traces(layers_opt(&["In1.Cu", "In2.Cu"]));
    assert_eq!((st.segments, st.vias), (1, 1));
    assert!(pcb.vias().is_empty());

    let orphan = StripOptions {
        remove_orphan_vias: true,
        ..layers_opt(&["In1.Cu"])
    };
    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "In1.Cu", "SIG_A");
    via(&mut pcb, 50.0, 10.0, &["F.Cu", "B.Cu"], "SIG_A");
    let st = pcb.strip_traces(orphan);
    assert_eq!((st.segments, st.vias), (1, 1));
    assert!(pcb.vias().is_empty());

    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (10.0, 10.0), (50.0, 10.0), "In1.Cu", "SIG_A");
    trace(&mut pcb, (50.0, 10.0), (90.0, 10.0), "F.Cu", "SIG_A");
    via(&mut pcb, 50.0, 10.0, &["F.Cu", "B.Cu"], "SIG_A");
    let st = pcb.strip_traces(StripOptions {
        remove_orphan_vias: true,
        ..layers_opt(&["In1.Cu"])
    });
    assert_eq!((st.segments, st.vias), (1, 0));
    assert_eq!(pcb.vias().len(), 1);
}

fn region(r: (f64, f64, f64, f64)) -> StripOptions<'static> {
    StripOptions {
        region: Some(r),
        ..Default::default()
    }
}

#[test]
fn strip_region() {
    let r = (10.0, 10.0, 40.0, 40.0);
    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (20.0, 20.0), (30.0, 20.0), "F.Cu", "SIG_A");
    let st = pcb.strip_traces(region(r));
    assert_eq!((st.segments, st.segments_clipped), (1, 0));
    assert!(pcb.segments().is_empty());

    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (60.0, 60.0), (80.0, 60.0), "F.Cu", "SIG_A");
    assert_eq!(pcb.strip_traces(region(r)), StripStats::default());
    assert_eq!(pcb.segments().len(), 1);

    let dir = tmp();
    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (20.0, 20.0), (60.0, 20.0), "F.Cu", "SIG_A");
    let st = pcb.strip_traces(region(r));
    assert_eq!((st.segments, st.segments_clipped), (0, 1));
    let s = &pcb.segments()[0];
    let mut xs = [s.start.0, s.end.0];
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(close(xs[0], 40.0) && close(xs[1], 60.0) && close(s.start.1, 20.0));
    let re = save_reload(&mut pcb, dir.path(), "clip.kicad_pcb");
    let s = &re.segments()[0];
    let mut xs = [s.start.0, s.end.0];
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(close(xs[0], 40.0) && close(xs[1], 60.0));

    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (5.0, 25.0), (55.0, 25.0), "F.Cu", "SIG_A");
    let st = pcb.strip_traces(region(r));
    assert_eq!((st.segments, st.segments_clipped, st.segments_boundary_skipped), (0, 0, 1));

    let mut pcb = create(100.0, 100.0);
    via(&mut pcb, 25.0, 25.0, &["F.Cu", "B.Cu"], "SIG_A");
    via(&mut pcb, 70.0, 70.0, &["F.Cu", "B.Cu"], "SIG_A");
    assert_eq!(pcb.strip_traces(region(r)).vias, 1);
    assert!(close2(pcb.vias()[0].position, (70.0, 70.0)));

    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (20.0, 20.0), (30.0, 20.0), "F.Cu", "NET_A");
    trace(&mut pcb, (20.0, 30.0), (30.0, 30.0), "F.Cu", "NET_B");
    trace(&mut pcb, (60.0, 60.0), (70.0, 60.0), "F.Cu", "NET_A");
    let st = pcb.strip_traces(StripOptions {
        nets: Some(vec!["NET_A".into()]),
        ..region(r)
    });
    assert_eq!(st.segments, 1);
    assert_eq!(pcb.segments().len(), 2);

    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (20.0, 20.0), (30.0, 20.0), "In1.Cu", "TARGET");
    trace(&mut pcb, (20.0, 22.0), (30.0, 22.0), "In1.Cu", "OTHER");
    trace(&mut pcb, (20.0, 24.0), (30.0, 24.0), "F.Cu", "TARGET");
    trace(&mut pcb, (60.0, 60.0), (70.0, 60.0), "In1.Cu", "TARGET");
    let st = pcb.strip_traces(StripOptions {
        nets: Some(vec!["TARGET".into()]),
        layers: Some(vec!["In1.Cu".into()]),
        ..region(r)
    });
    assert_eq!(st.segments, 1);
    assert_eq!(pcb.segments().len(), 3);

    let mut pcb = create(100.0, 100.0);
    trace(&mut pcb, (20.0, 20.0), (30.0, 20.0), "F.Cu", "SIG_A");
    assert_eq!(pcb.strip_traces(region((40.0, 40.0, 10.0, 10.0))).segments, 1);
}

// ------------------------------------------------------------ KiCad 10 nets

const KICAD10_NAMEONLY_PCB: &str = r#"(kicad_pcb
  (version 20260206)
  (generator "pcbnew")
  (generator_version "10.0")
  (general (thickness 1.6))
  (paper "A4")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
  (setup (pad_to_mask_clearance 0))
  (net 0 "")
  (net 1 "GND")
  (net 2 "VCC")
  (footprint "TestFP"
    (layer "F.Cu")
    (uuid "fp-uuid-1")
    (at 100 100)
    (property "Reference" "R1" (at 0 0) (layer "F.SilkS") (uuid "ref-uuid"))
    (pad "1" smd rect (at -1 0) (size 1 1) (layers "F.Cu") (net "GND"))
    (pad "2" smd rect (at 1 0) (size 1 1) (layers "F.Cu") (net "VCC"))
  )
  (segment (start 99 100) (end 101 100) (width 0.25) (layer "F.Cu") (net "GND") (uuid "seg-1"))
)
"#;

#[test]
fn kicad10_net_recovery() {
    let pcb = Pcb::parse_str(KICAD10_NAMEONLY_PCB).unwrap();
    let pads = &pcb.footprints()[0].pads;
    assert_eq!((pads[0].net_name.as_str(), pads[0].net_number), ("GND", 1));
    assert_eq!((pads[1].net_name.as_str(), pads[1].net_number), ("VCC", 2));
    assert_eq!(pcb.segments()[0].net_number, 1);
    assert!(pcb.net_name_only_dialect());

    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (version 20260206) (net 0 "") (net 1 "GND") (net 2 "VCC")
           (via (at 100 100) (size 0.8) (drill 0.4) (layers "F.Cu" "B.Cu") (net "VCC") (uuid "via-1")))"#,
    )
    .unwrap();
    assert_eq!((pcb.vias()[0].net_name.as_str(), pcb.vias()[0].net_number), ("VCC", 2));

    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (version 20260206) (net 0 "") (net 1 "GND")
           (footprint "T" (at 100 100) (property "Reference" "R1")
             (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu") (net 0 ""))
             (pad "2" smd rect (at 0 0) (size 1 1) (layers "F.Cu"))))"#,
    )
    .unwrap();
    for p in &pcb.footprints()[0].pads {
        assert_eq!((p.net_number, p.net_name.as_str()), (0, ""));
    }

    let pcb = Pcb::load(fixtures().join("test_zone_fill.kicad_pcb")).unwrap();
    assert_eq!(pcb.get_net(1).unwrap().name, "GND");
    assert_eq!(pcb.get_net(2).unwrap().name, "VCC");
    for fp in pcb.footprints() {
        for p in &fp.pads {
            match p.net_name.as_str() {
                "GND" => assert_eq!(p.net_number, 1),
                "VCC" => assert_eq!(p.net_number, 2),
                _ => {}
            }
        }
    }
}

#[test]
fn save_board_net_table_synthesis() {
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (version 20260206) (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
  (setup (pad_to_mask_clearance 0))
  (footprint "TestFP" (layer "F.Cu") (uuid "fp-uuid") (at 100 100)
    (property "Reference" "R1" (at 0 0) (layer "F.SilkS") (uuid "ref-uuid"))
    (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu") (net "VCC"))
    (pad "2" smd rect (at 2 0) (size 1 1) (layers "F.Cu") (net "GND")))
  (segment (start 0 0) (end 10 0) (width 0.25) (layer "F.Cu") (net "GND") (uuid "seg-1"))
  (via (at 5 5) (size 0.8) (drill 0.4) (layers "F.Cu" "B.Cu") (net "VCC") (uuid "via-1")))"#,
    )
    .unwrap();
    assert_eq!(pcb.get_net(0).unwrap().name, "");
    assert_eq!(net_number(&pcb, "VCC"), 1);
    assert_eq!(net_number(&pcb, "GND"), 2);
    assert_eq!(pcb.segments()[0].net_number, 2);
    assert_eq!(pcb.vias()[0].net_number, 1);
    // Written back after the setup node.
    let text = pcb.sexp().to_kicad_string();
    assert!(text.contains("(net 1 \"VCC\")"), "{text}");

    let src = r#"(kicad_pcb (version 20260206)
  (footprint "T" (at 100 100) (property "Reference" "R1")
    (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu") (net "SIGA"))
    (pad "2" smd rect (at 2 0) (size 1 1) (layers "F.Cu") (net "SIGB"))
    (pad "3" smd rect (at 4 0) (size 1 1) (layers "F.Cu") (net "SIGC"))))"#;
    let names = |p: &Pcb| p.nets().iter().map(|n| (n.name.clone(), n.number)).collect::<Vec<_>>();
    let a = names(&Pcb::parse_str(src).unwrap());
    assert_eq!(a, names(&Pcb::parse_str(src).unwrap()));
    assert_eq!(
        a,
        vec![("".into(), 0), ("SIGA".into(), 1), ("SIGB".into(), 2), ("SIGC".into(), 3)]
    );

    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (version 20260206)
  (footprint "T" (at 100 100) (property "Reference" "R1")
    (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu") (net "VCC"))
    (pad "2" smd rect (at 2 0) (size 1 1) (layers "F.Cu") (net 7 "GND"))))"#,
    )
    .unwrap();
    assert_eq!(net_number(&pcb, "GND"), 7);
    assert_eq!(net_number(&pcb, "VCC"), 1);
}

#[test]
fn save_board_fixture_round_trip() {
    let dir = tmp();
    let mut pcb = Pcb::load(fixtures().join("test_kicad10_save_board.kicad_pcb")).unwrap();
    let mut names: Vec<String> = pcb.nets().iter().map(|n| n.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["", "GND", "LED_ANODE", "VCC"]);
    for fp in pcb.footprints() {
        for p in fp.pads.iter().filter(|p| !p.net_name.is_empty()) {
            assert_ne!(p.net_number, 0);
            assert_eq!(p.net_number, net_number(&pcb, &p.net_name));
        }
    }
    let out = dir.path().join("resaved.kicad_pcb");
    pcb.save(Some(&out)).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("(net 0 \"\")") && text.contains("(net 1"));
    let re = Pcb::load(&out).unwrap();
    assert_eq!(re.nets().len(), 4);
}

fn net_refs(text: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&format!("({tag}")) {
        rest = &rest[i + 1..];
        let j = rest.find("(net").unwrap();
        let k = rest[j..].find(')').unwrap();
        out.push(rest[j..j + k + 1].to_string());
    }
    out
}

#[test]
fn net_dialect_preserved_on_added_copper() {
    let dir = tmp();
    let mut pcb = Pcb::parse_str(KICAD10_NAMEONLY_PCB).unwrap();
    let added = pcb
        .add_trace((99.0, 101.0), (101.0, 101.0), TraceOptions::net("GND"))
        .unwrap();
    assert!(added[0].net_name_only.0);
    let v = pcb.add_via(100.0, 100.0, ViaOptions::net("VCC")).unwrap();
    assert!(v.net_name_only.0);
    let out = dir.path().join("o.kicad_pcb");
    pcb.save(Some(&out)).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    let segs = net_refs(&text, "segment");
    assert_eq!(segs, vec!["(net \"GND\")", "(net \"GND\")"]);
    assert_eq!(net_refs(&text, "via "), vec!["(net \"VCC\")"]);
    assert!(Pcb::load(&out).unwrap().segments().iter().all(|s| s.net_name_only.0));

    let mut pcb = load("minimal.kicad_pcb");
    let added = pcb
        .add_trace((10.0, 10.0), (20.0, 10.0), TraceOptions::net("GND"))
        .unwrap();
    assert!(!added[0].net_name_only.0);
    let text = pcb.sexp().to_kicad_string();
    for r in net_refs(&text, "segment") {
        assert!(r.starts_with("(net 1"), "{r}");
    }
}

// ------------------------------------------------------------ removals

#[test]
fn remove_footprint_and_segments() {
    let dir = tmp();
    let mut pcb = load("minimal.kicad_pcb");
    assert!(!pcb.remove_footprint("NONEXISTENT"));
    assert!(pcb.remove_footprint("R1"));
    assert!(pcb.get_footprint("R1").is_none());
    assert_eq!(pcb.sexp().find_all("footprint").count(), 0);
    let re = save_reload(&mut pcb, dir.path(), "a.kicad_pcb");
    assert_eq!(re.footprints().len(), 0);

    let mut pcb = load("minimal.kicad_pcb");
    assert_eq!(pcb.remove_segments(&[]), 0);
    let seg = pcb.segments()[0].clone();
    assert_eq!(seg.uuid, "00000000-0000-0000-0000-000000000020");
    assert_eq!(pcb.remove_segments(&[seg]), 1);
    assert!(pcb.segments().is_empty());
    assert_eq!(pcb.segment_count(), 0);
}

#[test]
fn remove_without_uuid_survives_origin_round_trip() {
    let src = r#"(kicad_pcb (version 20211014) (generator "test")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
  (net 0 "") (net 1 "GND")
  (gr_rect (start 256.063615 24.408798) (end 400 200) (stroke (width 0.1) (type default)) (fill none) (layer "Edge.Cuts"))
  (segment (start 272.26 58.3225) (end 272.26 61.01) (width 0.25) (layer "F.Cu") (net 1))
  (via (at 272.26 58.3225) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1)))"#;
    let mut pcb = Pcb::parse_str(src).unwrap();
    assert_ne!(pcb.board_origin(), (0.0, 0.0));
    assert_eq!(pcb.segments()[0].uuid, "");
    let segs = pcb.segments().to_vec();
    assert_eq!(pcb.remove_segments(&segs), 1);
    assert!(pcb.segments().is_empty() && pcb.segment_count() == 0);
    let vias = pcb.vias().to_vec();
    assert_eq!(pcb.remove_vias(&vias), 1);
    assert!(pcb.vias().is_empty() && pcb.via_count() == 0);
}

#[test]
fn remove_vias_multilayer_fixture() {
    let path = fixtures().join("projects/multilayer_zones.kicad_pcb");
    let mut pcb = Pcb::load(&path).unwrap();
    assert_eq!(pcb.vias().len(), 2);
    assert_eq!(pcb.remove_vias(&[]), 0);
    let (s, f, z) = (pcb.segments().len(), pcb.footprints().len(), pcb.zones().len());
    let v = pcb.vias()[0].clone();
    assert!(!v.uuid.is_empty());
    assert_eq!(pcb.remove_vias(&[v]), 1);
    assert_eq!(pcb.vias().len(), 1);
    let rest = pcb.vias().to_vec();
    pcb.remove_vias(&rest);
    assert_eq!(pcb.via_count(), 0);
    assert_eq!((pcb.segments().len(), pcb.footprints().len(), pcb.zones().len()), (s, f, z));
}

#[test]
fn relocate_via_and_drag_endpoints() {
    let dir = tmp();
    let mut pcb = create(50.0, 50.0);
    trace(&mut pcb, (10.0, 10.0), (20.0, 10.0), "F.Cu", "A");
    trace(&mut pcb, (20.0, 10.0), (20.0, 20.0), "F.Cu", "A");
    via(&mut pcb, 20.0, 10.0, &["F.Cu", "B.Cu"], "A");
    assert!(pcb.relocate_via(0, (21.0, 11.0)));
    assert_eq!(pcb.drag_trace_endpoints((20.0, 10.0), (1.0, 1.0), 0.05, None), 2);
    let re = save_reload(&mut pcb, dir.path(), "d.kicad_pcb");
    assert!(close2(re.vias()[0].position, (21.0, 11.0)));
    assert!(close2(re.segments()[0].end, (21.0, 11.0)));
    assert!(close2(re.segments()[1].start, (21.0, 11.0)));
    assert_eq!(re.segments()[0].uuid, pcb.segments()[0].uuid);
}

// ------------------------------------------------------------ dedup

fn sig1() -> TraceOptions {
    TraceOptions::net("Sig1")
}

#[test]
fn copper_dedup() {
    let mut pcb = create(100.0, 100.0);
    assert_eq!(pcb.add_trace((10.0, 10.0), (50.0, 10.0), sig1()).unwrap().len(), 1);
    assert!(pcb.add_trace((10.0, 10.0), (50.0, 10.0), sig1()).unwrap().is_empty());
    assert!(pcb.add_trace((50.0, 10.0), (10.0, 10.0), sig1()).unwrap().is_empty());
    pcb.add_trace((10.0, 10.0), (50.0, 10.0), TraceOptions { layer: "B.Cu".into(), ..sig1() }).unwrap();
    pcb.add_trace((10.0, 10.0), (50.0, 10.0), TraceOptions::net("Sig2")).unwrap();
    pcb.add_trace((10.0, 10.0), (50.0, 10.0), TraceOptions { width: 0.5, ..sig1() }).unwrap();
    pcb.add_trace((50.0, 10.0), (50.0, 40.0), sig1()).unwrap();
    pcb.add_trace((10.0, 10.0), (50.0, 10.0), TraceOptions { dedupe: false, ..sig1() }).unwrap();
    assert_eq!(pcb.segments().len(), 6);

    assert!(pcb.add_via(50.0, 30.0, ViaOptions::net("GND")).is_some());
    assert!(pcb.add_via(50.0, 30.0, ViaOptions::net("GND")).is_none());
    assert!(pcb.add_via(50.0, 30.0, ViaOptions::net("VCC")).is_some());
    assert!(pcb.add_via(50.0, 31.0, ViaOptions::net("GND")).is_some());
    assert_eq!(pcb.vias().len(), 3);
}

#[test]
fn dedupe_copper_cleanup() {
    let dir = tmp();
    let mut pcb = create(100.0, 100.0);
    let n = pcb.add_net("Sig1").number;
    for _ in 0..4 {
        pcb.add_trace((10.0, 10.0), (50.0, 10.0), TraceOptions { dedupe: false, ..sig1() }).unwrap();
    }
    for _ in 0..3 {
        pcb.add_via(50.0, 30.0, ViaOptions { dedupe: false, ..ViaOptions::net("GND") });
    }
    pcb.add_trace((10.0, 10.0), (50.0, 40.0), TraceOptions { dedupe: false, ..sig1() }).unwrap();
    pcb.add_via(60.0, 30.0, ViaOptions { dedupe: false, ..ViaOptions::net("GND") });
    assert_eq!((pcb.segments().len(), pcb.vias().len()), (5, 4));
    let before: Vec<(f64, f64)> = pcb.segments_in_net(n).map(|s| s.end).collect();
    let st = pcb.dedupe_copper();
    assert_eq!((st.segments, st.vias), (3, 2));
    let mut after: Vec<(f64, f64)> = pcb.segments_in_net(n).map(|s| s.end).collect();
    after.dedup();
    let mut b = before.clone();
    b.dedup();
    assert_eq!(b, after);
    let re = save_reload(&mut pcb, dir.path(), "c.kicad_pcb");
    assert_eq!((re.segments().len(), re.vias().len()), (2, 2));
    let mut clean = create(100.0, 100.0);
    clean.add_trace((10.0, 10.0), (50.0, 10.0), sig1()).unwrap();
    assert_eq!(clean.dedupe_copper().segments, 0);
}

// ------------------------------------------------------------ tenting / arcs

fn via_text(tenting: &str) -> String {
    format!("(via (at 10 10) (size 0.6) (drill 0.3) (layers \"F.Cu\" \"B.Cu\") {tenting} (net 1) (uuid \"abc\"))")
}

#[test]
fn via_tenting_round_trip() {
    let v = Via::from_sexp(&kct::parse(&via_text("(tenting (front no) (back yes))")).unwrap());
    assert_eq!((v.tenting_front.as_deref(), v.tenting_back.as_deref()), (Some("no"), Some("yes")));
    let v = Via::from_sexp(&kct::parse(&via_text("(tenting (front none) (back no))")).unwrap());
    assert_eq!(v.tenting_front.as_deref(), Some("none"));
    let mut v = Via::from_sexp(&kct::parse(&via_text("")).unwrap());
    assert!(v.tenting_front.is_none() && v.tenting_back.is_none());
    assert!(v.to_sexp((0.0, 0.0)).get("tenting").is_none());
    let mut v = Via::from_sexp(&kct::parse(&via_text("(tenting (front no) (back none))")).unwrap());
    let node = v.to_sexp((0.0, 0.0));
    let names: Vec<&str> = node.children.iter().filter_map(|c| c.tag()).collect();
    assert_eq!(
        names.iter().position(|n| *n == "tenting").unwrap(),
        names.iter().position(|n| *n == "layers").unwrap() + 1
    );
    let re = Via::from_sexp(&node);
    assert_eq!((re.tenting_front.as_deref(), re.tenting_back.as_deref()), (Some("no"), Some("none")));
    let micro = Via::from_sexp(&kct::parse("(via micro (at 0 0) (size 0.3) (drill 0.1) (layers \"F.Cu\" \"In1.Cu\"))").unwrap());
    assert_eq!(micro.via_type.as_deref(), Some("micro"));

    let dir = tmp();
    let p = write(
        dir.path(),
        "t.kicad_pcb",
        r#"(kicad_pcb (version 20260206) (generator "pcbnew") (net 0 "") (net 1 "GND")
    (via (at 10 10) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (tenting (front no) (back yes)) (net 1) (uuid "abc")))"#,
    );
    let mut pcb = Pcb::load(&p).unwrap();
    let re = save_reload(&mut pcb, dir.path(), "o.kicad_pcb");
    assert_eq!(re.vias()[0].tenting_front.as_deref(), Some("no"));
    assert_eq!(re.vias()[0].tenting_back.as_deref(), Some("yes"));
}

#[test]
fn setup_tenting() {
    let mk = |body: &str| {
        Pcb::parse_str(&format!("(kicad_pcb (version 20260206) (setup (pad_to_mask_clearance 0) {body}))"))
            .unwrap()
    };
    let s = mk("(tenting (front yes) (back no))");
    assert_eq!((s.setup().unwrap().tenting_front, s.setup().unwrap().tenting_back), (Some(true), Some(false)));
    let s = mk("");
    assert_eq!(s.setup().unwrap().tenting_front, None);
}

#[test]
fn graphic_arc_legacy_center_angle() {
    let arc = |t: &str| GraphicArc::from_sexp(&kct::parse(t).unwrap());
    let a = arc(r#"(gr_arc (start 101 93) (end 101 92) (angle -90) (layer "Edge.Cuts") (width 0.05))"#);
    assert!(close2(a.start, (101.0, 92.0)) && close2(a.end, (100.0, 93.0)));
    assert!((a.mid.0 - 100.29289).abs() < 1e-4 && (a.mid.1 - 92.29289).abs() < 1e-4);
    let a = arc(r#"(gr_arc (start 121 93) (end 121 92) (angle 90) (layer "Edge.Cuts") (width 0.05))"#);
    assert!(close2(a.end, (122.0, 93.0)));
    assert!((a.mid.0 - 121.70711).abs() < 1e-4);
    let a = arc(r#"(gr_arc (start 2 0) (mid 0.585786 0.585786) (end 0 2) (layer "Edge.Cuts"))"#);
    assert!(close2(a.mid, (0.585786, 0.585786)));
    let a = arc(r#"(gr_arc (start 2 0) (end 0 2) (layer "Edge.Cuts"))"#);
    assert_eq!(a.mid, (0.0, 0.0));
    assert!(close(a.width, 0.1));
}

const ARC: &str = "(arc  (start 10.000 10)\n  (mid 15 5.0000) (end 20 10)\n (width .25) (layer \"F.Cu\")\n (uuid \"22ea5bc2-b75d-400a-964f-ff4bf9222c3d\") (net 1))";
const ARC_HEADER: &str = "(kicad_pcb (version 20240108) (generator \"test\")\n (general (thickness 1.6))\n (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal) (44 \"Edge.Cuts\" user))\n (net 0 \"\") (net 1 \"SIG\") (net 2 \"OTHER\")";

#[test]
fn copper_arcs() {
    let dir = tmp();
    for name_only in [false, true] {
        let arc = if name_only { ARC.replace("(net 1)", "(net \"SIG\")") } else { ARC.to_string() };
        let header = if name_only {
            ARC_HEADER.replace("(net 0 \"\") (net 1 \"SIG\") (net 2 \"OTHER\")", "")
        } else {
            ARC_HEADER.to_string()
        };
        let src = write(dir.path(), "in.kicad_pcb", &format!("{header}{arc})"));
        let mut pcb = Pcb::load(&src).unwrap();
        assert_eq!(pcb.arcs().len(), 1);
        let a = pcb.arcs()[0].clone();
        assert_eq!((a.start, a.mid, a.end), ((10.0, 10.0), (15.0, 5.0), (20.0, 10.0)));
        assert_eq!(a.width, 0.25);
        assert_eq!(a.net_name, "SIG");
        assert_eq!(a.net_name_only.0, name_only);
        assert_eq!(pcb.arcs_on_layer("F.Cu").count(), 1);
        assert_eq!(pcb.arcs_in_net(a.net_number).count(), 1);
        assert_eq!(pcb.arcs_on_layer("B.Cu").count(), 0);
        assert!(pcb.segments().is_empty());
        assert!(close(a.length(), 5.0 * std::f64::consts::PI));
        assert!(close(pcb.total_trace_length(None), 5.0 * std::f64::consts::PI));
        assert_eq!(pcb.arc_count(), 1);
        let pts = a.centerline_points(1e-5).unwrap();
        assert_eq!(*pts.first().unwrap(), a.start);
        assert_eq!(*pts.last().unwrap(), a.end);
        let out = dir.path().join("out.kicad_pcb");
        pcb.save(Some(&out)).unwrap();
        assert!(std::fs::read_to_string(&out).unwrap().contains(&arc));
        assert_eq!(Pcb::load(&out).unwrap().arcs(), pcb.arcs());
    }
    let bad = format!("{ARC_HEADER}{})", ARC.replace("(mid 15 5.0000)", "(mid 15 10)"));
    assert!(Pcb::parse_str(&bad).is_err());
}

// ------------------------------------------------------------ legacy boards

const LEGACY_MODULE_BOARD: &str = r#"(kicad_pcb (version 4) (host pcbnew "(2017-11-30)")
  (general (thickness 1.6))
  (page A4)
  (layers (0 F.Cu signal) (31 B.Cu signal) (44 Edge.Cuts user))
  (net 0 "")
  (net 1 GND)
  (net 2 SIG)
  (gr_line (start 0 0) (end 20 0) (layer Edge.Cuts) (width 0.05))
  (gr_line (start 20 0) (end 20 10) (layer Edge.Cuts) (width 0.05))
  (gr_line (start 20 10) (end 0 10) (layer Edge.Cuts) (width 0.05))
  (gr_line (start 0 10) (end 0 0) (layer Edge.Cuts) (width 0.05))
  (module R_0603 (layer F.Cu) (tedit 5A1F2B3C) (at 5 5)
    (attr smd)
    (fp_text reference R1 (at 0 1.5) (layer F.SilkS))
    (fp_text value 10K (at 0 -1.5) (layer F.Fab))
    (pad 1 smd rect (at -0.8 0) (size 0.9 0.8) (layers F.Cu F.Paste F.Mask) (net 1 GND))
    (pad 2 smd rect (at 0.8 0) (size 0.9 0.8) (layers F.Cu F.Paste F.Mask) (net 2 SIG))
  )
  (module C_0402 (layer B.Cu) (tedit 5A1F2B3D) (at 12 5 90)
    (fp_text reference C1 (at 0 1.5) (layer B.SilkS))
    (pad 1 smd rect (at -0.5 0) (size 0.6 0.6) (layers B.Cu B.Paste B.Mask) (net 2 SIG))
  )
  (segment (start 4.2 5) (end 12.8 5) (width 0.25) (layer F.Cu) (net 1))
)
"#;

#[test]
fn legacy_module_boards() {
    let dir = tmp();
    let pcb = Pcb::parse_str(LEGACY_MODULE_BOARD).unwrap();
    assert_eq!(pcb.footprint_count(), 2);
    let refs: Vec<&str> = pcb.footprints().iter().map(|f| f.reference.as_str()).collect();
    assert_eq!(refs, vec!["R1", "C1"]);
    let r1 = pcb.get_footprint("R1").unwrap();
    assert_eq!((r1.name.as_str(), r1.value.as_str(), r1.layer.as_str()), ("R_0603", "10K", "F.Cu"));
    assert_eq!(r1.position, (5.0, 5.0));
    assert_eq!(r1.attr, "smd");
    assert_eq!(pcb.get_footprint("C1").unwrap().rotation, 90.0);
    assert_eq!((r1.pads[0].number.as_str(), r1.pads[1].number.as_str()), ("1", "2"));
    assert_eq!(r1.pads[0].position, (-0.8, 0.0));
    assert_eq!((r1.pads[0].net_number, r1.pads[0].net_name.as_str()), (1, "GND"));
    assert_eq!(r1.pads[0].layers, vec!["F.Cu", "F.Paste", "F.Mask"]);
    assert!(close2(pcb.get_pad_position("R1", "1").unwrap(), (4.2, 5.0)));
    assert!(pcb.parse_warnings.is_empty());
    assert_eq!(pcb.get_board_outline().len(), 5);

    let mut pcb = Pcb::parse_str(LEGACY_MODULE_BOARD).unwrap();
    pcb.footprint_mut("R1").unwrap().set_position((7.5, 6.25));
    let re = save_reload(&mut pcb, dir.path(), "m.kicad_pcb");
    assert_eq!(re.get_footprint("R1").unwrap().position, (7.5, 6.25));
    assert_eq!(re.get_footprint("R1").unwrap().pads.len(), 2);

    let mut pcb = Pcb::parse_str(LEGACY_MODULE_BOARD).unwrap();
    assert!(pcb.update_footprint_reference("R1", "R99"));
    assert!(pcb.update_footprint_value("R99", "22K"));
    let re = save_reload(&mut pcb, dir.path(), "r.kicad_pcb");
    assert!(re.get_footprint("R1").is_none());
    assert_eq!(re.get_footprint("R99").unwrap().value, "22K");

    let mut pcb = Pcb::parse_str(LEGACY_MODULE_BOARD).unwrap();
    assert!(pcb.remove_footprint("R1"));
    assert_eq!(pcb.footprint_count(), 1);
    let out = dir.path().join("x.kicad_pcb");
    pcb.save(Some(&out)).unwrap();
    assert!(!std::fs::read_to_string(&out).unwrap().contains("R_0603"));
    assert!(Pcb::load(&out).unwrap().get_footprint("C1").is_some());
}

#[test]
fn unreadable_component_graph_warns() {
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (version 3) (layers (0 F.Cu signal)) (net 0 "") (net 1 GND)
  (component_v0 R_0603 (layer F.Cu) (at 5 5)
    (fp_text reference R1 (at 0 0) (layer F.SilkS))
    (pad 1 smd rect (at -0.8 0) (size 0.9 0.8) (layers F.Cu) (net 1 GND))))"#,
    )
    .unwrap();
    assert_eq!(pcb.footprint_count(), 0);
    assert_eq!(pcb.parse_warnings.len(), 1);
    assert!(pcb.parse_warnings[0].contains("component_v0"));
    assert!(pcb.parse_warnings[0].contains("version 3"));
}

// ------------------------------------------------------------ attr / lock

const ATTR_BASE: &str = "(kicad_pcb\n\t(version 20240108)\n\t(generator \"pytest\")\n\t(layers\n\t\t(0 \"F.Cu\" signal)\n\t\t(44 \"Edge.Cuts\" user)\n\t)\n\t(net 0 \"\")\n\t(net 1 \"GND\")\n\t(gr_rect (start 100 100) (end 150 150) (stroke (width 0.1) (type default)) (fill none) (layer \"Edge.Cuts\"))\n\t(footprint \"Resistor_SMD:R_0402_1005Metric\"\n\t\t(layer \"F.Cu\")\n\t\t(uuid \"00000000-0000-0000-0000-000000000010\")\n\t\t(at 125 125)\n\t\t(property \"Reference\" \"R1\" (at 0 -1.5 0) (layer \"F.SilkS\"))\n\t\t(property \"Value\" \"10k\" (at 0 1.5 0) (layer \"F.Fab\"))\n\t\t(pad \"1\" smd roundrect (at -0.51 0) (size 0.54 0.64) (layers \"F.Cu\") (net 1 \"GND\"))\n\t)\n)\n";

fn attr_board(inner: Option<&str>) -> Pcb {
    let text = match inner {
        Some(i) => ATTR_BASE.replace("\t\t(at 125 125)\n", &format!("\t\t(at 125 125)\n\t\t(attr {i})\n")),
        None => ATTR_BASE.to_string(),
    };
    Pcb::parse_str(&text).unwrap()
}

#[test]
fn attr_flags_round_trip() {
    let dir = tmp();
    let mut pcb = attr_board(None);
    {
        let fp = pcb.get_footprint("R1").unwrap();
        assert!(!fp.locked && !fp.dnp && fp.attr.is_empty());
    }
    {
        let mut fm = pcb.footprint_mut("R1").unwrap();
        fm.set_attr("smd");
        fm.set_locked(true);
        fm.set_dnp(true);
        fm.set_exclude_from_pos_files(true);
        fm.set_exclude_from_bom(true);
        let p = fm.position;
        fm.set_position((p.0 + 10.0, p.1 - 5.0));
    }
    let mut re = save_reload(&mut pcb, dir.path(), "a.kicad_pcb");
    let fp = re.get_footprint("R1").unwrap();
    assert_eq!(fp.attr, "smd");
    assert!(fp.locked && fp.dnp && fp.exclude_from_pos_files && fp.exclude_from_bom);
    assert!(close2(fp.position, (35.0, 20.0)));
    re.footprint_mut("R1").unwrap().set_attr("through_hole");
    let re2 = save_reload(&mut re, dir.path(), "b.kicad_pcb");
    assert_eq!(re2.get_footprint("R1").unwrap().attr, "through_hole");
}

#[test]
fn attr_unknown_tokens_and_clearing() {
    let dir = tmp();
    let mut pcb = attr_board(Some("smd board_only allow_missing_courtyard"));
    assert_eq!(
        pcb.get_footprint("R1").unwrap().attr_unknown_tokens.0,
        vec!["board_only", "allow_missing_courtyard"]
    );
    pcb.footprint_mut("R1").unwrap().set_locked(true);
    let out = dir.path().join("u.kicad_pcb");
    pcb.save(Some(&out)).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("board_only") && text.contains("allow_missing_courtyard"));
    let re = Pcb::load(&out).unwrap();
    assert!(re.get_footprint("R1").unwrap().locked);

    let mut pcb = attr_board(Some("smd locked dnp"));
    {
        let mut fm = pcb.footprint_mut("R1").unwrap();
        assert!(fm.locked && fm.dnp);
        fm.set_attr("");
        fm.set_locked(false);
        fm.set_dnp(false);
        fm.set_attr_unknown_tokens(vec![]);
    }
    let out = dir.path().join("c.kicad_pcb");
    pcb.save(Some(&out)).unwrap();
    assert!(!std::fs::read_to_string(&out).unwrap().contains("(attr"));
    let fp = Pcb::load(&out).unwrap().get_footprint("R1").unwrap().clone();
    assert!(!fp.locked && !fp.dnp && fp.attr.is_empty());
}

fn locked_text(extra: &str) -> String {
    format!(
        r#"(kicad_pcb (version 20240108) (generator "test")
  (layers (0 "F.Cu" signal) (44 "Edge.Cuts" user))
  (gr_line (start 0 0) (end 10 0) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 10 0) (end 10 10) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 10 10) (end 0 10) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 0 10) (end 0 0) (layer "Edge.Cuts") (width 0.05))
  (net 0 "")
  (footprint "Test:R_0805" (layer "F.Cu") (uuid "fp-r1") (at 5 5)
{extra}
    (property "Reference" "R1" (at 0 -1.5 0) (layer "F.SilkS"))
    (pad "1" smd rect (at -1 0) (size 1 1) (layers "F.Cu") (net 0 ""))
    (pad "2" smd rect (at 1 0) (size 1 1) (layers "F.Cu") (net 0 ""))))"#
    )
}

fn lock_pads(t: &str) -> String {
    t.replace("(pad \"1\" smd rect (at -1 0)", "(pad \"1\" smd rect (locked yes) (at -1 0)")
        .replace("(pad \"2\" smd rect (at 1 0)", "(pad \"2\" smd rect (locked yes) (at 1 0)")
}

fn saved_fp(pcb: &mut Pcb, dir: &Path) -> (String, SExp) {
    let out = dir.join("lock.kicad_pcb");
    pcb.save(Some(&out)).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    let fp = kct::parse(&text).unwrap().find("footprint").unwrap().clone();
    (text, fp)
}

fn legacy_in_attr(text: &str) -> bool {
    text.lines().any(|l| l.contains("(attr") && l.contains("locked"))
}

#[test]
fn locked_save_form() {
    let dir = tmp();
    let mut pcb = Pcb::parse_str(&locked_text("(attr smd)")).unwrap();
    pcb.footprint_mut("R1").unwrap().set_locked(true);
    let (text, fp) = saved_fp(&mut pcb, dir.path());
    assert!(!legacy_in_attr(&text));
    assert_eq!(fp.find_children("locked")[0].string_at(0), Some("yes"));
    assert!(text.contains("(attr smd)"));

    let mut pcb = Pcb::parse_str(&locked_text("(attr smd) (locked yes)")).unwrap();
    assert!(pcb.get_footprint("R1").unwrap().locked);
    pcb.footprint_mut("R1").unwrap().set_locked(false);
    let (_, fp) = saved_fp(&mut pcb, dir.path());
    assert!(fp.find_children("locked").is_empty());

    let mut pcb = Pcb::parse_str(&lock_pads(&locked_text("(attr smd)"))).unwrap();
    assert!(!pcb.get_footprint("R1").unwrap().locked);
    pcb.footprint_mut("R1").unwrap().set_locked(true);
    let (_, fp) = saved_fp(&mut pcb, dir.path());
    assert_eq!(fp.find_children("pad").iter().filter(|p| !p.find_children("locked").is_empty()).count(), 2);
}

#[test]
fn legacy_lock_migration() {
    let dir = tmp();
    let mut pcb = Pcb::parse_str(&locked_text("(attr smd locked)")).unwrap();
    assert!(pcb.get_footprint("R1").unwrap().locked);
    let (text, fp) = saved_fp(&mut pcb, dir.path());
    assert!(!legacy_in_attr(&text), "{text}");
    assert!(text.contains("(attr smd)"));
    assert!(!fp.find_children("locked").is_empty());

    let mut pcb =
        Pcb::parse_str(&locked_text("(attr smd locked exclude_from_bom allow_missing_courtyard)")).unwrap();
    assert!(pcb.get_footprint("R1").unwrap().exclude_from_bom);
    let (_, fp) = saved_fp(&mut pcb, dir.path());
    let attr = fp.find_children("attr")[0];
    let tokens: Vec<String> = attr.atoms().map(|v| v.to_string()).collect();
    assert!(tokens.contains(&"smd".into()) && tokens.contains(&"exclude_from_bom".into()));
    assert!(tokens.contains(&"allow_missing_courtyard".into()) && !tokens.contains(&"locked".into()));

    let mut pcb = Pcb::parse_str(&lock_pads(&locked_text("(attr smd) (locked yes)"))).unwrap();
    pcb.footprint_mut("R1").unwrap().set_locked(false);
    let (text, _) = saved_fp(&mut pcb, dir.path());
    let mut re = Pcb::parse_str(&text).unwrap();
    assert!(!re.get_footprint("R1").unwrap().locked);
    let (_, fp) = saved_fp(&mut re, dir.path());
    assert!(fp.find_children("locked").is_empty());

    let no_uuid = locked_text("(attr smd)")
        .replace("(uuid \"fp-r1\") ", "")
        .replace("(at -1 0)", "(at -1 0) (uuid \"pad-uuid-1\")");
    assert_eq!(Pcb::parse_str(&no_uuid).unwrap().get_footprint("R1").unwrap().uuid, "");
}

// ------------------------------------------------------------ net classes

const LEGACY_HEADER: &str = "(kicad_pcb (version 20171130) (host pcbnew 5.1.9)\n  (general (thickness 1.6))\n  (page A4)\n  (layers (0 F.Cu signal) (31 B.Cu signal) (44 Edge.Cuts user))\n  (setup (pad_to_mask_clearance 0.05) (aux_axis_origin 0 0))\n  (net 0 \"\")\n  (net 1 GND)\n";
const LEGACY_FOOTER: &str = "  (gr_line (start 0 0) (end 50 0) (layer Edge.Cuts) (width 0.1))\n)\n";

#[test]
fn legacy_net_classes() {
    let pcb = Pcb::parse_str(&format!(
        "{LEGACY_HEADER}  (net_class Default \"This is the default net class.\"
    (clearance 0.2) (trace_width 0.25) (via_dia 0.8) (via_drill 0.4) (uvia_dia 0.3) (uvia_drill 0.1)
    (diff_pair_width 0.2) (diff_pair_gap 0.25) (add_net GND) (add_net /SIG))\n{LEGACY_FOOTER}"
    ))
    .unwrap();
    assert_eq!(pcb.net_classes().len(), 1);
    let d = pcb.net_class("Default").unwrap();
    assert_eq!(d.description, "This is the default net class.");
    assert_eq!(
        (d.clearance, d.trace_width, d.via_dia, d.via_drill),
        (Some(0.2), Some(0.25), Some(0.8), Some(0.4))
    );
    assert_eq!((d.uvia_dia, d.uvia_drill, d.diff_pair_width, d.diff_pair_gap), (Some(0.3), Some(0.1), Some(0.2), Some(0.25)));
    assert_eq!(d.nets, vec!["GND", "/SIG"]);
    assert!(close(pcb.setup().unwrap().pad_to_mask_clearance, 0.05));

    let pcb = Pcb::parse_str(&format!(
        "{LEGACY_HEADER}  (net_class Default \"\" (clearance 0.2) (trace_width 0.25))\n  (net_class \"Power\" \"wide rails\" (clearance 0.3) (trace_width 0.8) (add_net VBUS))\n{LEGACY_FOOTER}"
    ))
    .unwrap();
    assert_eq!(pcb.net_classes().len(), 2);
    assert_eq!(pcb.net_class("Power").unwrap().trace_width, Some(0.8));
    assert_eq!(pcb.net_class("Default").unwrap().via_dia, None);
    let mut pcb = pcb;
    pcb.reload_from_tree().unwrap();
    assert_eq!(pcb.net_classes().len(), 2);

    assert!(Pcb::parse_str(&format!("{LEGACY_HEADER}{LEGACY_FOOTER}")).unwrap().net_classes().is_empty());
    let dir = tmp();
    let mut written = create(50.0, 40.0);
    let re = save_reload(&mut written, dir.path(), "w.kicad_pcb");
    assert!(re.net_classes().is_empty());
    assert!(!re.sexp().to_kicad_string().contains("(net_class"));
}

// ------------------------------------------------------------ edge contours

const TWO_OUTLINES: &str = r#"(kicad_pcb (version 20240108) (generator "test")
  (layers (0 "F.Cu" signal) (44 "Edge.Cuts" user)) (net 0 "")
  (gr_rect (start 100 100) (end 150 130) (stroke (width 0.1) (type default)) (fill none) (layer "Edge.Cuts") (uuid "rect-outline-1"))
  (gr_line (start 100 100) (end 160 100) (stroke (width 0.1) (type default)) (layer "Edge.Cuts") (uuid "line-1"))
  (gr_line (start 160 100) (end 160 140) (stroke (width 0.1) (type default)) (layer "Edge.Cuts") (uuid "line-2"))
  (gr_line (start 160 140) (end 100 140) (stroke (width 0.1) (type default)) (layer "Edge.Cuts") (uuid "line-3"))
  (gr_line (start 100 140) (end 100 100) (stroke (width 0.1) (type default)) (layer "Edge.Cuts") (uuid "line-4")))"#;

const WITH_HOLE: &str = r#"(kicad_pcb (version 20240108) (generator "test")
  (layers (0 "F.Cu" signal) (44 "Edge.Cuts" user)) (net 0 "")
  (gr_rect (start 100 100) (end 200 160) (stroke (width 0.1) (type default)) (fill none) (layer "Edge.Cuts") (uuid "main-outline"))
  (gr_circle (center 110 110) (end 112 110) (stroke (width 0.1) (type default)) (fill none) (layer "Edge.Cuts") (uuid "mounting-hole-1")))"#;

const SINGLE_RECT: &str = r#"(kicad_pcb (version 20240108) (generator "test")
  (layers (0 "F.Cu" signal) (44 "Edge.Cuts" user)) (net 0 "")
  (gr_rect (start 100 100) (end 150 130) (stroke (width 0.1) (type default)) (fill none) (layer "Edge.Cuts") (uuid "single-rect")))"#;

#[test]
fn list_edge_contours() {
    let pcb = Pcb::parse_str(TWO_OUTLINES).unwrap();
    let c = pcb.list_edge_contours();
    assert_eq!(c.len(), 2);
    assert_eq!(c.iter().filter(|c| c.element_count == 1).count(), 1);
    assert_eq!(c.iter().filter(|c| c.element_count == 4).count(), 1);

    let c = Pcb::parse_str(SINGLE_RECT).unwrap().list_edge_contours();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].bbox, (100.0, 100.0, 150.0, 130.0));
    assert!(close(c[0].bbox_width(), 50.0) && close(c[0].bbox_height(), 30.0));

    let c = Pcb::parse_str(WITH_HOLE).unwrap().list_edge_contours();
    assert_eq!(c.iter().filter(|c| c.is_mounting_hole).count(), 1);
    assert_eq!(c.iter().filter(|c| !c.is_mounting_hole).count(), 1);

    let none = Pcb::parse_str("(kicad_pcb (version 20240108) (net 0 \"\"))").unwrap();
    assert!(none.list_edge_contours().is_empty());
}

#[test]
fn remove_edge_contour() {
    let dir = tmp();
    for count in [1usize, 4] {
        let mut pcb = Pcb::parse_str(TWO_OUTLINES).unwrap();
        let idx = pcb.list_edge_contours().iter().find(|c| c.element_count == count).unwrap().index;
        assert!(pcb.remove_edge_contour(idx));
        let rest = pcb.list_edge_contours();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].element_count, 5 - count);
        let re = save_reload(&mut pcb, dir.path(), "e.kicad_pcb");
        assert_eq!(re.list_edge_contours()[0].element_count, 5 - count);
    }
    let mut pcb = Pcb::parse_str(SINGLE_RECT).unwrap();
    assert!(!pcb.remove_edge_contour(999));
}

#[test]
fn replace_outline() {
    let mut pcb = Pcb::parse_str(TWO_OUTLINES).unwrap();
    assert_eq!(pcb.replace_outline(50.0, 50.0, 80.0, 40.0), 2);
    let c = pcb.list_edge_contours();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].bbox, (50.0, 50.0, 130.0, 90.0));

    let mut pcb = Pcb::parse_str(WITH_HOLE).unwrap();
    assert_eq!(pcb.replace_outline(50.0, 50.0, 100.0, 60.0), 1);
    let c = pcb.list_edge_contours();
    assert_eq!(c.len(), 2);
    assert_eq!(c.iter().filter(|c| c.is_mounting_hole).count(), 1);

    let dir = tmp();
    let mut pcb = Pcb::parse_str(SINGLE_RECT).unwrap();
    pcb.replace_outline(200.0, 200.0, 60.0, 40.0);
    let re = save_reload(&mut pcb, dir.path(), "r.kicad_pcb");
    assert_eq!(re.list_edge_contours()[0].bbox, (200.0, 200.0, 260.0, 240.0));
}

#[test]
fn board_outline_polygon_and_segments() {
    let pcb = Pcb::parse_str(TWO_OUTLINES).unwrap();
    // The 60x40 line contour beats the 50x30 rect.
    let poly = pcb.get_board_outline();
    assert_eq!(poly.len(), 5);
    let (ox, oy) = pcb.board_origin();
    assert_eq!((ox, oy), (100.0, 100.0));
    assert!(poly.iter().any(|&p| close2(p, (60.0, 40.0))));
    assert_eq!(pcb.get_board_outline_segments().len(), 8);
    let poly = Pcb::parse_str(
        r#"(kicad_pcb (gr_poly (pts (xy 0 0) (xy 10 0) (xy 10 10) (xy 0 10)) (layer "Edge.Cuts")))"#,
    )
    .unwrap();
    assert_eq!(poly.get_board_outline().len(), 5);
    assert_eq!(poly.graphics()[0].points.len(), 4);
    assert!(close2(poly.board_size().unwrap(), (10.0, 10.0)));
}

#[test]
fn malformed_outline_is_tolerated_on_load() {
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (gr_line (start 0) (end 5 5) (layer "Edge.Cuts"))
           (segment (start 1 1) (end 2 2) (width 0.2) (layer "F.Cu") (net 0)))"#,
    )
    .unwrap();
    assert!(!pcb.outline_error.is_empty());
    assert_eq!(pcb.board_origin(), (0.0, 0.0));
    assert!(pcb.board_size().is_err());
    assert_eq!(pcb.segments()[0].start, (1.0, 1.0));
}

// ------------------------------------------------------------ misc

#[test]
fn summary_and_counts() {
    let pcb = load("routing_test.kicad_pcb");
    let s = pcb.summary().unwrap();
    assert_eq!((s.width_mm, s.height_mm, s.area_mm2), (50.0, 40.0, 2000.0));
    assert_eq!((s.copper_layers, s.footprints, s.nets), (2, 3, 4));
    let json = serde_json::to_value(&s).unwrap();
    assert!(json.get("trace_length_mm").is_some());
}

#[test]
fn net_assignment() {
    let dir = tmp();
    let mut pcb = load("routing_test.kicad_pcb");
    assert!(pcb.assign_net_to_footprint_pad("R1", "1", "NEWNET"));
    assert!(!pcb.assign_net_to_footprint_pad("R1", "9", "NEWNET"));
    assert!(!pcb.assign_net_to_footprint_pad("ZZ", "1", "NEWNET"));
    let n = net_number(&pcb, "NEWNET");
    assert_eq!(n, 4);
    let re = save_reload(&mut pcb, dir.path(), "n.kicad_pcb");
    let pad = &re.get_footprint("R1").unwrap().pads[0];
    assert_eq!((pad.net_number, pad.net_name.as_str()), (4, "NEWNET"));
    let mut pcb = load("routing_test.kicad_pcb");
    let stats = pcb.assign_nets_from_netlist(
        &[(
            "X".into(),
            vec![("R1".into(), "2".into()), ("Q9".into(), "1".into()), ("U1".into(), "77".into())],
        )],
        None,
    );
    assert_eq!(stats.assigned, vec!["R1.2"]);
    assert_eq!(stats.missing_footprints, vec!["Q9"]);
    assert_eq!(stats.missing_pads, vec!["U1.77"]);
}

#[test]
fn footprint_texts_graphics_properties() {
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (footprint "Lib:X" (layer "F.Cu") (at 1 2)
          (property "Reference" "U1" (at 0 0) (layer "F.SilkS") (hide yes))
          (property "Value" "V" (at 0 1))
          (property "LCSC" "C123")
          (fp_line (start 0 0) (end 1 0) (stroke (width 0.12) (type solid)) (layer "F.SilkS"))
          (fp_poly (pts (xy 0 0) (xy 1 0) (xy 1 1)) (fill yes) (layer "F.SilkS"))
          (fp_arc (start 1 0) (mid 0.7 0.7) (end 0 1) (layer "F.Fab"))
          (pad "1" thru_hole oval (at 0 0 45) (size 1 2) (drill oval 0.5 0.8 (offset 0.1 0)) (layers "*.Cu") (solder_mask_margin 0.05))))"#,
    )
    .unwrap();
    let fp = &pcb.footprints()[0];
    assert_eq!(fp.property("LCSC"), Some("C123"));
    assert!(fp.texts.iter().find(|t| t.text_type == "reference").unwrap().hidden);
    assert_eq!(fp.graphics.len(), 3);
    assert_eq!(fp.graphics[0].graphic_type, "line");
    assert!(close(fp.graphics[0].stroke_width, 0.12));
    assert!(fp.graphics[1].is_filled() && fp.graphics[1].points.len() == 3);
    assert_eq!(fp.graphics[2].mid, Some((0.7, 0.7)));
    let pad = &fp.pads[0];
    assert_eq!(pad.drill_size, Some((0.5, 0.8)));
    assert_eq!(pad.drill_offset, (0.1, 0.0));
    assert_eq!(pad.rotation, 45.0);
    assert_eq!(pad.solder_mask_margin, Some(0.05));
    assert_eq!(pad.roundrect_rratio, 0.25);
}
