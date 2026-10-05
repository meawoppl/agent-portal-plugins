//! Port of upstream `tests/test_schema.py` (wire/label/schematic parts),
//! `tests/test_schematic.py`, and `tests/test_schematic_editing.py`.

use std::path::{Path, PathBuf};

use kct::parse;
use kct::schema::label::{GlobalLabel, HierarchicalLabel, Label, PowerSymbol};
use kct::schema::library::LibrarySymbol;
use kct::schema::schematic::{AddSymbolOptions, Schematic, SheetInstance, TitleBlock};
use kct::schema::symbol::{OrderedMap, SymbolInstance, SymbolPin, SymbolProperty};
use kct::schema::wire::{Bus, Junction, Wire};

const MINIMAL_SCHEMATIC: &str = r##"(kicad_sch
  (version 20231120)
  (generator "test")
  (generator_version "8.0")
  (uuid "00000000-0000-0000-0000-000000000001")
  (paper "A4")
  (lib_symbols)
  (symbol
    (lib_id "Device:R")
    (at 100 100 0)
    (uuid "00000000-0000-0000-0000-000000000002")
    (property "Reference" "R1" (at 100 90 0) (effects (font (size 1.27 1.27))))
    (property "Value" "10k" (at 100 110 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "Resistor_SMD:R_0402_1005Metric" (at 100 100 0) (effects (hide yes)))
    (property "Datasheet" "" (at 100 100 0) (effects (hide yes)))
    (pin "1" (uuid "00000000-0000-0000-0000-000000000003"))
    (pin "2" (uuid "00000000-0000-0000-0000-000000000004"))
    (instances
      (project "test"
        (path "/00000000-0000-0000-0000-000000000001"
          (reference "R1")
          (unit 1)
        )
      )
    )
  )
  (wire
    (pts (xy 90 100) (xy 100 100))
    (stroke (width 0) (type default))
    (uuid "00000000-0000-0000-0000-000000000005")
  )
  (label "NET1"
    (at 90 100 0)
    (effects (font (size 1.27 1.27)))
    (uuid "00000000-0000-0000-0000-000000000006")
  )
)
"##;

const SCHEMATIC_WITH_LIB: &str = r##"(kicad_sch
  (version 20231120)
  (generator "test")
  (generator_version "8.0")
  (uuid "00000000-0000-0000-0000-000000000001")
  (paper "A4")
  (lib_symbols
    (symbol "Device:R"
      (property "Reference" "R" (at 0 0 0) (effects (font (size 1.27 1.27))))
      (property "Value" "R" (at 0 0 0) (effects (font (size 1.27 1.27))))
      (property "Footprint" "" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes)))
      (property "Datasheet" "" (at 0 0 0) (effects (font (size 1.27 1.27)) (hide yes)))
      (symbol "Device:R_0_1"
        (polyline (pts (xy -1.016 -2.54) (xy -1.016 2.54)) (stroke (width 0) (type default)) (fill (type none)))
      )
      (symbol "Device:R_1_1"
        (pin passive line (at 0 3.81 270) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
        (pin passive line (at 0 -3.81 90) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))
      )
    )
    (symbol "power:GND"
      (property "Reference" "#PWR" (at 0 0 0) (effects (font (size 1.27 1.27))))
      (property "Value" "GND" (at 0 0 0) (effects (font (size 1.27 1.27))))
      (symbol "power:GND_0_1"
        (polyline (pts (xy 0 0) (xy 0 -1.27)) (stroke (width 0) (type default)) (fill (type none)))
      )
      (symbol "power:GND_1_1"
        (pin power_in line (at 0 0 0) (length 0) (name "GND" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
      )
    )
  )
  (sheet_instances
    (path "/" (page "1"))
  )
)
"##;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn minimal() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "test.kicad_sch", MINIMAL_SCHEMATIC);
    (dir, p)
}

fn with_lib() -> (tempfile::TempDir, Schematic) {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "edit_test.kicad_sch", SCHEMATIC_WITH_LIB);
    let sch = Schematic::load(&p).unwrap();
    (dir, sch)
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

// ------------------------------------------------------------------ Wire

#[test]
fn wire_from_sexp() {
    let sexp = parse(
        r##"(wire (pts (xy 90 100) (xy 110 100)) (stroke (width 0.2) (type solid)) (uuid "test-uuid-123"))"##,
    )
    .unwrap();
    let wire = Wire::from_sexp(&sexp);
    assert_eq!(wire.start, (90.0, 100.0));
    assert_eq!(wire.end, (110.0, 100.0));
    assert_eq!(wire.uuid, "test-uuid-123");
    assert_eq!(wire.stroke_width, 0.2);
    assert_eq!(wire.stroke_type, "solid");
}

#[test]
fn wire_from_sexp_minimal() {
    let wire = Wire::from_sexp(&parse("(wire)").unwrap());
    assert_eq!(wire.start, (0.0, 0.0));
    assert_eq!(wire.end, (0.0, 0.0));
    assert_eq!(wire.uuid, "");
}

#[test]
fn wire_length() {
    assert_eq!(Wire::new((0.0, 0.0), (10.0, 0.0)).length(), 10.0);
    assert_eq!(Wire::new((0.0, 0.0), (0.0, 5.0)).length(), 5.0);
    assert_eq!(Wire::new((0.0, 0.0), (3.0, 4.0)).length(), 5.0);
    assert_eq!(Wire::new((5.0, 5.0), (5.0, 5.0)).length(), 0.0);
}

#[test]
fn wire_contains_point() {
    let wire = Wire::new((0.0, 0.0), (10.0, 0.0));
    assert!(wire.contains_point((5.0, 0.0), 0.1));
    assert!(wire.contains_point((0.0, 0.0), 0.1));
    assert!(wire.contains_point((10.0, 0.0), 0.1));
    assert!(!wire.contains_point((5.0, 5.0), 0.1));
    assert!(!wire.contains_point((15.0, 0.0), 0.1));
    assert!(wire.contains_point((5.0, 0.05), 0.1));

    let dot = Wire::new((5.0, 5.0), (5.0, 5.0));
    assert!(dot.contains_point((5.0, 5.0), 0.1));
    assert!(dot.contains_point((5.05, 5.0), 0.1));
    assert!(!dot.contains_point((6.0, 6.0), 0.1));
}

#[test]
fn wire_repr() {
    let s = Wire::new((0.0, 0.0), (10.0, 10.0)).to_string();
    assert!(s.contains("Wire"));
    assert!(s.contains("(0, 0)"));
    assert!(s.contains("(10, 10)"));
}

#[test]
fn wire_to_sexp_round_trip() {
    let mut wire = Wire::new((10.0, 20.0), (30.0, 40.0));
    wire.uuid = "wire-uuid-1".into();
    let parsed = Wire::from_sexp(&wire.to_sexp());
    assert_eq!(parsed.start, (10.0, 20.0));
    assert_eq!(parsed.end, (30.0, 40.0));
    assert_eq!(parsed.uuid, "wire-uuid-1");
    // Reparse from text, too.
    let reparsed = Wire::from_sexp(&parse(&wire.to_sexp().to_kicad_string()).unwrap());
    assert_eq!(reparsed, parsed);
}

// -------------------------------------------------------- Junction / Bus

#[test]
fn junction_from_sexp() {
    let j = Junction::from_sexp(
        &parse(r##"(junction (at 50.8 76.2) (diameter 1.0) (uuid "junction-uuid"))"##).unwrap(),
    );
    assert_eq!(j.position, (50.8, 76.2));
    assert_eq!(j.diameter, 1.0);
    assert_eq!(j.uuid, "junction-uuid");

    let j = Junction::from_sexp(&parse("(junction)").unwrap());
    assert_eq!(j.position, (0.0, 0.0));
    assert_eq!(j.diameter, 0.0);
    assert_eq!(j.uuid, "");
}

#[test]
fn junction_repr_and_round_trip() {
    let s = Junction::new((10.0, 20.0)).to_string();
    assert!(s.contains("Junction") && s.contains("(10, 20)"));
    let mut j = Junction::new((15.0, 25.0));
    j.uuid = "junc-uuid-1".into();
    let parsed = Junction::from_sexp(&j.to_sexp());
    assert_eq!(parsed.position, (15.0, 25.0));
    assert_eq!(parsed.uuid, "junc-uuid-1");
}

#[test]
fn bus_from_sexp() {
    let bus =
        Bus::from_sexp(&parse(r##"(bus (pts (xy 10 20) (xy 30 40)) (uuid "bus-uuid"))"##).unwrap());
    assert_eq!(bus.start, (10.0, 20.0));
    assert_eq!(bus.end, (30.0, 40.0));
    assert_eq!(bus.uuid, "bus-uuid");
    let bus = Bus::from_sexp(&parse("(bus)").unwrap());
    assert_eq!(bus.start, (0.0, 0.0));
    assert_eq!(bus.end, (0.0, 0.0));
}

// ---------------------------------------------------------------- Labels

#[test]
fn label_from_sexp() {
    let l =
        Label::from_sexp(&parse(r##"(label "NET1" (at 50 60 90) (uuid "label-uuid"))"##).unwrap());
    assert_eq!(l.text, "NET1");
    assert_eq!(l.position, (50.0, 60.0));
    assert_eq!(l.rotation, 90.0);
    assert_eq!(l.uuid, "label-uuid");

    let l = Label::from_sexp(&parse(r##"(label "VCC" (at 10 20) (uuid "uuid"))"##).unwrap());
    assert_eq!(l.text, "VCC");
    assert_eq!(l.rotation, 0.0);

    let s = Label::new("GND", (0.0, 0.0)).to_string();
    assert!(s.contains("Label") && s.contains("GND"));
}

#[test]
fn hierarchical_label_from_sexp() {
    let l = HierarchicalLabel::from_sexp(
        &parse(
            r##"(hierarchical_label "CLK" (shape output) (at 100 50 180) (uuid "hlabel-uuid"))"##,
        )
        .unwrap(),
    );
    assert_eq!(l.text, "CLK");
    assert_eq!(l.shape, "output");
    assert_eq!(l.position, (100.0, 50.0));
    assert_eq!(l.rotation, 180.0);
    assert_eq!(l.uuid, "hlabel-uuid");

    for shape in ["input", "output", "bidirectional", "tri_state", "passive"] {
        let text = format!(r##"(hierarchical_label "SIG" (shape {shape}) (at 0 0))"##);
        assert_eq!(
            HierarchicalLabel::from_sexp(&parse(&text).unwrap()).shape,
            shape
        );
    }
    let l =
        HierarchicalLabel::from_sexp(&parse(r##"(hierarchical_label "SIG" (at 0 0))"##).unwrap());
    assert_eq!(l.shape, "input");

    let mut l = HierarchicalLabel::new("DATA", (0.0, 0.0));
    l.shape = "bidirectional".into();
    let s = l.to_string();
    assert!(s.contains("HierarchicalLabel") && s.contains("DATA") && s.contains("bidirectional"));
}

#[test]
fn global_label_from_sexp() {
    let l = GlobalLabel::from_sexp(
        &parse(r##"(global_label "RESET" (shape input) (at 75 25 0) (uuid "glabel-uuid"))"##)
            .unwrap(),
    );
    assert_eq!(l.text, "RESET");
    assert_eq!(l.shape, "input");
    assert_eq!(l.position, (75.0, 25.0));
    assert_eq!(l.rotation, 0.0);
    assert_eq!(l.uuid, "glabel-uuid");
    let s = GlobalLabel::new("SPI_CLK", (0.0, 0.0)).to_string();
    assert!(s.contains("GlobalLabel") && s.contains("SPI_CLK"));
}

#[test]
fn label_to_sexp_round_trips() {
    let mut g = GlobalLabel::new("VBUS", (1.27, 2.54));
    g.rotation = 180.0;
    g.shape = "output".into();
    g.uuid = "g".into();
    let node = g.to_sexp();
    assert!(node.to_kicad_string().contains("(justify right)"));
    assert_eq!(
        GlobalLabel::from_sexp(&parse(&node.to_kicad_string()).unwrap()),
        g
    );

    let mut h = HierarchicalLabel::new("SDA", (3.0, 4.0));
    h.uuid = "h".into();
    assert_eq!(HierarchicalLabel::from_sexp(&h.to_sexp()), h);

    let mut l = Label::new("N", (5.0, 6.0));
    l.uuid = "l".into();
    assert_eq!(Label::from_sexp(&l.to_sexp()), l);
}

#[test]
fn power_symbol_from_sexp() {
    let sexp = parse(
        r##"(symbol (lib_id "power:GND") (at 50 100 0) (uuid "power-uuid")
            (property "Reference" "#PWR01" (at 0 0 0)) (property "Value" "GND" (at 0 0 0)))"##,
    )
    .unwrap();
    let p = PowerSymbol::from_symbol_sexp(&sexp).unwrap();
    assert_eq!(p.lib_id, "power:GND");
    assert_eq!(p.position, (50.0, 100.0));
    assert_eq!(p.value, "GND");
    assert_eq!(p.uuid, "power-uuid");

    let sexp = parse(
        r##"(symbol (lib_id "power:+5V") (at 30 40 0) (property "Value" "+5V" (at 0 0 0)))"##,
    )
    .unwrap();
    let p = PowerSymbol::from_symbol_sexp(&sexp).unwrap();
    assert_eq!(p.lib_id, "power:+5V");
    assert_eq!(p.value, "+5V");

    let sexp = parse(r##"(symbol (lib_id "Device:R") (at 50 100 0))"##).unwrap();
    assert!(PowerSymbol::from_symbol_sexp(&sexp).is_none());
}

// ----------------------------------------------------- TitleBlock / Sheet

#[test]
fn title_block_from_sexp() {
    let tb = TitleBlock::from_sexp(
        &parse(
            r##"(title_block (title "Test Project") (date "2024-01-15") (rev "1.0")
                (company "ACME Corp") (comment 1 "First comment") (comment 2 "Second comment"))"##,
        )
        .unwrap(),
    );
    assert_eq!(tb.title, "Test Project");
    assert_eq!(tb.date, "2024-01-15");
    assert_eq!(tb.rev, "1.0");
    assert_eq!(tb.company, "ACME Corp");
    assert_eq!(tb.comments[&1], "First comment");
    assert_eq!(tb.comments[&2], "Second comment");

    let tb = TitleBlock::from_sexp(&parse("(title_block)").unwrap());
    assert_eq!(tb, TitleBlock::default());
}

#[test]
fn schematic_sheet_instance_from_sexp() {
    let sheet = SheetInstance::from_sexp(
        &parse(
            r##"(sheet (at 100 50) (size 76.2 50.8) (uuid "sheet-uuid")
                (property "Sheetname" "Power Supply") (property "Sheetfile" "power.kicad_sch"))"##,
        )
        .unwrap(),
    );
    assert_eq!(sheet.name, "Power Supply");
    assert_eq!(sheet.filename, "power.kicad_sch");
    assert_eq!(sheet.uuid, "sheet-uuid");
    assert_eq!(sheet.position, (100.0, 50.0));
    assert_eq!(sheet.size, (76.2, 50.8));
}

// ------------------------------------------------------------- Schematic

#[test]
fn schematic_load_and_header() {
    let (_d, p) = minimal();
    let sch = Schematic::load(&p).unwrap();
    assert_eq!(sch.path(), Some(p.as_path()));
    assert_eq!(sch.symbols().len(), 1);
    assert_eq!(sch.version(), Some(20231120));
    assert_eq!(sch.generator().as_deref(), Some("test"));
    assert_eq!(sch.paper().as_deref(), Some("A4"));
    assert_eq!(
        sch.uuid().as_deref(),
        Some("00000000-0000-0000-0000-000000000001")
    );
}

#[test]
fn schematic_rejects_non_schematic() {
    let err = Schematic::new(parse("(kicad_pcb)").unwrap(), None).unwrap_err();
    assert!(err.to_string().contains("Not a schematic: kicad_pcb"));
    assert!(Schematic::load("/nonexistent/x.kicad_sch")
        .unwrap_err()
        .to_string()
        .contains("not found"));
}

#[test]
fn schematic_symbols_and_queries() {
    let (_d, p) = minimal();
    let sch = Schematic::load(&p).unwrap();
    assert_eq!(sch.symbols()[0].reference(), "R1");
    let sym = sch.get_symbol("R1").unwrap();
    assert_eq!(sym.value(), "10k");
    assert_eq!(sym.footprint(), "Resistor_SMD:R_0402_1005Metric");
    assert_eq!(sym.datasheet(), "");
    assert_eq!(sym.project_name, "test");
    assert_eq!(sym.instance_path, "/00000000-0000-0000-0000-000000000001");
    assert!(!sym.properties["Footprint"].visible);
    assert!(sym.properties["Reference"].visible);
    assert_eq!(sym.pins.len(), 2);
    assert!(sch.get_symbol("R99").is_none());
    let by_lib = sch.find_symbols_by_lib("Device:R");
    assert_eq!(by_lib.len(), 1);
    assert_eq!(by_lib[0].reference(), "R1");
    assert_eq!(sch.iter_symbols().count(), 1);
}

#[test]
fn schematic_wires_labels_junctions() {
    let (_d, p) = minimal();
    let sch = Schematic::load(&p).unwrap();
    assert_eq!(sch.wires().len(), 1);
    assert_eq!(sch.wires()[0].start, (90.0, 100.0));
    assert_eq!(sch.wires()[0].end, (100.0, 100.0));
    assert_eq!(sch.labels().len(), 1);
    assert_eq!(sch.labels()[0].text, "NET1");
    assert!(sch.junctions().is_empty());
    assert!(sch.no_connects().is_empty());
    assert!(!sch.is_hierarchical());
}

#[test]
fn schematic_invalidate_cache() {
    let (_d, p) = minimal();
    let mut sch = Schematic::load(&p).unwrap();
    let _ = sch.symbols();
    let _ = sch.wires();
    assert!(sch.is_symbols_cached() && sch.is_wires_cached());
    sch.invalidate_cache();
    assert!(!sch.is_symbols_cached());
    assert!(!sch.is_wires_cached());
}

#[test]
fn schematic_save_and_repr() {
    let (d, p) = minimal();
    let sch = Schematic::load(&p).unwrap();
    let new_path = d.path().join("saved.kicad_sch");
    sch.save(Some(&new_path)).unwrap();
    assert!(new_path.exists());
    assert_eq!(Schematic::load(&new_path).unwrap().symbols().len(), 1);
    let s = sch.to_string();
    assert!(s.contains("Schematic") && s.contains("symbols=1"));

    let unsaved = Schematic::new(parse("(kicad_sch)").unwrap(), None).unwrap();
    assert!(unsaved
        .save(None)
        .unwrap_err()
        .to_string()
        .contains("No path specified"));
}

#[test]
fn simple_rc_fixture() {
    let sch = Schematic::load(fixtures().join("simple_rc.kicad_sch")).unwrap();
    let refs: Vec<&str> = sch.symbols().iter().map(|s| s.reference()).collect();
    assert_eq!(refs, ["R1", "C1"]);
    // Every placed symbol has its definition embedded.
    for sym in sch.symbols() {
        let lib = sch.get_lib_symbol_resolved(&sym.lib_id).unwrap().unwrap();
        assert!(lib.pin_count() >= 2, "{}", sym.lib_id);
    }
}

#[test]
fn untouched_save_is_stable() {
    // Load -> save -> load -> save gives identical bytes.
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.kicad_sch");
    let b = dir.path().join("b.kicad_sch");
    Schematic::load(fixtures().join("simple_rc.kicad_sch"))
        .unwrap()
        .save(Some(&a))
        .unwrap();
    Schematic::load(&a).unwrap().save(Some(&b)).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
}

#[test]
fn hierarchical_fixture_sheets() {
    let sch = Schematic::load(fixtures().join("projects/hierarchical_main.kicad_sch")).unwrap();
    assert!(sch.is_hierarchical());
    let names: Vec<&str> = sch.sheets().iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Logic", "Output"]);
    assert_eq!(sch.sheets()[0].filename, "logic_subsheet.kicad_sch");
}

#[test]
fn pin_positions_on_placed_symbols() {
    // mosfets fixture: Q1A at (100, 60) rotation 0 using Device:Q_NMOS_GDS.
    let sch = Schematic::load(fixtures().join("component_stress/mosfets.kicad_sch")).unwrap();
    let sym = sch
        .symbols()
        .iter()
        .find(|s| s.reference() == "Q1A")
        .unwrap();
    let lib = sch.get_lib_symbol_resolved(&sym.lib_id).unwrap().unwrap();
    let pos = lib.get_all_pin_positions(sym.position, sym.rotation, &sym.mirror);
    assert_eq!(pos["1"], (100.0 - 5.08, 60.0));
    assert_eq!(pos["2"], (100.0, 60.0 - 5.08));
    assert_eq!(pos["3"], (100.0, 60.0 + 5.08));
    let names: Vec<&str> = lib.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["G", "D", "S"]);
}

// ------------------------------------------------- schematic.py parsing via doc

#[test]
fn parse_schematic_from_document() {
    let (_d, p) = minimal();
    let doc = kct::Document::load(&p).unwrap();
    assert!(doc.root.has_tag("kicad_sch"));
    let sch = Schematic::new(doc.root, None).unwrap();
    assert_eq!(sch.symbols()[0].reference(), "R1");
    assert_eq!(sch.symbols()[0].value(), "10k");
    assert_eq!(sch.wires()[0].start, (90.0, 100.0));
    assert_eq!(sch.labels()[0].text, "NET1");
}

// ------------------------------------------------------------- editing API

#[test]
fn symbol_property_round_trip() {
    let prop = SymbolProperty::new("Reference", "R1", (10.0, 20.0), 0.0, true);
    let parsed = SymbolProperty::from_sexp(&prop.to_sexp());
    assert_eq!(parsed.name, "Reference");
    assert_eq!(parsed.value, "R1");
    assert!(parsed.visible);

    let prop = SymbolProperty::new("Footprint", "SMD:R_0402", (5.0, 5.0), 0.0, false);
    let parsed = SymbolProperty::from_sexp(&prop.to_sexp());
    assert_eq!(parsed.value, "SMD:R_0402");
    assert!(!parsed.visible);
}

#[test]
fn symbol_pin_round_trip() {
    let pin = SymbolPin::new("1", "aaaa-bbbb");
    let sexp = pin.to_sexp();
    assert_eq!(sexp.to_compact_string(), r#"(pin "1" (uuid "aaaa-bbbb"))"#);
    let parsed = SymbolPin::from_sexp(&sexp);
    assert_eq!(parsed.number, "1");
    assert_eq!(parsed.uuid, "aaaa-bbbb");
}

#[test]
fn symbol_instance_round_trip() {
    let mut props = OrderedMap::new();
    props.insert(
        "Reference",
        SymbolProperty::new("Reference", "C1", (50.0, 60.0), 0.0, true),
    );
    props.insert(
        "Value",
        SymbolProperty::new("Value", "100nF", (50.0, 62.0), 0.0, true),
    );
    props.insert(
        "Footprint",
        SymbolProperty::new("Footprint", "SMD:C_0402", (50.0, 60.0), 0.0, false),
    );
    props.insert(
        "Datasheet",
        SymbolProperty::new("Datasheet", "", (50.0, 60.0), 0.0, false),
    );
    let inst = SymbolInstance {
        lib_id: "Device:C".into(),
        uuid: "sym-uuid-1".into(),
        position: (50.0, 60.0),
        rotation: 90.0,
        mirror: "x".into(),
        properties: props,
        pins: vec![
            SymbolPin::new("1", "pin-uuid-1"),
            SymbolPin::new("2", "pin-uuid-2"),
        ],
        project_name: "proj".into(),
        instance_path: "/root".into(),
        ..Default::default()
    };
    let text = inst.to_sexp().to_kicad_string();
    let parsed = SymbolInstance::from_sexp(&parse(&text).unwrap());
    assert_eq!(parsed.lib_id, "Device:C");
    assert_eq!(parsed.uuid, "sym-uuid-1");
    assert_eq!(parsed.position, (50.0, 60.0));
    assert_eq!(parsed.rotation, 90.0);
    assert_eq!(parsed.mirror, "x");
    assert!(parsed.in_bom && parsed.on_board && !parsed.dnp);
    assert_eq!(parsed.reference(), "C1");
    assert_eq!(parsed.value(), "100nF");
    assert_eq!(parsed.footprint(), "SMD:C_0402");
    assert_eq!(parsed.pins.len(), 2);
    assert_eq!(parsed.project_name, "proj");
    assert_eq!(parsed.instance_path, "/root");
    assert_eq!(parsed, inst);
    assert!(inst.to_string().contains("SymbolInstance(\"C1\""));
}

#[test]
fn add_symbol_basic() {
    let (_d, mut sch) = with_lib();
    assert!(sch.symbols().is_empty());
    let sym = sch
        .add_symbol(
            "Device:R",
            "R1",
            "10k",
            "Resistor_SMD:R_0402",
            (100.0, 100.0),
            AddSymbolOptions::default(),
        )
        .unwrap();
    assert_eq!(sym.reference(), "R1");
    assert_eq!(sym.value(), "10k");
    assert_eq!(sym.lib_id, "Device:R");
    assert_eq!(sym.position, (100.0, 100.0));
    assert_eq!(sym.pins.len(), 2);
    assert!(sym.in_bom && sym.on_board);
    assert_eq!(sch.symbols().len(), 1);
    assert_eq!(sch.symbols()[0].reference(), "R1");
    assert_eq!(sch.symbols()[0], sym);
}

#[test]
fn add_symbol_with_rotation_and_errors() {
    let (_d, mut sch) = with_lib();
    let sym = sch
        .add_symbol(
            "Device:R",
            "R2",
            "4.7k",
            "Resistor_SMD:R_0402",
            (120.0, 80.0),
            AddSymbolOptions {
                rotation: 90.0,
                mirror: "x".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(sym.rotation, 90.0);
    assert_eq!(sym.mirror, "x");

    let err = sch
        .add_symbol(
            "Device:C",
            "C1",
            "100nF",
            "SMD:C_0402",
            (50.0, 50.0),
            AddSymbolOptions::default(),
        )
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("not found in schematic lib_symbols"));

    let sym = sch
        .add_symbol(
            "Device:C",
            "C1",
            "100nF",
            "SMD:C_0402",
            (50.0, 50.0),
            AddSymbolOptions {
                pin_numbers: Some(vec!["1".into(), "2".into()]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(sym.pins.len(), 2);
}

#[test]
fn add_multiple_symbols_before_sheet_instances() {
    let (_d, mut sch) = with_lib();
    let o = AddSymbolOptions::default;
    sch.add_symbol("Device:R", "R1", "10k", "SMD:R_0402", (100.0, 100.0), o())
        .unwrap();
    sch.add_symbol("Device:R", "R2", "4.7k", "SMD:R_0402", (120.0, 100.0), o())
        .unwrap();
    assert_eq!(sch.symbols().len(), 2);
    let last_named = sch
        .sexp()
        .children
        .iter()
        .filter_map(|c| c.tag())
        .next_back();
    assert_eq!(last_named, Some("sheet_instances"));
}

#[test]
fn add_power_gnd() {
    let (_d, mut sch) = with_lib();
    let sym = sch.add_power("GND", (100.0, 110.0), 0.0, "", "").unwrap();
    assert_eq!(sym.lib_id, "power:GND");
    assert!(!sym.in_bom && !sym.on_board);
    assert_eq!(sym.position, (100.0, 110.0));
    assert_eq!(sym.pins.len(), 1);
    assert_eq!(sym.reference(), "#PWR01");
    assert_eq!(sch.symbols().len(), 1);
    let second = sch.add_power("GND", (110.0, 110.0), 0.0, "", "").unwrap();
    assert_eq!(second.reference(), "#PWR02");
    let flag = sch.add_power("PWR_FLAG", (0.0, 0.0), 0.0, "", "").unwrap();
    assert_eq!(flag.reference(), "#FLG01");
    assert_eq!(flag.pins.len(), 1); // default pin "1"
}

#[test]
fn add_wire_label_junction() {
    let (_d, mut sch) = with_lib();
    assert!(sch.wires().is_empty());
    let wire = sch.add_wire((90.0, 100.0), (110.0, 100.0));
    assert_eq!(wire.start, (90.0, 100.0));
    assert_eq!(wire.end, (110.0, 100.0));
    assert_eq!(wire.uuid.len(), 36);
    assert_eq!(sch.wires().len(), 1);

    sch.add_label("NET1", (90.0, 100.0), 0.0);
    sch.add_global_label("VBUS", (1.0, 2.0), 180.0, "output");
    sch.add_hierarchical_label("SDA", (3.0, 4.0), 0.0, "bidirectional");
    sch.add_junction((100.0, 100.0));
    assert_eq!(sch.labels()[0].text, "NET1");
    assert_eq!(sch.global_labels()[0].shape, "output");
    assert_eq!(sch.hierarchical_labels()[0].shape, "bidirectional");
    assert_eq!(sch.junctions()[0].position, (100.0, 100.0));
}

#[test]
fn embed_lib_symbol() {
    let (_d, mut sch) = with_lib();
    sch.embed_lib_symbol(&LibrarySymbol::new("Device:R"));
    let count = sch
        .lib_symbols()
        .unwrap()
        .find_all("symbol")
        .filter(|s| s.string_at(0) == Some("Device:R"))
        .count();
    assert_eq!(count, 1);

    let mut c = LibrarySymbol::new("Device:C");
    c.add_pin("1", "~", "passive", (0.0, 3.81), 270.0, 1.27, 1, "line")
        .unwrap();
    c.add_pin("2", "~", "passive", (0.0, -3.81), 90.0, 1.27, 1, "line")
        .unwrap();
    sch.embed_lib_symbol(&c);
    assert!(sch.get_lib_symbol("Device:C").is_some());
    let resolved = sch.get_lib_symbol_resolved("Device:C").unwrap().unwrap();
    assert_eq!(resolved.pin_count(), 2);
}

#[test]
fn embed_creates_lib_symbols_section() {
    let mut sch = Schematic::new(
        parse(r##"(kicad_sch (version 20231120) (generator "t") (uuid "u") (paper "A4") (symbol (lib_id "x")))"##)
            .unwrap(),
        None,
    )
    .unwrap();
    sch.embed_lib_symbol(&LibrarySymbol::new("Device:R"));
    let tags: Vec<&str> = sch.sexp().children.iter().filter_map(|c| c.tag()).collect();
    assert_eq!(
        tags,
        [
            "version",
            "generator",
            "uuid",
            "paper",
            "lib_symbols",
            "symbol"
        ]
    );
}

#[test]
fn replace_lib_symbol() {
    let (_d, mut sch) = with_lib();
    let mut r = LibrarySymbol::new("Device:R");
    r.add_simple_pin("1", "A", "passive", (0.0, 0.0), 0.0)
        .unwrap();
    assert!(sch.replace_lib_symbol("Device:R", &r));
    let resolved = sch.get_lib_symbol_resolved("Device:R").unwrap().unwrap();
    assert_eq!(resolved.pin_count(), 1);
    assert!(!sch.replace_lib_symbol("Device:Missing", &LibrarySymbol::new("Device:New")));
    assert!(sch.get_lib_symbol("Device:New").is_some());
}

#[test]
fn find_nearest_connection_point() {
    let (_d, mut sch) = with_lib();
    sch.add_symbol(
        "Device:R",
        "R1",
        "10k",
        "SMD",
        (100.0, 100.0),
        AddSymbolOptions::default(),
    )
    .unwrap();
    sch.add_wire((50.0, 50.0), (60.0, 50.0));
    // Device:R pin 1 at (0, 3.81) -> sheet (100, 96.19).
    let p = sch
        .find_nearest_connection_point((100.5, 96.0), 2.0)
        .unwrap()
        .unwrap();
    assert!(approx(p.0, 100.0) && approx(p.1, 96.19));
    let p = sch
        .find_nearest_connection_point((59.0, 50.5), 2.0)
        .unwrap()
        .unwrap();
    assert_eq!(p, (60.0, 50.0));
    assert!(sch
        .find_nearest_connection_point((0.0, 0.0), 2.0)
        .unwrap()
        .is_none());
}

#[test]
fn snap_to_grid() {
    assert!(approx(Schematic::snap_to_grid(88.91, 1.27), 88.9));
    assert!(approx(Schematic::snap_to_grid(2.54, 1.27), 2.54));
    assert!(approx(Schematic::snap_to_grid(3.0, 2.54), 2.54));
}

#[test]
fn snap_all_to_grid() {
    let (_d, mut sch) = with_lib();
    sch.add_symbol(
        "Device:R",
        "R1",
        "10k",
        "SMD:R_0402",
        (88.91, 101.5),
        AddSymbolOptions {
            pin_numbers: Some(vec!["1".into(), "2".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    sch.add_wire((88.91, 101.5), (92.0, 101.5));
    sch.snap_all_to_grid(1.27);
    let sym = &sch.symbols()[0];
    assert!(approx(sym.position.0, 88.9));
    assert!(approx(sym.position.1, 101.6));
    let wire = &sch.wires()[0];
    assert!(approx(wire.start.0, 88.9));
    assert!(approx(wire.end.0, 91.44));
}

#[test]
fn add_and_save_reload() {
    let (d, mut sch) = with_lib();
    sch.add_symbol(
        "Device:R",
        "R1",
        "10k",
        "SMD:R_0402",
        (100.0, 100.0),
        AddSymbolOptions::default(),
    )
    .unwrap();
    sch.add_wire((90.0, 100.0), (100.0, 100.0));
    let out = d.path().join("output.kicad_sch");
    sch.save(Some(&out)).unwrap();
    let sch2 = Schematic::load(&out).unwrap();
    assert_eq!(sch2.symbols().len(), 1);
    assert_eq!(sch2.symbols()[0].reference(), "R1");
    assert_eq!(sch2.wires().len(), 1);
    assert_eq!(sch2.wires()[0].start, (90.0, 100.0));
    assert_eq!(sch2.wires()[0].end, (100.0, 100.0));
}

#[test]
fn edits_preserve_existing_content() {
    let (d, p) = minimal();
    let mut sch = Schematic::load(&p).unwrap();
    let (n_sym, n_wire, n_label) = (sch.symbols().len(), sch.wires().len(), sch.labels().len());
    sch.add_symbol(
        "Device:R",
        "R99",
        "1M",
        "SMD:R_0402",
        (150.0, 150.0),
        AddSymbolOptions {
            pin_numbers: Some(vec!["1".into(), "2".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    sch.add_wire((140.0, 150.0), (150.0, 150.0));
    let out = d.path().join("round_trip.kicad_sch");
    sch.save(Some(&out)).unwrap();
    let sch2 = Schematic::load(&out).unwrap();
    assert_eq!(sch2.symbols().len(), n_sym + 1);
    assert_eq!(sch2.wires().len(), n_wire + 1);
    assert_eq!(sch2.labels().len(), n_label);
}

#[test]
fn set_symbol_property_edits_tree() {
    let (d, p) = minimal();
    let mut sch = Schematic::load(&p).unwrap();
    assert!(sch.set_symbol_property("R1", "Value", "22k"));
    assert!(!sch.set_symbol_property("R1", "Nope", "x"));
    assert!(!sch.set_symbol_property("R404", "Value", "x"));
    assert_eq!(sch.get_symbol("R1").unwrap().value(), "22k");
    let out = d.path().join("edited.kicad_sch");
    sch.save(Some(&out)).unwrap();
    assert_eq!(
        Schematic::load(&out)
            .unwrap()
            .get_symbol("R1")
            .unwrap()
            .value(),
        "22k"
    );
}
