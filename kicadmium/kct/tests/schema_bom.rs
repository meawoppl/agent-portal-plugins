//! Port of upstream BOM tests (`tests/test_schema.py` BOM classes,
//! `tests/test_bom_footprint_backfill.py`), plus field geometry and
//! physical identity coverage.

use std::path::{Path, PathBuf};

use kct::schema::bom::{
    backfill_footprints_from_pcb, extract_bom, extract_bom_from_pcb, extract_bom_from_schematic,
    fnmatch, BOMGroup, BOMItem, BOM,
};
use kct::schema::field_geometry::{
    default_field_positions, field_offset_mm, placed_body_bbox, DEFAULT_FIELD_CLEARANCE_MM,
};
use kct::schema::library::{LibraryPin, LibrarySymbol};
use kct::schema::physical_identity::{
    component_keys, footprint_keys, physical_pad_position, validate_netlist_selectors, ComponentRef,
};
use kct::schema::schematic::Schematic;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn item(reference: &str, value: &str, footprint: &str, lib_id: &str) -> BOMItem {
    BOMItem::new(reference, value, footprint, lib_id)
}

fn r(reference: &str, value: &str, footprint: &str) -> BOMItem {
    item(reference, value, footprint, "Device:R")
}

// ------------------------------------------------------------- BOMItem

#[test]
fn bom_item_power_and_virtual() {
    assert!(item("#PWR01", "GND", "", "power:GND").is_power_symbol());
    assert!(!r("R1", "10k", "R_0402").is_power_symbol());
    assert!(item("X1", "VIN", "", "kicad_tools_pwr:VIN").is_power_symbol());
    assert!(item("#PWR02", "V", "", "").is_power_symbol());

    assert!(item("#PWR01", "GND", "", "power:GND").is_virtual());
    let mut tp = item("TP1", "Test", "", "Connector:TestPoint");
    tp.in_bom = false;
    assert!(tp.is_virtual());
    assert!(!r("R1", "10k", "R_0402").is_virtual());
}

#[test]
fn bom_item_to_dict_keys() {
    let mut i = r("R1", "10k", "R_0402");
    i.lcsc = "C25744".into();
    let json = serde_json::to_string(&i.to_dict()).unwrap();
    assert_eq!(
        json,
        r#"{"reference":"R1","value":"10k","footprint":"R_0402","description":"","manufacturer":"","mpn":"","lcsc":"C25744","dnp":false}"#
    );
}

// ------------------------------------------------------------ BOMGroup

#[test]
fn bom_group_properties() {
    let g = BOMGroup::new(
        "10k",
        "R_0402",
        vec![
            r("R1", "10k", "R_0402"),
            r("R2", "10k", "R_0402"),
            r("R3", "10k", "R_0402"),
        ],
    );
    assert_eq!(g.quantity(), 3);

    let c = |n: &str| item(n, "100nF", "C_0402", "Device:C");
    let g = BOMGroup::new("100nF", "C_0402", vec![c("C10"), c("C2"), c("C1")]);
    assert_eq!(g.references(), "C1, C2, C10");

    let mut with_lcsc = r("R2", "10k", "R_0402");
    with_lcsc.lcsc = "C25744".into();
    let g = BOMGroup::new("10k", "R_0402", vec![r("R1", "10k", "R_0402"), with_lcsc]);
    assert_eq!(g.lcsc(), "C25744");
    assert_eq!(
        BOMGroup::new("10k", "R_0402", vec![r("R1", "10k", "R_0402")]).lcsc(),
        ""
    );

    let mut m = r("R1", "10k", "R_0402");
    m.mpn = "RC0402FR-0710KL".into();
    m.description = "Resistor".into();
    let g = BOMGroup::new("10k", "R_0402", vec![m]);
    assert_eq!(g.mpn(), "RC0402FR-0710KL");
    assert_eq!(g.description(), "Resistor");
    let d = g.to_dict();
    assert_eq!((d.qty, d.refs.as_str(), d.items.len()), (1, "R1", 1));
}

// ----------------------------------------------------------------- BOM

#[test]
fn bom_counts() {
    let mut dnp = r("R3", "10k", "R_0402");
    dnp.dnp = true;
    let bom = BOM::new(vec![
        r("R1", "10k", "R_0402"),
        r("R2", "10k", "R_0402"),
        item("#PWR01", "GND", "", "power:GND"),
        dnp,
    ]);
    assert_eq!(bom.total_components(), 2);
    assert_eq!(bom.dnp_count(), 1);
}

#[test]
fn bom_grouping_modes() {
    let bom = BOM::new(vec![
        r("R1", "10k", "R_0402"),
        r("R2", "10k", "R_0402"),
        r("R3", "10k", "R_0603"),
        r("R4", "4.7k", "R_0402"),
    ]);
    assert_eq!(bom.grouped("value+footprint").len(), 3);

    let bom = BOM::new(vec![
        r("R1", "10k", "R_0402"),
        r("R2", "10k", "R_0603"),
        r("R3", "4.7k", "R_0402"),
    ]);
    assert_eq!(bom.grouped("value").len(), 2);

    let bom = BOM::new(vec![
        r("R1", "10k", "R_0402"),
        r("R2", "4.7k", "R_0402"),
        r("R3", "10k", "R_0603"),
    ]);
    assert_eq!(bom.grouped("footprint").len(), 2);

    let with_mpn = |n: &str, mpn: &str| {
        let mut i = r(n, "10k", "R_0402");
        i.mpn = mpn.into();
        i
    };
    let bom = BOM::new(vec![
        with_mpn("R1", "RC0402FR-0710KL"),
        with_mpn("R2", "RC0402FR-0710KL"),
        with_mpn("R3", "ERJ-2RKF1002X"),
    ]);
    assert_eq!(bom.grouped("mpn").len(), 2);

    let bom = BOM::new(vec![
        r("R1", "10k", "R_0402"),
        item("#PWR01", "GND", "", "power:GND"),
    ]);
    let groups = bom.grouped("value+footprint");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].items[0].reference, "R1");

    // Issue #439: empty references must not break sorting.
    let bom = BOM::new(vec![
        r("R1", "10k", "R_0402"),
        r("", "10k", "R_0402"),
        item("C1", "100nF", "C_0402", "Device:C"),
    ]);
    let groups = bom.grouped("value+footprint");
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].value, "100nF"); // sorted by ref prefix: C before R

    let bom = BOM::new(vec![
        r("R1", "10k", "R_0402"),
        r("R2", "10k", "R_0402"),
        item("C1", "100nF", "C_0402", "Device:C"),
    ]);
    assert_eq!(bom.unique_parts(), 2);
}

#[test]
fn bom_filtering() {
    let mut dnp = r("R2", "10k", "R_0402");
    dnp.dnp = true;
    let bom = BOM::new(vec![r("R1", "10k", "R_0402"), dnp]);
    let f = bom.filter(false, None);
    assert_eq!(f.items.len(), 1);
    assert_eq!(f.items[0].reference, "R1");
    assert_eq!(bom.filter(true, None).items.len(), 2);

    let bom = BOM::new(vec![
        r("R1", "10k", "R_0402"),
        r("R2", "10k", "R_0402"),
        item("C1", "100nF", "C_0402", "Device:C"),
        item("U1", "ATmega", "QFP", "MCU:ATmega"),
    ]);
    let f = bom.filter(false, Some("R*"));
    assert_eq!(f.items.len(), 2);
    assert!(f.items.iter().all(|i| i.reference.starts_with('R')));
}

#[test]
fn fnmatch_patterns() {
    assert!(fnmatch("R10", "R*"));
    assert!(fnmatch("R1", "R?"));
    assert!(!fnmatch("R10", "R?"));
    assert!(fnmatch("C3", "[CR]*"));
    assert!(!fnmatch("U3", "[CR]*"));
    assert!(fnmatch("U3", "[!CR]*"));
    assert!(fnmatch("R5", "R[0-9]"));
    assert!(!fnmatch("r1", "R*"));
    assert!(fnmatch("[x", "[x"));
}

// ------------------------------------------------------------- extraction

#[test]
fn extract_from_schematic_maps_fields() {
    let text = r##"(kicad_sch (version 20231120) (generator "t") (uuid "u")
      (symbol (lib_id "Device:R") (at 0 0 0) (uuid "a") (dnp yes)
        (property "Reference" "R1") (property "Value" "10k") (property "Footprint" "R_0402")
        (property "Datasheet" "ds") (property "MPN" "RC0402") (property "LCSC Part" "C25744")
        (property "Mfr" "Yageo") (property "Desc" "Res") (property "Tolerance" "1%"))
      (symbol (lib_id "power:GND") (at 0 0 0) (uuid "b") (property "Reference" "#PWR01") (property "Value" "GND")))"##;
    let sch = Schematic::new(kct::parse(text).unwrap(), None).unwrap();
    let items = extract_bom_from_schematic(&sch);
    assert_eq!(items.len(), 1);
    let i = &items[0];
    assert_eq!(
        (i.reference.as_str(), i.value.as_str(), i.footprint.as_str()),
        ("R1", "10k", "R_0402")
    );
    assert_eq!(i.datasheet, "ds");
    assert!(i.dnp);
    assert_eq!(i.mpn, "RC0402");
    assert_eq!(i.lcsc, "C25744");
    assert_eq!(i.manufacturer, "Yageo");
    assert_eq!(i.description, "Res");
    assert_eq!(i.properties["Tolerance"], "1%");
    assert!(i.properties.contains_key("Reference"));
}

#[test]
fn extract_bom_flat_and_hierarchical() {
    let path = fixtures().join("simple_rc.kicad_sch");
    let bom = extract_bom(&path.to_string_lossy(), false).unwrap();
    let refs: Vec<&str> = bom.items.iter().map(|i| i.reference.as_str()).collect();
    assert_eq!(refs, ["R1", "C1"]);

    let flat = extract_bom(
        &fixtures()
            .join("hierarchical/root.kicad_sch")
            .to_string_lossy(),
        false,
    )
    .unwrap();
    let deep = extract_bom(
        &fixtures()
            .join("hierarchical/root.kicad_sch")
            .to_string_lossy(),
        true,
    )
    .unwrap();
    assert!(deep.items.len() > flat.items.len());
    assert!(extract_bom("/nonexistent.kicad_sch", false).is_err());
}

const PCB: &str = r#"(kicad_pcb (version 20240108) (generator "test")
  (footprint "Resistor_SMD:R_0402_1005Metric" (layer "F.Cu") (uuid "f1") (at 100 100)
    (property "Reference" "R1" (at 0 -1.5 0) (layer "F.SilkS"))
    (property "Value" "10k" (at 0 1.5 0) (layer "F.Fab"))
    (property "LCSC" "C25744" (at 0 0 0) (layer "F.Fab")))
  (footprint "Capacitor_SMD:C_0402_1005Metric" (layer "F.Cu") (uuid "f2") (at 110 100)
    (attr smd dnp)
    (property "Reference" "C1" (at 0 -1.5 0) (layer "F.SilkS"))
    (property "Value" "100nF" (at 0 1.5 0) (layer "F.Fab")))
  (footprint "TestPoint:TP" (layer "F.Cu") (uuid "f3") (at 120 100)
    (attr exclude_from_bom)
    (property "Reference" "TP1" (at 0 -1.5 0) (layer "F.SilkS"))
    (property "Value" "TP" (at 0 1.5 0) (layer "F.Fab")))
)"#;

#[test]
fn pcb_bom_and_backfill() {
    let dir = tempfile::tempdir().unwrap();
    let pcb = dir.path().join("board.kicad_pcb");
    std::fs::write(&pcb, PCB).unwrap();

    let bom = extract_bom_from_pcb(&pcb.to_string_lossy()).unwrap();
    let refs: Vec<&str> = bom.items.iter().map(|i| i.reference.as_str()).collect();
    assert_eq!(refs, ["R1", "C1"]);
    assert_eq!(bom.items[0].footprint, "Resistor_SMD:R_0402_1005Metric");
    assert_eq!(bom.items[0].lcsc, "C25744");
    assert!(bom.items[1].dnp);

    let mut items = vec![
        r("R1", "10k", ""),
        r("C1", "100nF", "   "),
        r("R9", "1k", ""),
        r("C2", "1u", "Keep:This"),
    ];
    let mut kept = r("C1", "100nF", "Schematic:Fp");
    kept.reference = "C1".into();
    assert_eq!(backfill_footprints_from_pcb(&mut items, &pcb).unwrap(), 2);
    assert_eq!(items[0].footprint, "Resistor_SMD:R_0402_1005Metric");
    assert_eq!(items[1].footprint, "Capacitor_SMD:C_0402_1005Metric");
    assert_eq!(items[2].footprint, "");
    assert_eq!(items[3].footprint, "Keep:This");
    let mut only = vec![kept];
    assert_eq!(backfill_footprints_from_pcb(&mut only, &pcb).unwrap(), 0);
    assert_eq!(only[0].footprint, "Schematic:Fp");
}

// ---------------------------------------------------------- field geometry

fn box_symbol() -> LibrarySymbol {
    let mut s = LibrarySymbol::new("Box");
    s.add_rectangle((-2.54, 5.08), (2.54, -5.08), 0.0, "default", "none")
        .unwrap();
    s.pins.push(LibraryPin::new(
        "1",
        "A",
        "passive",
        (0.0, 7.62),
        270.0,
        2.54,
    ));
    s
}

#[test]
fn placed_bbox_transforms() {
    let s = box_symbol();
    let close = |got: (f64, f64, f64, f64), want: (f64, f64, f64, f64)| {
        for (a, b) in [
            (got.0, want.0),
            (got.1, want.1),
            (got.2, want.2),
            (got.3, want.3),
        ] {
            assert!((a - b).abs() < 1e-9, "{got:?} vs {want:?}");
        }
    };
    close(
        placed_body_bbox(&s, (100.0, 100.0), 0.0, "", None),
        (97.46, 94.92, 102.54, 105.08),
    );
    close(
        placed_body_bbox(&s, (100.0, 100.0), 90.0, "", None),
        (94.92, 97.46, 105.08, 102.54),
    );
    // No graphics: falls back to pin extents; nothing at all: degenerate.
    let mut pins_only = LibrarySymbol::new("P");
    pins_only
        .pins
        .push(LibraryPin::new("1", "A", "passive", (0.0, 2.54), 0.0, 2.54));
    close(
        placed_body_bbox(&pins_only, (10.0, 10.0), 0.0, "", None),
        (10.0, 7.46, 10.0, 7.46),
    );
    close(
        placed_body_bbox(&pins_only, (10.0, 10.0), 0.0, "y", None),
        (10.0, 12.54, 10.0, 12.54),
    );
    assert_eq!(
        placed_body_bbox(&LibrarySymbol::new("E"), (1.0, 2.0), 0.0, "", None),
        (1.0, 2.0, 1.0, 2.0)
    );

    // Multi-unit: the unit's pins define the extents.
    let mut mu = box_symbol();
    mu.units = 2;
    let mut p2 = LibraryPin::new("2", "B", "passive", (10.0, 0.0), 0.0, 2.54);
    p2.unit = 2;
    mu.pins.push(p2);
    assert_eq!(
        placed_body_bbox(&mu, (0.0, 0.0), 0.0, "", Some(2)),
        (10.0, 0.0, 10.0, 0.0)
    );
}

#[test]
fn field_offsets_and_defaults() {
    let bbox = (97.46, 94.92, 102.54, 105.08);
    assert_eq!(field_offset_mm((100.0, 100.0), bbox), 0.0);
    assert_eq!(field_offset_mm((102.54, 105.08), bbox), 0.0);
    assert!((field_offset_mm((100.0, 90.92), bbox) - 4.0).abs() < 1e-9);
    assert!((field_offset_mm((105.54, 109.08), bbox) - 5.0).abs() < 1e-9);

    let pos = default_field_positions(bbox, DEFAULT_FIELD_CLEARANCE_MM);
    assert_eq!(pos["Reference"], (100.33, 93.98, 0.0));
    assert_eq!(pos["Value"], (100.33, 106.68, 0.0));
    assert_eq!(pos.keys().collect::<Vec<_>>(), ["Reference", "Value"]);
}

// ------------------------------------------------------- physical identity

#[test]
fn footprint_keys_disambiguate() {
    let keys = footprint_keys(&[
        ("R1", "a"),
        ("DUP", "b"),
        ("DUP", "c"),
        ("", "d"),
        ("X", "e"),
    ]);
    assert_eq!(
        keys,
        [
            "R1",
            "@kct-footprint:uuid:b",
            "@kct-footprint:uuid:c",
            "",
            "X"
        ]
    );
    let keys = footprint_keys(&[("", "u"), ("", "u"), ("", "")]);
    assert_eq!(
        keys,
        [
            "@kct-footprint:index:0",
            "@kct-footprint:index:1",
            "@kct-footprint:index:2"
        ]
    );
    // A real reference that collides with a generated key gets extra '@'.
    let keys = footprint_keys(&[("@kct-footprint:index:1", "x"), ("D", ""), ("D", "")]);
    assert_eq!(
        keys,
        [
            "@kct-footprint:index:1",
            "@@kct-footprint:index:1",
            "@kct-footprint:index:2"
        ]
    );
}

#[test]
fn component_keys_rules() {
    let c = |r: &str, id: Option<&str>| ComponentRef {
        reference: r.into(),
        component_id: id.map(String::from),
    };
    let keys = component_keys(&[
        c("R1", None),
        c("U", None),
        c("U", None),
        c("Q", Some("q-id")),
    ])
    .unwrap();
    assert_eq!(
        keys,
        [
            "R1",
            "@kct-footprint:index:1",
            "@kct-footprint:index:2",
            "q-id"
        ]
    );
    let err = component_keys(&[c("A", Some("same")), c("B", Some("same"))]).unwrap_err();
    assert!(err
        .to_string()
        .contains("Duplicate physical component identity"));
}

#[test]
fn netlist_selectors_and_pad_positions() {
    let fps = [("U1", vec!["1", "2"]), ("U1", vec!["1"]), ("R1", vec!["1"])];
    assert!(validate_netlist_selectors(&fps, ["R1.1", "U1.2"]).is_ok());
    let err = validate_netlist_selectors(&fps, ["U1.1", "R1.1"]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Ambiguous netlist terminal selectors: U1.1"
    );

    // pcbnew-verified: footprint (100,100), pad local (2,0).
    for (rot, want) in [
        (0.0, (102.0, 100.0)),
        (90.0, (100.0, 98.0)),
        (180.0, (98.0, 100.0)),
        (270.0, (100.0, 102.0)),
    ] {
        let got = physical_pad_position((100.0, 100.0), rot, (2.0, 0.0));
        assert!(
            (got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9,
            "{rot}: {got:?}"
        );
    }
}
