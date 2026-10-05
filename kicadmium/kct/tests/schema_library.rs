//! Port of upstream library tests: `tests/test_schema.py` (library parts),
//! `tests/test_library_extends.py`, `tests/test_library_multiunit_roundtrip.py`,
//! plus kct's sym-lib-table resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use kct::parse;
use kct::schema::library::{
    expand_kicad_vars, find_project_sym_lib_table, parse_sym_lib_table, resolve_extends,
    LibraryManager, LibraryPin, LibrarySymbol, SymbolArc, SymbolCircle, SymbolGraphic,
    SymbolLibrary, SymbolPolyline, SymbolRectangle, VALID_PIN_TYPES,
};
use kct::schema::schematic::Schematic;
use kct::schema::symbol::OrderedMap;

const MINIMAL_SYMBOL_LIBRARY: &str = r#"(kicad_symbol_lib
  (version "20231120")
  (generator "test")
  (symbol "Device:R"
    (property "Reference" "R" (at 0 0 0) (effects (font (size 1.27 1.27))))
    (property "Value" "R" (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (property "Footprint" "" (at 0 0 0) (effects (hide yes)))
    (property "Datasheet" "" (at 0 0 0) (effects (hide yes)))
    (symbol "Device:R_0_1"
      (pin passive line (at -2.54 0 0) (length 2.54) (name "1") (number "1"))
      (pin passive line (at 2.54 0 180) (length 2.54) (name "2") (number "2"))
    )
  )
  (symbol "Device:C"
    (property "Reference" "C" (at 0 0 0) (effects (font (size 1.27 1.27))))
    (property "Value" "C" (at 0 2.54 0) (effects (font (size 1.27 1.27))))
    (symbol "Device:C_0_1"
      (pin passive line (at -2.54 0 0) (length 2.54) (name "1") (number "1"))
      (pin passive line (at 2.54 0 180) (length 2.54) (name "2") (number "2"))
    )
  )
)
"#;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn tmp_lib() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("test.kicad_sym");
    std::fs::write(&p, MINIMAL_SYMBOL_LIBRARY).unwrap();
    (dir, p)
}

fn pin(number: &str, name: &str, ty: &str, pos: (f64, f64)) -> LibraryPin {
    LibraryPin::new(number, name, ty, pos, 0.0, 2.54)
}

fn sym_with(name: &str, pins: Vec<LibraryPin>) -> LibrarySymbol {
    let mut s = LibrarySymbol::new(name);
    s.pins = pins;
    s
}

fn approx2(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
}

// ------------------------------------------------------------ LibraryPin

#[test]
fn library_pin_from_sexp() {
    let p = LibraryPin::from_sexp(
        &parse(r#"(pin input line (at 5.08 0 180) (length 2.54) (name "IN") (number "1"))"#)
            .unwrap(),
        1,
    );
    assert_eq!(p.number, "1");
    assert_eq!(p.name, "IN");
    assert_eq!(p.pin_type, "input");
    assert_eq!(p.position, (5.08, 0.0));
    assert_eq!(p.rotation, 180.0);
    assert_eq!(p.length, 2.54);
    assert_eq!(p.shape, "line");
    assert_eq!(p.connection_offset(), (0.0, 0.0));
}

#[test]
fn library_pin_to_sexp_node() {
    let mut p = pin("1", "IN", "input", (5.08, 0.0));
    p.rotation = 180.0;
    let node = p.to_sexp_node();
    assert!(node.has_tag("pin"));
    let s = node.to_compact_string();
    for needle in ["input", "line", "5.08", "180", "\"IN\"", "\"1\""] {
        assert!(s.contains(needle), "{needle} in {s}");
    }
}

// --------------------------------------------------------- LibrarySymbol

#[test]
fn library_symbol_from_sexp() {
    let s = LibrarySymbol::from_sexp(
        &parse(
            r#"(symbol "Device:R" (property "Reference" "R") (property "Value" "R")
                (symbol "Device:R_0_1"
                    (pin passive line (at -2.54 0 0) (length 2.54) (name "1") (number "1"))
                    (pin passive line (at 2.54 0 180) (length 2.54) (name "2") (number "2"))))"#,
        )
        .unwrap(),
    );
    assert_eq!(s.name, "Device:R");
    assert!(s.properties.contains_key("Reference"));
    assert_eq!(s.pins.len(), 2);
}

#[test]
fn library_symbol_pin_lookup() {
    let s = sym_with(
        "Test",
        vec![
            pin("1", "GND", "power_in", (0.0, 0.0)),
            pin("2", "VCC", "power_in", (0.0, 0.0)),
            pin("3", "GND", "power_in", (0.0, 0.0)),
        ],
    );
    assert_eq!(s.pin_count(), 3);
    assert_eq!(s.get_pin("2").unwrap().name, "VCC");
    assert!(s.get_pin("99").is_none());
    assert_eq!(s.get_pins_by_name("GND").len(), 2);
}

#[test]
fn pin_position_plain_rotated_mirrored() {
    let s = sym_with("Test", vec![pin("1", "A", "input", (2.54, 0.0))]);
    assert_eq!(
        s.get_pin_position("1", (100.0, 100.0), 0.0, ""),
        Some((102.54, 100.0))
    );
    assert!(approx2(
        s.get_pin_position("1", (100.0, 100.0), 90.0, "").unwrap(),
        (100.0, 97.46)
    ));
    assert_eq!(
        s.get_pin_position("1", (100.0, 100.0), 0.0, "x"),
        Some((97.46, 100.0))
    );
    assert!(s.get_pin_position("9", (0.0, 0.0), 0.0, "").is_none());

    let s = sym_with("Test", vec![pin("1", "A", "input", (0.0, 2.54))]);
    assert_eq!(
        s.get_pin_position("1", (100.0, 100.0), 0.0, "y"),
        Some((100.0, 102.54))
    );
}

#[test]
fn pin_position_rotation_nonzero_y() {
    // Issue #2118: C_Small at (182.88, 97.79) rotated 90.
    let s = sym_with(
        "C_Small",
        vec![
            pin("1", "~", "passive", (0.0, 2.54)),
            pin("2", "~", "passive", (0.0, -2.54)),
        ],
    );
    assert!(approx2(
        s.get_pin_position("1", (182.88, 97.79), 90.0, "").unwrap(),
        (180.34, 97.79)
    ));
    assert!(approx2(
        s.get_pin_position("2", (182.88, 97.79), 90.0, "").unwrap(),
        (185.42, 97.79)
    ));
}

#[test]
fn pin_position_all_rotations() {
    let s = sym_with("C_Small", vec![pin("1", "~", "passive", (0.0, 2.54))]);
    let c = (100.0, 100.0);
    for (rot, want) in [
        (0.0, (100.0, 97.46)),
        (90.0, (97.46, 100.0)),
        (180.0, (100.0, 102.54)),
        (270.0, (102.54, 100.0)),
    ] {
        assert!(
            approx2(s.get_pin_position("1", c, rot, "").unwrap(), want),
            "{rot}"
        );
    }
    // Grid snapping removes trig drift exactly.
    assert_eq!(s.get_pin_position("1", c, 90.0, ""), Some((97.46, 100.0)));
    // Without snapping the raw trig result is kept.
    let raw = s
        .get_pin_position_with("1", (0.0, 0.0), 90.0, "", false)
        .unwrap();
    assert!(raw.1 != 0.0 && raw.1.abs() < 1e-12);
}

#[test]
fn pin_position_mirror_then_rotate() {
    // Mirror applies before rotation (library coordinates).
    let s = sym_with("T", vec![pin("1", "A", "input", (2.54, 1.27))]);
    // mirror x: (-2.54, 1.27); rot 90: (-1.27, -2.54); flip y: (-1.27, 2.54)
    assert_eq!(
        s.get_pin_position("1", (0.0, 0.0), 90.0, "x"),
        Some((-1.27, 2.54))
    );
    // mirror y: (2.54, -1.27); rot 90: (1.27, 2.54); flip y: (1.27, -2.54)
    assert_eq!(
        s.get_pin_position("1", (0.0, 0.0), 90.0, "y"),
        Some((1.27, -2.54))
    );
}

#[test]
fn all_pin_positions() {
    let s = sym_with(
        "Test",
        vec![
            pin("1", "A", "input", (2.54, 0.0)),
            pin("2", "B", "output", (-2.54, 0.0)),
        ],
    );
    let pos = s.get_all_pin_positions((100.0, 100.0), 0.0, "");
    assert_eq!(pos.len(), 2);
    assert_eq!(pos["1"], (102.54, 100.0));
    assert_eq!(pos["2"], (97.46, 100.0));
}

// --------------------------------------------------------- LibraryManager

#[test]
fn library_manager_basics() {
    let mut m = LibraryManager::new();
    m.add_library(
        "Test",
        SymbolLibrary::new("test.kicad_sym", OrderedMap::new()),
    );
    assert!(m.libraries.contains_key("Test"));

    let mut m = LibraryManager::new();
    let syms: OrderedMap<LibrarySymbol> = [("R", LibrarySymbol::new("R"))].into_iter().collect();
    m.add_library("Device", SymbolLibrary::new("Device.kicad_sym", syms));
    assert_eq!(m.get_symbol("R").unwrap().name, "R");
    assert_eq!(m.get_symbol("Device:R").unwrap().name, "R");
    assert!(m.get_symbol("Device:NonExistent").is_none());
    assert!(LibraryManager::new()
        .get_symbol("Device:NonExistent")
        .is_none());
}

fn embedded(text: &str) -> kct::SExp {
    parse(text).unwrap()
}

#[test]
fn load_embedded_single_and_multiple() {
    let ls = embedded(
        r#"(lib_symbols (symbol "Device:R" (property "Reference" "R")
            (symbol "Device:R_1_1"
                (pin passive line (at -2.54 0 0) (length 2.54) (name "1") (number "1"))
                (pin passive line (at 2.54 0 180) (length 2.54) (name "2") (number "2")))))"#,
    );
    let mut m = LibraryManager::new();
    m.load_embedded_sexp(Some(&ls)).unwrap();
    assert_eq!(m.get_symbol("Device:R").unwrap().pins.len(), 2);

    let ls = embedded(
        r#"(lib_symbols
            (symbol "Device:R" (property "Reference" "R")
                (symbol "Device:R_1_1" (pin passive line (at 0 0 0) (length 2.54) (name "1") (number "1"))))
            (symbol "power:GND" (property "Reference" "PWR")
                (symbol "power:GND_1_1" (pin power_in line (at 0 0 0) (length 0) (name "GND") (number "1")))))"#,
    );
    let mut m = LibraryManager::new();
    m.load_embedded_sexp(Some(&ls)).unwrap();
    assert!(m.get_symbol("Device:R").is_some());
    assert!(m.get_symbol("power:GND").is_some());
}

#[test]
fn load_embedded_none_and_no_colon() {
    let mut m = LibraryManager::new();
    m.load_embedded_sexp(None).unwrap();
    assert!(m.libraries.is_empty());

    let ls = embedded(
        r#"(lib_symbols (symbol "MySymbol" (property "Reference" "U")
            (symbol "MySymbol_1_1" (pin input line (at 0 0 0) (length 2.54) (name "IN") (number "1")))))"#,
    );
    m.load_embedded_sexp(Some(&ls)).unwrap();
    assert!(m.libraries.contains_key("MySymbol"));
    assert!(m.get_symbol("MySymbol").is_some());
}

#[test]
fn load_embedded_does_not_overwrite_existing() {
    let mut m = LibraryManager::new();
    let mut existing = LibrarySymbol::new("R");
    existing.add_property("Reference", "R");
    let syms: OrderedMap<LibrarySymbol> = [("R", existing.clone())].into_iter().collect();
    m.add_library("Device", SymbolLibrary::new("Device.kicad_sym", syms));
    let ls = embedded(
        r#"(lib_symbols (symbol "Device:R" (property "Reference" "R")
            (symbol "Device:R_1_1" (pin passive line (at 0 0 0) (length 2.54) (name "1") (number "1")))))"#,
    );
    m.load_embedded_sexp(Some(&ls)).unwrap();
    assert_eq!(m.get_symbol("Device:R"), Some(&existing));
}

#[test]
fn load_embedded_from_schematic() {
    let sch = Schematic::load(fixtures().join("simple_rc.kicad_sch")).unwrap();
    let mut m = LibraryManager::new();
    m.load_embedded(&sch).unwrap();
    for sym in sch.symbols() {
        let pos = m.get_pin_positions(&sym.lib_id, sym.position, sym.rotation, &sym.mirror);
        assert!(pos.len() >= 2, "{}", sym.lib_id);
    }
}

#[test]
fn library_manager_search_path_loading() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Device.kicad_sym"),
        MINIMAL_SYMBOL_LIBRARY.replace("Device:", ""),
    )
    .unwrap();
    let mut m = LibraryManager::new();
    m.add_search_path(&dir.path().to_string_lossy());
    let r = m.get_symbol("Device:R").unwrap();
    assert_eq!(r.pin_count(), 2);
    assert!(m.libraries.contains_key("Device"));
    assert!(m.get_symbol("Device:Missing").is_none());
}

// ---------------------------------------------------------- sym-lib-table

#[test]
fn sym_lib_table_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("proj.kicad_pro"), "{}").unwrap();
    std::fs::create_dir(root.join("libs")).unwrap();
    std::fs::write(
        root.join("libs/mine.kicad_sym"),
        MINIMAL_SYMBOL_LIBRARY.replace("Device:", ""),
    )
    .unwrap();
    std::fs::write(
        root.join("sym-lib-table"),
        r#"(sym_lib_table
  (version 7)
  (lib (name "MyLib")(type "KiCad")(uri "${KIPRJMOD}/libs/mine.kicad_sym")(options "")(descr ""))
  (lib (name "Legacy")(type "Legacy")(uri "${KIPRJMOD}/old.lib")(options "")(descr ""))
  (lib (name "Env")(type "KiCad")(uri "${KCT_TEST_UNSET_VAR_X}/x.kicad_sym")(options "")(descr ""))
)"#,
    )
    .unwrap();
    let sub = root.join("sub");
    std::fs::create_dir(&sub).unwrap();

    let table = find_project_sym_lib_table(&sub).unwrap();
    assert_eq!(table.file_name().unwrap(), "sym-lib-table");
    let entries = parse_sym_lib_table(&table);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].name, "MyLib");
    assert!(entries[0]
        .resolved_path
        .as_ref()
        .unwrap()
        .ends_with("libs/mine.kicad_sym"));
    assert_eq!(entries[1].lib_type, "Legacy");
    assert!(entries[1].resolved_path.is_none());
    assert!(entries[2].resolved_path.is_none());

    let mut m = LibraryManager::new();
    assert_eq!(m.add_sym_lib_table(&table), 1);
    assert_eq!(m.get_symbol("MyLib:C").unwrap().pin_count(), 2);
}

#[test]
fn expand_vars() {
    let env = |k: &str| (k == "KICAD9_SYMBOL_DIR").then(|| "/usr/share/kicad/symbols".to_string());
    assert_eq!(
        expand_kicad_vars("${KICAD9_SYMBOL_DIR}/Device.kicad_sym", None, Some(&env)),
        Some(PathBuf::from("/usr/share/kicad/symbols/Device.kicad_sym"))
    );
    assert_eq!(
        expand_kicad_vars(
            "file://${KIPRJMOD}/a.kicad_sym",
            Some(Path::new("/p")),
            Some(&env)
        ),
        Some(PathBuf::from("/p/a.kicad_sym"))
    );
    assert_eq!(expand_kicad_vars("${KIPRJMOD}/a", None, Some(&env)), None);
    assert_eq!(expand_kicad_vars("${NOPE}/a", None, Some(&env)), None);
}

// ------------------------------------------------------ Symbol creation

#[test]
fn create_symbols_in_library() {
    let mut lib = SymbolLibrary::new("test.kicad_sym", OrderedMap::new());
    {
        let s = lib.create_symbol("TestPart", 1).unwrap();
        assert_eq!(s.name, "TestPart");
        assert_eq!(s.units, 1);
        assert!(s.pins.is_empty() && s.properties.is_empty());
    }
    assert!(lib.symbols.contains_key("TestPart"));
    assert_eq!(lib.create_symbol("QuadOpAmp", 4).unwrap().units, 4);
    let err = lib.create_symbol("TestPart", 1).unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

#[test]
fn add_pins_and_properties() {
    let mut s = LibrarySymbol::new("Test");
    let p = s
        .add_simple_pin("1", "VCC", "power_in", (0.0, 5.08), 270.0)
        .unwrap()
        .clone();
    assert_eq!(s.pins.len(), 1);
    assert_eq!(p, s.pins[0]);
    assert_eq!(s.pins[0].pin_type, "power_in");
    assert_eq!(s.pins[0].rotation, 270.0);
    assert_eq!(s.pins[0].length, 2.54);

    s.add_pin("2", "IN", "input", (-5.08, 0.0), 0.0, 5.08, 1, "line")
        .unwrap();
    assert_eq!(s.pins[1].length, 5.08);

    let err = s
        .add_simple_pin("3", "IN", "invalid_type", (0.0, 0.0), 0.0)
        .unwrap_err();
    assert!(err.to_string().contains("Invalid pin type"));

    let mut all = LibrarySymbol::new("All");
    let mut types: Vec<&str> = VALID_PIN_TYPES.to_vec();
    types.sort();
    for (i, t) in types.iter().enumerate() {
        all.add_simple_pin(
            &(i + 1).to_string(),
            &format!("PIN{}", i + 1),
            t,
            (0.0, i as f64 * 2.54),
            0.0,
        )
        .unwrap();
    }
    assert_eq!(all.pins.len(), VALID_PIN_TYPES.len());

    s.add_property("Reference", "U");
    s.add_property("Value", "OldValue");
    s.set_property("Value", "NewValue");
    assert_eq!(s.properties["Reference"], "U");
    assert_eq!(s.properties["Value"], "NewValue");
}

#[test]
fn multi_unit_pin_assignment() {
    let mut s = LibrarySymbol::new("QuadOpAmp");
    s.units = 4;
    for (unit, base) in [(1, 1), (2, 5)] {
        s.add_pin(
            &base.to_string(),
            "+",
            "input",
            (-5.08, 2.54),
            0.0,
            2.54,
            unit,
            "line",
        )
        .unwrap();
        s.add_pin(
            &(base + 1).to_string(),
            "-",
            "input",
            (-5.08, -2.54),
            0.0,
            2.54,
            unit,
            "line",
        )
        .unwrap();
        s.add_pin(
            &(base + 2).to_string(),
            "OUT",
            "output",
            (5.08, 0.0),
            0.0,
            2.54,
            unit,
            "line",
        )
        .unwrap();
    }
    let u1 = s.get_pins_for_unit(1);
    let u2 = s.get_pins_for_unit(2);
    assert_eq!((u1.len(), u2.len()), (3, 3));
    assert!(u1.iter().all(|p| p.unit == 1) && u2.iter().all(|p| p.unit == 2));
    let text = s.to_sexp_node().to_compact_string();
    assert!(text.contains("QuadOpAmp_1_1") && text.contains("QuadOpAmp_2_1"));
}

// -------------------------------------------------------- serialization

#[test]
fn library_symbol_to_sexp_node() {
    let mut s = LibrarySymbol::new("TestPart");
    s.add_property("Reference", "U");
    s.add_property("Value", "TestPart");
    s.add_simple_pin("1", "IN", "input", (-5.08, 0.0), 0.0)
        .unwrap();
    s.add_simple_pin("2", "OUT", "output", (5.08, 0.0), 180.0)
        .unwrap();
    let node = s.to_sexp_node();
    assert!(node.has_tag("symbol"));
    let text = node.to_compact_string();
    for needle in ["TestPart", "Reference", "Value", "TestPart_1_1"] {
        assert!(text.contains(needle));
    }
    assert!(!text.contains("TestPart_0_1"));
}

#[test]
fn qualified_name_uses_short_unit_names() {
    let mut s = LibrarySymbol::new("Connector_Generic:Conn_01x02");
    s.add_simple_pin("1", "P1", "passive", (0.0, 0.0), 0.0)
        .unwrap();
    s.add_rectangle((-1.0, 1.0), (1.0, -1.0), 0.0, "default", "background")
        .unwrap();
    let text = s.to_sexp_node().to_compact_string();
    assert!(text.contains("\"Conn_01x02_0_1\"") && text.contains("\"Conn_01x02_1_1\""));
    let back = LibrarySymbol::from_sexp(&parse(&text).unwrap());
    assert_eq!(back.pins, s.pins);
    assert_eq!(back.graphics, s.graphics);
}

#[test]
fn symbol_library_to_sexp_node() {
    let mut lib = SymbolLibrary::new("test.kicad_sym", OrderedMap::new());
    let s = lib.create_symbol("TestPart", 1).unwrap();
    s.add_property("Reference", "U");
    s.add_simple_pin("1", "IN", "input", (-5.08, 0.0), 0.0)
        .unwrap();
    let node = lib.to_sexp_node();
    assert!(node.has_tag("kicad_symbol_lib"));
    let text = node.to_compact_string();
    assert!(text.contains("version") && text.contains("generator") && text.contains("TestPart"));
    assert!(text.contains("(generator_version \"10.0\")"));
}

#[test]
fn symbol_library_save_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.kicad_sym");
    let mut lib = SymbolLibrary::new(path.to_string_lossy(), OrderedMap::new());
    let s = lib.create_symbol("TestPart", 1).unwrap();
    s.add_property("Reference", "U");
    s.add_property("Value", "TestPart");
    s.add_property("Footprint", "Package_SO:SOIC-8");
    s.add_simple_pin("1", "IN", "input", (-5.08, 0.0), 0.0)
        .unwrap();
    s.add_simple_pin("2", "OUT", "output", (5.08, 0.0), 180.0)
        .unwrap();
    lib.save(Some(&path.to_string_lossy())).unwrap();
    assert!(path.exists());
    let lib2 = SymbolLibrary::load(&path).unwrap();
    let s2 = &lib2.symbols["TestPart"];
    assert_eq!(s2.name, "TestPart");
    assert_eq!(
        s2.properties.get("Reference").map(String::as_str),
        Some("U")
    );
    assert_eq!(
        s2.properties.get("Value").map(String::as_str),
        Some("TestPart")
    );
    assert_eq!(s2.pins.len(), 2);
}

#[test]
fn save_without_path_errors() {
    let lib = SymbolLibrary::new("", OrderedMap::new());
    assert!(lib
        .save(None)
        .unwrap_err()
        .to_string()
        .contains("No path specified"));
}

#[test]
fn create_library() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("new-symbols.kicad_sym")
        .to_string_lossy()
        .into_owned();
    let lib = SymbolLibrary::create(path.clone(), None);
    assert_eq!(lib.path, path);
    assert!(lib.symbols.is_empty());
    assert_eq!(lib.generator, "kicad_tools");
    assert_eq!(lib.version.len(), 8);
    assert!(lib.version.chars().all(|c| c.is_ascii_digit()));
    assert!(lib.version.as_str() > "20250101");
    assert_eq!(
        SymbolLibrary::create(path, Some("20240101")).version,
        "20240101"
    );
}

#[test]
fn save_new_library_and_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved-symbols.kicad_sym");
    let lib = SymbolLibrary::create(path.to_string_lossy(), Some("20231120"));
    lib.save(None).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("kicad_symbol_lib") && content.contains("20231120"));
    assert!(content.contains("kicad_tools"));
    let loaded = SymbolLibrary::load(&path).unwrap();
    assert_eq!(loaded.version, "20231120");
    assert_eq!(loaded.generator, "kicad_tools");

    let original = dir.path().join("original.kicad_sym");
    let copy = dir.path().join("copy.kicad_sym");
    let lib = SymbolLibrary::create(original.to_string_lossy(), Some("20231120"));
    lib.save(Some(&copy.to_string_lossy())).unwrap();
    assert!(copy.exists() && !original.exists());
}

#[test]
fn round_trip_preserves_content() {
    let (dir, p) = tmp_lib();
    let lib = SymbolLibrary::load(&p).unwrap();
    let out = dir.path().join("output.kicad_sym");
    lib.save(Some(&out.to_string_lossy())).unwrap();
    let reloaded = SymbolLibrary::load(&out).unwrap();
    let keys = |l: &SymbolLibrary| l.symbols.keys().map(String::from).collect::<BTreeSet<_>>();
    assert_eq!(keys(&lib), keys(&reloaded));
    assert_eq!(reloaded.version, lib.version);
    assert_eq!(reloaded.generator, lib.generator);
    let r = reloaded.get_symbol("Device:R").unwrap();
    assert!(r.properties.contains_key("Reference"));
    assert_eq!(r.pins.len(), 2);
    assert!(reloaded.get_symbol("Device:C").is_some());
    // Cached tree round-trips byte-for-byte on a second save.
    let out2 = dir.path().join("output2.kicad_sym");
    reloaded.save(Some(&out2.to_string_lossy())).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&out2).unwrap());
}

#[test]
fn load_valid_and_invalid() {
    let (dir, p) = tmp_lib();
    let lib = SymbolLibrary::load(&p).unwrap();
    assert_eq!(lib.version, "20231120");
    assert_eq!(lib.generator, "test");
    assert_eq!(lib.len(), 2);
    assert!(lib.symbols.contains_key("Device:R") && lib.symbols.contains_key("Device:C"));
    assert!(!lib.symbols.contains_key("Device:R_0_1"));
    assert_eq!(lib.symbols["Device:R"].pin_count(), 2);

    let bad = dir.path().join("invalid.kicad_sym");
    std::fs::write(&bad, "(kicad_sch (version 20231120))").unwrap();
    let err = SymbolLibrary::load(&bad).unwrap_err();
    assert!(err.to_string().contains("Not a KiCad symbol library"));
    assert!(SymbolLibrary::load_from_string("(kicad_sch)").is_err());
    assert_eq!(
        SymbolLibrary::load_from_string(MINIMAL_SYMBOL_LIBRARY)
            .unwrap()
            .path,
        "<string>"
    );
}

// --------------------------------------------------------------- graphics

#[test]
fn polyline_graphics() {
    let mut s = LibrarySymbol::new("Test");
    let g = s
        .add_polyline(
            vec![(0.0, 0.0), (5.08, 0.0), (5.08, 5.08)],
            0.0,
            "default",
            "none",
        )
        .unwrap()
        .clone();
    assert_eq!(s.graphics.len(), 1);
    let SymbolGraphic::Polyline(pl) = g else {
        panic!("polyline")
    };
    assert_eq!(pl.points, vec![(0.0, 0.0), (5.08, 0.0), (5.08, 5.08)]);
    assert_eq!(
        (
            pl.stroke_width,
            pl.stroke_type.as_str(),
            pl.fill_type.as_str()
        ),
        (0.0, "default", "none")
    );

    let pl = SymbolPolyline::new(vec![(0.0, 0.0), (2.54, -1.27)], 0.254, "dash", "none").unwrap();
    let node = pl.to_sexp_node();
    assert!(node.has_tag("polyline"));
    let t = node.to_compact_string();
    for needle in [
        "(pts",
        "(xy 0",
        "(xy 2.54 -1.27)",
        "(stroke",
        "(width 0.254)",
        "(type dash)",
        "(fill",
    ] {
        assert!(t.contains(needle), "{needle} in {t}");
    }

    let orig = SymbolPolyline::new(
        vec![(1.0, 2.0), (3.0, 4.0), (5.0, 6.0)],
        0.5,
        "default",
        "outline",
    )
    .unwrap();
    assert_eq!(SymbolPolyline::from_sexp(&orig.to_sexp_node()), orig);

    for pts in [vec![(0.0, 0.0)], vec![]] {
        let err = SymbolPolyline::new(pts, 0.0, "default", "none").unwrap_err();
        assert!(err.to_string().contains("at least 2 points"));
    }
}

#[test]
fn circle_arc_rectangle_graphics() {
    let mut s = LibrarySymbol::new("Test");
    let SymbolGraphic::Circle(c) = s
        .add_circle((0.0, 0.0), 2.54, 0.0, "default", "none")
        .unwrap()
        .clone()
    else {
        panic!()
    };
    assert_eq!((c.center, c.radius), ((0.0, 0.0), 2.54));
    let t = SymbolCircle::new((1.27, -1.27), 5.08, 0.0, "default", "none")
        .unwrap()
        .to_sexp_node()
        .to_compact_string();
    assert!(t.starts_with("(circle") && t.contains("(center") && t.contains("(radius"));
    let orig = SymbolCircle::new((2.54, 3.81), 5.08, 0.25, "default", "background").unwrap();
    assert_eq!(SymbolCircle::from_sexp(&orig.to_sexp_node()), orig);
    for r in [0.0, -1.0] {
        let err = SymbolCircle::new((0.0, 0.0), r, 0.0, "default", "none").unwrap_err();
        assert!(err.to_string().contains("radius must be positive"));
    }

    let SymbolGraphic::Arc(a) = s
        .add_arc(
            (0.0, 0.0),
            (1.27, 1.27),
            (2.54, 0.0),
            0.0,
            "default",
            "none",
        )
        .unwrap()
        .clone()
    else {
        panic!()
    };
    assert_eq!(
        (a.start, a.mid, a.end),
        ((0.0, 0.0), (1.27, 1.27), (2.54, 0.0))
    );
    let t = a.to_sexp_node().to_compact_string();
    for needle in ["(arc", "(start", "(mid", "(end", "(stroke", "(fill"] {
        assert!(t.contains(needle));
    }
    let orig = SymbolArc::new(
        (0.0, 0.0),
        (1.27, 2.54),
        (2.54, 0.0),
        0.1,
        "default",
        "outline",
    )
    .unwrap();
    assert_eq!(SymbolArc::from_sexp(&orig.to_sexp_node()), orig);

    let SymbolGraphic::Rectangle(r) = s
        .add_rectangle((-5.08, 5.08), (5.08, -5.08), 0.0, "default", "background")
        .unwrap()
        .clone()
    else {
        panic!()
    };
    assert_eq!(
        (r.start, r.end, r.fill_type.as_str()),
        ((-5.08, 5.08), (5.08, -5.08), "background")
    );
    let orig =
        SymbolRectangle::new((-5.08, 5.08), (5.08, -5.08), 0.254, "solid", "background").unwrap();
    assert_eq!(SymbolRectangle::from_sexp(&orig.to_sexp_node()), orig);
    assert_eq!(s.graphics.len(), 3);
}

#[test]
fn polygon_helpers_and_validation() {
    let mut s = LibrarySymbol::new("Test");
    let SymbolGraphic::Polyline(pl) = s
        .add_polygon(
            vec![(0.0, 0.0), (2.54, -1.27), (0.0, -2.54)],
            0.0,
            "default",
            "outline",
        )
        .unwrap()
        .clone()
    else {
        panic!()
    };
    assert_eq!(pl.points.first(), pl.points.last());
    assert_eq!(pl.points.len(), 4);
    assert_eq!(pl.fill_type, "outline");

    let SymbolGraphic::Polyline(pl) = s
        .add_polygon(
            vec![(0.0, 0.0), (2.54, -1.27), (0.0, -2.54), (0.0, 0.0)],
            0.0,
            "default",
            "outline",
        )
        .unwrap()
        .clone()
    else {
        panic!()
    };
    assert_eq!(pl.points.len(), 4);

    let err = s
        .add_polygon(vec![(0.0, 0.0), (1.0, 1.0)], 0.0, "default", "outline")
        .unwrap_err();
    assert!(err.to_string().contains("at least 3 points"));
    let err =
        SymbolPolyline::new(vec![(0.0, 0.0), (1.0, 1.0)], 0.0, "default", "invalid").unwrap_err();
    assert!(err.to_string().contains("Invalid fill_type"));
    let err = SymbolCircle::new((0.0, 0.0), 1.0, 0.0, "zigzag", "none").unwrap_err();
    assert!(err.to_string().contains("Invalid stroke_type"));
}

#[test]
fn graphics_in_0_1_and_round_trip() {
    let mut s = LibrarySymbol::new("TriBody");
    s.add_polygon(
        vec![(0.0, 0.0), (2.54, -1.27), (0.0, -2.54)],
        0.0,
        "default",
        "outline",
    )
    .unwrap();
    s.add_simple_pin("1", "B", "input", (-5.08, -1.27), 0.0)
        .unwrap();
    let t = s.to_sexp_node().to_compact_string();
    assert!(t.contains("TriBody_0_1") && t.contains("(polyline"));
    assert!(t.contains("TriBody_1_1") && t.contains("(pin"));

    let mut s = LibrarySymbol::new("Mixed");
    s.add_polyline(vec![(0.0, 0.0), (5.0, 0.0)], 0.0, "default", "none")
        .unwrap();
    s.add_circle((0.0, 0.0), 2.0, 0.0, "default", "none")
        .unwrap();
    s.add_arc((0.0, 0.0), (1.0, 1.0), (2.0, 0.0), 0.0, "default", "none")
        .unwrap();
    s.add_rectangle((-3.0, 3.0), (3.0, -3.0), 0.0, "default", "none")
        .unwrap();
    let t = s.to_sexp_node().to_compact_string();
    for needle in ["(polyline", "(circle", "(arc", "(rectangle"] {
        assert!(t.contains(needle));
    }

    let mut s = LibrarySymbol::new("RoundTrip");
    s.add_polygon(
        vec![(0.0, 0.0), (2.54, -1.27), (0.0, -2.54)],
        0.0,
        "default",
        "outline",
    )
    .unwrap();
    s.add_circle((0.0, 0.0), 1.27, 0.0, "default", "background")
        .unwrap();
    s.add_rectangle((-5.0, 5.0), (5.0, -5.0), 0.254, "default", "none")
        .unwrap();
    s.add_simple_pin("1", "A", "input", (-5.08, 0.0), 0.0)
        .unwrap();
    let restored = LibrarySymbol::from_sexp(&s.to_sexp_node());
    assert_eq!(restored.pins.len(), 1);
    assert_eq!(restored.graphics.len(), 3);
    let types: BTreeSet<&str> = restored
        .graphics
        .iter()
        .map(SymbolGraphic::type_name)
        .collect();
    assert_eq!(
        types,
        BTreeSet::from(["SymbolPolyline", "SymbolCircle", "SymbolRectangle"])
    );
}

#[test]
fn save_and_load_with_graphics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gfx.kicad_sym");
    let mut lib = SymbolLibrary::new(path.to_string_lossy(), OrderedMap::new());
    let s = lib.create_symbol("GfxPart", 1).unwrap();
    s.add_property("Reference", "U");
    s.add_property("Value", "GfxPart");
    s.add_polygon(
        vec![(0.0, 0.0), (5.08, -2.54), (0.0, -5.08)],
        0.0,
        "default",
        "outline",
    )
    .unwrap();
    s.add_simple_pin("1", "IN", "input", (-5.08, 0.0), 0.0)
        .unwrap();
    lib.save(None).unwrap();
    let lib2 = SymbolLibrary::load(&path).unwrap();
    let s2 = &lib2.symbols["GfxPart"];
    assert_eq!(s2.pins.len(), 1);
    assert_eq!(s2.graphics.len(), 1);
    let SymbolGraphic::Polyline(pl) = &s2.graphics[0] else {
        panic!()
    };
    assert_eq!(pl.fill_type, "outline");
}

// ----------------------------------------------------------------- extends

fn mk_pin(number: &str, name: &str) -> LibraryPin {
    let y = number
        .parse::<i64>()
        .map(|n| n as f64 * 2.54)
        .unwrap_or(0.0);
    LibraryPin::new(number, name, "passive", (0.0, y), 0.0, 2.54)
}

fn polyline() -> SymbolGraphic {
    SymbolGraphic::Polyline(
        SymbolPolyline::new(
            vec![(0.0, 0.0), (5.0, 0.0), (5.0, 5.0), (0.0, 5.0), (0.0, 0.0)],
            0.0,
            "default",
            "none",
        )
        .unwrap(),
    )
}

fn derived(name: &str, base: &str) -> LibrarySymbol {
    let mut s = LibrarySymbol::new(name);
    s.extends = Some(base.into());
    s
}

#[test]
fn extends_basic_and_multi_level() {
    let mut base = sym_with(
        "OpAmp",
        vec![mk_pin("1", "+"), mk_pin("2", "-"), mk_pin("3", "OUT")],
    );
    base.graphics.push(polyline());
    let mut syms: OrderedMap<LibrarySymbol> =
        [("OpAmp", base), ("LM358", derived("LM358", "OpAmp"))]
            .into_iter()
            .collect();
    resolve_extends(&mut syms).unwrap();
    let d = &syms["LM358"];
    let names: Vec<&str> = d.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["+", "-", "OUT"]);
    assert_eq!(d.graphics.len(), 1);

    let mut syms: OrderedMap<LibrarySymbol> = [
        (
            "Base",
            sym_with("Base", vec![mk_pin("1", "P1"), mk_pin("2", "P2")]),
        ),
        ("Mid", derived("Mid", "Base")),
        ("Leaf", derived("Leaf", "Mid")),
    ]
    .into_iter()
    .collect();
    resolve_extends(&mut syms).unwrap();
    assert_eq!(syms["Mid"].pins.len(), 2);
    assert_eq!(syms["Leaf"].pins.len(), 2);
    assert_eq!(syms["Leaf"].pins[0].name, "P1");
}

#[test]
fn extends_edge_cases() {
    let mut syms: OrderedMap<LibrarySymbol> = [("A", derived("A", "B")), ("B", derived("B", "A"))]
        .into_iter()
        .collect();
    let err = resolve_extends(&mut syms).unwrap_err();
    assert!(err.to_string().contains("Circular extends chain"));

    let mut syms: OrderedMap<LibrarySymbol> = [("Derived", derived("Derived", "Missing"))]
        .into_iter()
        .collect();
    resolve_extends(&mut syms).unwrap();
    assert!(syms["Derived"].pins.is_empty());

    let mut own = derived("Derived", "Base");
    own.pins = vec![mk_pin("1", "D1")];
    let mut syms: OrderedMap<LibrarySymbol> = [
        (
            "Base",
            sym_with("Base", vec![mk_pin("1", "B1"), mk_pin("2", "B2")]),
        ),
        ("Derived", own),
    ]
    .into_iter()
    .collect();
    resolve_extends(&mut syms).unwrap();
    assert_eq!(syms["Derived"].pins.len(), 1);
    assert_eq!(syms["Derived"].pins[0].name, "D1");

    let mut syms: OrderedMap<LibrarySymbol> = [
        (
            "Amplifier_Operational:OpAmp",
            sym_with(
                "Amplifier_Operational:OpAmp",
                vec![mk_pin("1", "+"), mk_pin("2", "-")],
            ),
        ),
        ("LM358", derived("LM358", "OpAmp")),
    ]
    .into_iter()
    .collect();
    resolve_extends(&mut syms).unwrap();
    assert_eq!(syms["LM358"].pins.len(), 2);

    let mut syms: OrderedMap<LibrarySymbol> = [(
        "Resistor",
        sym_with("Resistor", vec![mk_pin("1", "1"), mk_pin("2", "2")]),
    )]
    .into_iter()
    .collect();
    resolve_extends(&mut syms).unwrap();
    assert_eq!(syms["Resistor"].pins.len(), 2);
}

#[test]
fn methods_after_resolution() {
    let mut syms: OrderedMap<LibrarySymbol> = [
        (
            "Base",
            sym_with(
                "Base",
                vec![mk_pin("1", "GND"), mk_pin("2", "VCC"), mk_pin("3", "GND")],
            ),
        ),
        ("Derived", derived("Derived", "Base")),
    ]
    .into_iter()
    .collect();
    assert_eq!(syms["Derived"].pin_count(), 0);
    resolve_extends(&mut syms).unwrap();
    let d = &syms["Derived"];
    assert_eq!(d.pin_count(), 3);
    assert_eq!(d.get_pin("1").unwrap().name, "GND");
    assert!(d.get_pin("99").is_none());
    assert_eq!(d.get_pins_by_name("GND").len(), 2);
    assert_eq!(d.get_all_pin_positions((10.0, 20.0), 0.0, "").len(), 3);
}

const OPAMP_PINS: &str = r#"(pin input line (at -5.08 2.54 0) (length 2.54) (name "+" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
              (pin input line (at -5.08 -2.54 0) (length 2.54) (name "-" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))
              (pin output line (at 5.08 0 180) (length 2.54) (name "OUT" (effects (font (size 1.27 1.27)))) (number "3" (effects (font (size 1.27 1.27)))))"#;

#[test]
fn embedded_extends_resolved() {
    let text = format!(
        r#"(lib_symbols
          (symbol "mylib:OpAmp"
            (symbol "OpAmp_0_1"
              (polyline (pts (xy 0 0) (xy 5 0)) (stroke (width 0) (type default)) (fill (type none))))
            (symbol "OpAmp_1_1" {OPAMP_PINS}))
          (symbol "mylib:LM358"
            (extends "OpAmp")
            (property "Reference" "U" (at 0 0 0) (effects (font (size 1.27 1.27))))
            (property "Value" "LM358" (at 0 0 0) (effects (font (size 1.27 1.27))))))"#
    );
    let mut m = LibraryManager::new();
    m.load_embedded_sexp(Some(&parse(&text).unwrap())).unwrap();
    let d = m.get_symbol("mylib:LM358").unwrap();
    assert_eq!(d.extends.as_deref(), Some("OpAmp"));
    assert_eq!(d.pin_count(), 3);
    assert_eq!(d.get_pin("1").unwrap().name, "+");
    assert_eq!(d.graphics.len(), 1);

    let text = r#"(lib_symbols (symbol "mylib:OpAmp" (symbol "OpAmp_1_1"
        (pin input line (at 0 0 0) (length 2.54) (name "IN") (number "1")))))"#;
    let mut m = LibraryManager::new();
    m.load_embedded_sexp(Some(&parse(text).unwrap())).unwrap();
    let s = m.get_symbol("mylib:OpAmp").unwrap();
    assert_eq!(s.pin_count(), 1);
    assert!(s.extends.is_none());
}

#[test]
fn library_load_resolves_extends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.kicad_sym");
    std::fs::write(
        &path,
        format!(
            r#"(kicad_symbol_lib (version 20231120) (generator "kicad_symbol_editor")
              (symbol "OpAmp" (symbol "OpAmp_1_1" {OPAMP_PINS}))
              (symbol "LM358" (extends "OpAmp")
                (property "Reference" "U" (at 0 0 0) (effects (font (size 1.27 1.27))))
                (property "Value" "LM358" (at 0 0 0) (effects (font (size 1.27 1.27))))))"#
        ),
    )
    .unwrap();
    let lib = SymbolLibrary::load(&path).unwrap();
    let d = lib.get_symbol("LM358").unwrap();
    assert_eq!(d.extends.as_deref(), Some("OpAmp"));
    assert_eq!(d.pin_count(), 3);
    assert_eq!(d.get_pin("1").unwrap().name, "+");
    assert_eq!(d.get_pin("3").unwrap().name, "OUT");
    assert_eq!(lib.resolve_base(d).unwrap().name, "OpAmp");
    // Derived symbols serialize without unit sub-symbols.
    let text = d.to_sexp_node().to_compact_string();
    assert!(text.contains("(extends \"OpAmp\")") && !text.contains("LM358_1_1"));
}

#[test]
fn schematic_get_lib_symbol_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.kicad_sch");
    std::fs::write(
        &path,
        format!(
            r#"(kicad_sch (version 20231120) (generator "kicad_tools")
              (lib_symbols
                (symbol "mylib:OpAmp" (symbol "OpAmp_1_1" {OPAMP_PINS}))
                (symbol "mylib:LM358" (extends "OpAmp")
                  (property "Reference" "U" (at 0 0 0) (effects (font (size 1.27 1.27))))
                  (property "Value" "LM358" (at 0 0 0) (effects (font (size 1.27 1.27)))))))"#
        ),
    )
    .unwrap();
    let sch = Schematic::load(&path).unwrap();
    assert_eq!(
        sch.get_lib_symbol_resolved("mylib:OpAmp")
            .unwrap()
            .unwrap()
            .pin_count(),
        3
    );
    let d = sch.get_lib_symbol_resolved("mylib:LM358").unwrap().unwrap();
    assert_eq!(d.pin_count(), 3);
    assert_eq!(d.get_pin("1").unwrap().name, "+");
    assert!(sch
        .get_lib_symbol_resolved("nonexistent:Symbol")
        .unwrap()
        .is_none());
}

// --------------------------------------------------- multi-unit round trip

fn unit_map(s: &LibrarySymbol) -> BTreeMap<String, i64> {
    s.pins.iter().map(|p| (p.number.clone(), p.unit)).collect()
}

fn regenerate(lib: &mut SymbolLibrary, dir: &Path, name: &str) -> (SymbolLibrary, String) {
    lib.sexp = None;
    let out = dir.join(name);
    lib.save(Some(&out.to_string_lossy())).unwrap();
    (
        SymbolLibrary::load(&out).unwrap(),
        std::fs::read_to_string(&out).unwrap(),
    )
}

#[test]
fn multiunit_load_and_regenerate() {
    let fixture = fixtures().join("multiunit_test.kicad_sym");
    let mut lib = SymbolLibrary::load(&fixture).unwrap();
    assert_eq!(lib.symbols.keys().collect::<Vec<_>>(), ["MultiUnitPart"]);
    let s = lib.get_symbol("MultiUnitPart").unwrap();
    assert_eq!(s.units, 3);
    assert_eq!(s.pin_count(), 6);
    let expected: BTreeMap<String, i64> =
        [("1", 1), ("2", 1), ("3", 2), ("4", 2), ("5", 3), ("6", 3)]
            .map(|(k, v)| (k.to_string(), v))
            .into();
    assert_eq!(unit_map(s), expected);

    let dir = tempfile::tempdir().unwrap();
    let (reloaded, text) = regenerate(&mut lib, dir.path(), "multiunit_out.kicad_sym");
    assert_eq!(
        reloaded.symbols.keys().collect::<Vec<_>>(),
        ["MultiUnitPart"]
    );
    let r = reloaded.get_symbol("MultiUnitPart").unwrap();
    assert_eq!(r.units, 3);
    assert_eq!(unit_map(r), expected);
    assert!(!text.contains("MultiUnitPart_0_1"));
    for u in 1..=3 {
        assert!(text.contains(&format!("MultiUnitPart_{u}_1")));
    }
    let nums = |u| {
        r.get_pins_for_unit(u)
            .iter()
            .map(|p| p.number.clone())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(nums(1), BTreeSet::from(["1".to_string(), "2".to_string()]));
    assert_eq!(nums(2), BTreeSet::from(["3".to_string(), "4".to_string()]));
    assert_eq!(nums(3), BTreeSet::from(["5".to_string(), "6".to_string()]));
}

#[test]
fn single_unit_and_graphics_regenerate() {
    let dir = tempfile::tempdir().unwrap();
    let single = r#"(kicad_symbol_lib (version 20231120) (generator "kicad_symbol_editor")
        (symbol "SinglePart"
            (property "Reference" "U" (at 0 0 0) (effects (font (size 1.27 1.27))))
            (property "Value" "SinglePart" (at 0 0 0) (effects (font (size 1.27 1.27))))
            (symbol "SinglePart_1_1"
                (pin input line (at -5.08 2.54 0) (length 2.54) (name "IN" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
                (pin output line (at 5.08 0 180) (length 2.54) (name "OUT" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27))))))))"#;
    let src = dir.path().join("single.kicad_sym");
    std::fs::write(&src, single).unwrap();
    let mut lib = SymbolLibrary::load(&src).unwrap();
    let s = lib.get_symbol("SinglePart").unwrap();
    assert_eq!(s.units, 1);
    assert!(s.pins.iter().all(|p| p.unit == 1));
    let (reloaded, text) = regenerate(&mut lib, dir.path(), "single_out.kicad_sym");
    assert_eq!(reloaded.symbols.keys().collect::<Vec<_>>(), ["SinglePart"]);
    assert_eq!(reloaded.get_symbol("SinglePart").unwrap().units, 1);
    assert_eq!(reloaded.get_symbol("SinglePart").unwrap().pin_count(), 2);
    assert!(!text.contains("SinglePart_0_1"));

    let gfx = r#"(kicad_symbol_lib (version 20231120) (generator "kicad_symbol_editor")
        (symbol "GfxPart"
            (property "Reference" "U" (at 0 0 0) (effects (font (size 1.27 1.27))))
            (symbol "GfxPart_0_1"
                (rectangle (start -5.08 5.08) (end 5.08 -5.08) (stroke (width 0.254) (type default)) (fill (type background))))
            (symbol "GfxPart_1_1"
                (pin input line (at -7.62 0 0) (length 2.54) (name "IN") (number "1")))))"#;
    let src = dir.path().join("gfx.kicad_sym");
    std::fs::write(&src, gfx).unwrap();
    let mut lib = SymbolLibrary::load(&src).unwrap();
    let s = lib.get_symbol("GfxPart").unwrap();
    assert_eq!((s.units, s.graphics.len(), s.pin_count()), (1, 1, 1));
    let (reloaded, text) = regenerate(&mut lib, dir.path(), "gfx_out.kicad_sym");
    assert!(text.contains("GfxPart_0_1") && text.contains("GfxPart_1_1"));
    let r = reloaded.get_symbol("GfxPart").unwrap();
    assert_eq!((r.units, r.pin_count(), r.graphics.len()), (1, 1, 1));
}

#[test]
fn underscore_name_units() {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"(kicad_symbol_lib (version 20231120) (generator "kicad_symbol_editor")
        (symbol "My_Part_X"
            (property "Reference" "U" (at 0 0 0) (effects (font (size 1.27 1.27))))
            (symbol "My_Part_X_1_1" (pin input line (at -5.08 2.54 0) (length 2.54) (name "A") (number "1")))
            (symbol "My_Part_X_2_1" (pin output line (at 5.08 0 180) (length 2.54) (name "B") (number "2")))))"#;
    let src = dir.path().join("underscore.kicad_sym");
    std::fs::write(&src, text).unwrap();
    let mut lib = SymbolLibrary::load(&src).unwrap();
    let want: BTreeMap<String, i64> = [("1".to_string(), 1), ("2".to_string(), 2)].into();
    let s = lib.get_symbol("My_Part_X").unwrap();
    assert_eq!(s.units, 2);
    assert_eq!(unit_map(s), want);
    let (reloaded, _) = regenerate(&mut lib, dir.path(), "underscore_out.kicad_sym");
    assert_eq!(reloaded.symbols.keys().collect::<Vec<_>>(), ["My_Part_X"]);
    let r = reloaded.get_symbol("My_Part_X").unwrap();
    assert_eq!(r.units, 2);
    assert_eq!(unit_map(r), want);
}
