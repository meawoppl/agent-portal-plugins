//! Ports of upstream ERC tests: tests/test_erc.py, test_erc_cross_sheet.py,
//! test_erc_cross_sheet_globals.py, test_erc_reattribute_symbol.py,
//! test_erc_explain.py, test_erc_violation_classification.py,
//! test_suggestions.py (ERC half).

use std::path::{Path, PathBuf};

use kct::cli::erc::{sequence_ratio, ERCExplainer};
use kct::erc::cross_sheet::{
    build_power_driver_inventory, check_cross_sheet_duplicates, extract_label_name,
    extract_power_net_name, filter_cross_sheet_global_labels,
    filter_cross_sheet_global_labels_objs, filter_cross_sheet_power_violations,
    filter_phantom_wire_violations, reattribute_symbol_violations,
    reattribute_wire_dangling_violations,
};
use kct::erc::report::parse_text_report;
use kct::erc::{
    ERCReport, ERCViolation, ERCViolationType, Severity, ERC_BLOCKING_TYPES,
    ERC_NON_BLOCKING_TYPES, ERC_TYPE_DESCRIPTIONS,
};
use kct::feedback::generate_erc_suggestions;
use kct::pyjson::{dumps, loads, Json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn sample() -> ERCReport {
    ERCReport::load(fixtures().join("sample_erc.json")).unwrap()
}

// ---------------------------------------------------------- report parsing

#[test]
fn json_report_counts() {
    let r = sample();
    assert_eq!(r.source_file, "test-schematic.kicad_sch");
    assert_eq!(r.kicad_version, "8.0.0");
    assert_eq!(r.violation_count(), 6);
    assert_eq!(r.error_count(), 4);
    assert_eq!(r.warning_count(), 2);
    assert_eq!(r.exclusion_count(), 1);
}

#[test]
fn json_report_violation_details() {
    let r = sample();
    let pins = r.by_type(ERCViolationType::PIN_NOT_CONNECTED);
    assert_eq!(pins.len(), 1);
    let v = pins[0];
    assert_eq!(v.severity, Severity::Error);
    assert!(v.is_connection_issue());
    assert_eq!((v.pos_x, v.pos_y), (100.0, 50.0));
    assert!(v.items[0].contains("R1"));
    assert_eq!(v.type_description(), "Unconnected pin");
    assert!(!v.suggestions.is_empty());

    let sub = r.by_sheet("/subsheet");
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0].sheet, "/subsheet");
    assert_eq!(r.by_type(ERCViolationType::LABEL_DANGLING).len(), 1);
    assert_eq!(r.by_sheet("/").len(), 5);

    let by_sheet = r.violations_by_sheet();
    assert_eq!(by_sheet.iter().find(|(s, _)| s == "/").unwrap().1.len(), 5);
    assert!(by_sheet.iter().any(|(s, _)| s == "/subsheet"));
    assert_eq!(r.filter_by_type("label").len(), 3);

    let s = r.summary();
    assert_eq!(s.get("total_violations"), Some(&Json::Int(6)));
    assert_eq!(s.get("exclusions"), Some(&Json::Int(1)));
}

#[test]
fn violation_to_json_and_display() {
    let r = sample();
    let v = r.by_type(ERCViolationType::PIN_NOT_CONNECTED)[0];
    let d = v.to_json();
    assert_eq!(d.get("type"), Some(&Json::from("pin_not_connected")));
    assert_eq!(
        dumps(d.get("position").unwrap()),
        r#"{"x": 100.0, "y": 50.0}"#
    );
    assert_eq!(v.location_str(), "/ at (100.0, 50.0)");
    let mut bare = ERCViolation::new(
        ERCViolationType::UNKNOWN,
        "foo_bar",
        Severity::Warning,
        "desc",
    );
    // Missing positions default to Python int 0.
    assert_eq!(
        dumps(bare.to_json().get("position").unwrap()),
        r#"{"x": 0, "y": 0}"#
    );
    assert_eq!(bare.type_description(), "Foo Bar");
    assert_eq!(bare.to_string(), "[foo_bar]: desc");
    bare = bare.with_pos(1.0, 2.0);
    assert_eq!(bare.to_string(), "[foo_bar]: desc at (1.0, 2.0)");
}

#[test]
fn text_report_parsing() {
    let content = "** ERC report for design.kicad_sch **
** Created on 2025-01-15 **

** Found 3 ERC violations **
[pin_not_connected]: Unconnected pin
    @(100.0000 mm, 50.0000 mm): Pin 1 of R1
    severity: warning
    sheet: /Power
[label_dangling]: Label not connected
    @(10.5000 mm, 5.0000 mm): Label 'FOO'

** End of Report **
";
    let r = parse_text_report(content, "x.rpt").unwrap();
    assert_eq!(r.source_file, "design.kicad_sch");
    assert_eq!(r.violations.len(), 2);
    let v = &r.violations[0];
    assert_eq!(v.severity, Severity::Warning);
    assert_eq!(v.sheet, "/Power");
    assert_eq!(v.items, vec!["Pin 1 of R1"]);
    assert_eq!(r.violations[1].severity, Severity::Error);
}

#[test]
fn type_parsing_and_tables() {
    assert_eq!(
        ERCViolationType::from_string("pin_not_connected"),
        ERCViolationType::PIN_NOT_CONNECTED
    );
    assert_eq!(
        ERCViolationType::from_string(" POWER_PIN_NOT_DRIVEN "),
        ERCViolationType::POWER_PIN_NOT_DRIVEN
    );
    assert_eq!(
        ERCViolationType::from_string("nonsense"),
        ERCViolationType::UNKNOWN
    );
    for t in [
        "lib_symbol_mismatch",
        "footprint_link_issues",
        "isolated_pin_label",
        "single_global_label",
        "unconnected_wire_endpoint",
    ] {
        assert_ne!(ERCViolationType::from_string(t), ERCViolationType::UNKNOWN);
    }
    for t in ERCViolationType::ALL {
        assert!(
            ERC_TYPE_DESCRIPTIONS.iter().any(|(k, _)| *k == t.value()),
            "{t}"
        );
    }
    assert!(ERCViolationType::POWER_PIN_NOT_DRIVEN.is_blocking());
    assert!(ERCViolationType::PIN_TO_PIN.is_non_blocking());
    assert!(ERC_BLOCKING_TYPES
        .iter()
        .all(|t| !ERC_NON_BLOCKING_TYPES.contains(t)));
    assert_eq!(Severity::from_string("ERROR"), Severity::Error);
    assert_eq!(Severity::from_string("warning"), Severity::Warning);
    assert_eq!(Severity::from_string("exclusion"), Severity::Exclusion);
}

#[test]
fn erc_suggestions() {
    let mut v = ERCViolation::new(
        ERCViolationType::PIN_NOT_CONNECTED,
        "pin_not_connected",
        Severity::Error,
        "Pin not connected",
    );
    v.items = vec!["Pin 1 (input) of R1".into()];
    let s = generate_erc_suggestions(&v);
    assert_eq!(s[0], "Add wire to connect Pin 1 (input) of R1");
    assert_eq!(s.len(), 5);

    let mut dup = ERCViolation::new(
        ERCViolationType::DUPLICATE_REFERENCE,
        "duplicate_reference",
        Severity::Error,
        "Reference 'R1' is used on multiple sheets: /a; /b",
    );
    dup.suggestions = vec!["Consider renaming the duplicate to R2".into()];
    let s = generate_erc_suggestions(&dup);
    assert_eq!(
        s[0],
        "Check all hierarchical sheets for components sharing this reference"
    );
    assert_eq!(s[1], "Consider renaming the duplicate to R2");

    let mut generic = ERCViolation::new(ERCViolationType::UNKNOWN, "weird", Severity::Error, "x")
        .with_pos(12.34, 5.0);
    generic.sheet = "/Power".into();
    let s = generate_erc_suggestions(&generic);
    assert_eq!(
        s,
        vec![
            "Review 'weird' violation in schematic",
            "Check ERC settings in Schematic Setup > Electrical Rules",
            "Navigate to sheet '/Power' to inspect the issue",
            "Inspect area around (12.3, 5.0)",
        ]
    );
}

// ------------------------------------------------------------- cross-sheet

const ROOT: &str = r#"(kicad_sch
  (version 20231120)
  (generator "test")
  (uuid "root-uuid-001")
  (paper "A4")
  (lib_symbols {lib})
  {symbols}
  {sheets}
)
"#;

fn symbol(reference: &str, value: &str, lib_id: &str, uuid: &str) -> String {
    format!(
        r#"  (symbol (lib_id "{lib_id}") (at 100 100 0) (unit 1) (uuid "{uuid}")
    (property "Reference" "{reference}" (at 100 90 0))
    (property "Value" "{value}" (at 100 110 0))
    (pin "1" (uuid "{uuid}-pin1")))
"#
    )
}

fn sheet(name: &str, file: &str) -> String {
    format!(
        r#"  (sheet (at 130 40) (size 40 30) (uuid "sheet-{name}")
    (property "Sheetname" "{name}" (at 130 39 0))
    (property "Sheetfile" "{file}" (at 130 71 0)))
"#
    )
}

fn sch(lib: &str, symbols: &[String], sheets: &[String], extra: &str) -> String {
    ROOT.replace("{lib}", lib)
        .replace("{symbols}", &(symbols.concat() + extra))
        .replace("{sheets}", &sheets.concat())
}

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, content).unwrap();
    p
}

#[test]
fn duplicate_across_sheets() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "sub.kicad_sch",
        &sch("", &[symbol("R1", "4k7", "Device:R", "s2")], &[], ""),
    );
    let root = write(
        d.path(),
        "root.kicad_sch",
        &sch(
            "",
            &[symbol("R1", "10k", "Device:R", "s1")],
            &[sheet("Sub", "sub.kicad_sch")],
            "",
        ),
    );
    let v = check_cross_sheet_duplicates(&root);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].vtype, ERCViolationType::DUPLICATE_REFERENCE);
    assert_eq!(v[0].sheet, "/");
    assert_eq!(
        v[0].description,
        "Reference 'R1' is used on multiple sheets: / (value=10k); /Sub (value=4k7)"
    );
    assert_eq!(
        v[0].suggestions,
        vec!["Consider renaming the duplicate to R2"]
    );
}

#[test]
fn duplicates_ignored_cases() {
    let d = tempfile::tempdir().unwrap();
    // Multi-unit on one sheet.
    let root = write(
        d.path(),
        "mu.kicad_sch",
        &sch(
            "",
            &[
                symbol("U1", "LM358", "Amp:LM358", "a"),
                symbol("U1", "LM358", "Amp:LM358", "b"),
            ],
            &[],
            "",
        ),
    );
    assert!(check_cross_sheet_duplicates(&root).is_empty());
    // Power symbols across sheets.
    write(
        d.path(),
        "psub.kicad_sch",
        &sch("", &[symbol("#PWR01", "GND", "power:GND", "p2")], &[], ""),
    );
    let root = write(
        d.path(),
        "proot.kicad_sch",
        &sch(
            "",
            &[symbol("#PWR01", "GND", "power:GND", "p1")],
            &[sheet("P", "psub.kicad_sch")],
            "",
        ),
    );
    assert!(check_cross_sheet_duplicates(&root).is_empty());
}

#[test]
fn suggestion_next_available_and_shared_instances() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "shared.kicad_sch",
        &sch("", &[symbol("R2", "1k", "Device:R", "x")], &[], ""),
    );
    let root = write(
        d.path(),
        "root.kicad_sch",
        &sch(
            "",
            &[
                symbol("R1", "1k", "Device:R", "a"),
                symbol("R3", "1k", "Device:R", "b"),
            ],
            &[
                sheet("A", "shared.kicad_sch"),
                sheet("B", "shared.kicad_sch"),
            ],
            "",
        ),
    );
    let v = check_cross_sheet_duplicates(&root);
    assert_eq!(v.len(), 1);
    assert!(v[0].description.contains("/A (value=1k); /B (value=1k)"));
    assert_eq!(
        v[0].suggestions,
        vec!["Consider renaming the duplicate to R4"]
    );
}

const PWR_FLAG_LIB: &str = r#"(symbol "power:PWR_FLAG" (symbol "PWR_FLAG_0_1"
    (pin power_out line (at 0 0 90) (length 0) (name "pwr" (effects)) (number "1" (effects)))))
  (symbol "Reg:LDO" (symbol "LDO_1_1"
    (pin power_out line (at 0 0 0) (length 2.54) (name "VOUT" (effects)) (number "2" (effects)))
    (pin power_in line (at 0 0 0) (length 2.54) (name "VIN" (effects)) (number "1" (effects)))))"#;

#[test]
fn power_driver_inventory_and_filter() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "power_sub.kicad_sch",
        &sch(
            PWR_FLAG_LIB,
            &[
                symbol("#FLG01", "+3V3", "power:PWR_FLAG", "f1"),
                symbol("U1", "LDO", "Reg:LDO", "u1"),
            ],
            &[],
            "",
        ),
    );
    let root = write(
        d.path(),
        "root.kicad_sch",
        &sch("", &[], &[sheet("Power", "power_sub.kicad_sch")], ""),
    );
    let driven = build_power_driver_inventory(&root);
    assert!(driven.contains("+3V3"));
    assert!(driven.contains("VOUT"));
    assert!(!driven.contains("VIN"));

    let violations: Vec<Json> = vec![
        loads(
            r#"{"type": "power_pin_not_driven", "description": "Power input pin not driven",
                 "items": [{"description": "Pin +3V3 (power_in) of U2"}]}"#,
        )
        .unwrap(),
        loads(
            r#"{"type": "power_pin_not_driven", "description": "x",
                 "items": [{"description": "Pin VBAT (power_in) of U3"}]}"#,
        )
        .unwrap(),
        loads(r#"{"type": "pin_not_connected", "description": "y"}"#).unwrap(),
    ];
    let kept = filter_cross_sheet_power_violations(violations, &root);
    assert_eq!(kept.len(), 2);
    assert!(dumps(&kept[0]).contains("VBAT"));
}

#[test]
fn power_net_name_extraction() {
    let items = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        extract_power_net_name("", &items(&["Pin VCC (power_in) of U1"])).as_deref(),
        Some("VCC")
    );
    assert_eq!(
        extract_power_net_name("", &items(&["Pin GND of U1"])).as_deref(),
        Some("GND")
    );
    assert_eq!(
        extract_power_net_name("Pin VDD not driven", &[]).as_deref(),
        Some("VDD")
    );
    assert_eq!(extract_power_net_name("nothing here", &[]), None);
}

#[test]
fn global_label_false_positives() {
    let d = tempfile::tempdir().unwrap();
    let gl = r#"(global_label "AUDIO_L" (shape input) (at 50 50 0) (uuid "gl-1"))"#;
    write(d.path(), "dac.kicad_sch", &sch("", &[], &[], gl));
    write(d.path(), "conn.kicad_sch", &sch("", &[], &[], gl));
    let root = write(
        d.path(),
        "root.kicad_sch",
        &sch(
            "",
            &[],
            &[
                sheet("DAC", "dac.kicad_sch"),
                sheet("Conn", "conn.kicad_sch"),
            ],
            "",
        ),
    );
    let mk = |desc: &str, item: &str, sheet: &str| {
        let mut v = ERCViolation::new(
            ERCViolationType::SINGLE_GLOBAL_LABEL,
            "single_global_label",
            Severity::Warning,
            desc,
        );
        v.sheet = sheet.into();
        if !item.is_empty() {
            v.items = vec![item.into()];
        }
        v
    };
    let vs = vec![
        mk(
            "Label 'AUDIO_L' appears only once in the design",
            "",
            "/DAC",
        ),
        mk(
            "Label connected to only one pin",
            "Global Label 'AUDIO_R'",
            "/DAC",
        ),
        // Unparseable name on a label-free sheet: phantom, suppressed.
        mk("Label connected to only one pin", "", "/"),
        mk("Unparseable", "", ""),
    ];
    let kept = filter_cross_sheet_global_labels_objs(vs, &root);
    assert_eq!(kept.len(), 2);
    assert!(kept[0].items[0].contains("AUDIO_R"));
    assert_eq!(kept[1].description, "Unparseable");

    let dicts = vec![
        loads(r#"{"type": "single_global_label", "description": "Global label 'AUDIO_L' is not connected anywhere else in the schematic", "_sheet_path": "/DAC"}"#).unwrap(),
        loads(r#"{"type": "pin_not_connected", "description": "z"}"#).unwrap(),
    ];
    assert_eq!(filter_cross_sheet_global_labels(dicts, &root).len(), 1);

    assert_eq!(
        extract_label_name("Global label \"X\"", &[]).as_deref(),
        Some("X")
    );
}

#[test]
fn wire_reattribution_and_phantoms() {
    let d = tempfile::tempdir().unwrap();
    let wire = r#"(wire (pts (xy 10 20) (xy 30 20)) (uuid "w1"))"#;
    write(d.path(), "child.kicad_sch", &sch("", &[], &[], wire));
    let root = write(
        d.path(),
        "root.kicad_sch",
        &sch("", &[], &[sheet("Child", "child.kicad_sch")], ""),
    );
    let vs = vec![
        loads(r#"{"type": "wire_dangling", "description": "Wire not connected", "pos": {"x": 20.04, "y": 20.0}, "_sheet_path": "/"}"#).unwrap(),
        loads(r#"{"type": "wire_dangling", "description": "Phantom", "pos": {"x": 55.0, "y": 55.0}, "_sheet_path": "/"}"#).unwrap(),
        loads(r#"{"type": "pin_not_connected", "description": "keep", "_sheet_path": "/"}"#).unwrap(),
    ];
    let vs = reattribute_wire_dangling_violations(vs, &root);
    assert_eq!(vs[0].get("_sheet_path"), Some(&Json::from("/Child")));
    assert_eq!(
        vs[0].get("description"),
        Some(&Json::from("Wire not connected at (20.0, 20.0)"))
    );
    assert_eq!(vs[1].get("_sheet_path"), Some(&Json::from("/")));
    let kept = filter_phantom_wire_violations(vs, &root);
    assert_eq!(kept.len(), 2);
    assert_eq!(kept[1].get("description"), Some(&Json::from("keep")));
}

#[test]
fn symbol_reattribution() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "dac.kicad_sch",
        &sch("", &[symbol("U3", "DAC", "Audio:DAC", "u3-uuid")], &[], ""),
    );
    let root = write(
        d.path(),
        "root.kicad_sch",
        &sch(
            "",
            &[symbol("R1", "1k", "Device:R", "r1-uuid")],
            &[sheet("DAC", "dac.kicad_sch")],
            "",
        ),
    );
    let vs = vec![
        loads(r#"{"type": "pin_not_connected", "items": [{"description": "Pin VCC (power_in) of U3"}], "_sheet_path": "/"}"#).unwrap(),
        loads(r#"{"type": "pin_not_driven", "items": [{"uuid": "u3-uuid"}], "_sheet_path": "/"}"#).unwrap(),
        loads(r#"{"type": "pin_not_connected", "items": [{"description": "Pin 1 of R1"}], "_sheet_path": "/"}"#).unwrap(),
        loads(r#"{"type": "wire_dangling", "items": [{"description": "Symbol U3"}], "_sheet_path": "/"}"#).unwrap(),
    ];
    let vs = reattribute_symbol_violations(vs, &root);
    let sheets: Vec<&str> = vs
        .iter()
        .map(|v| v.get("_sheet_path").and_then(Json::as_str).unwrap())
        .collect();
    assert_eq!(sheets, ["/DAC", "/DAC", "/", "/"]);
}

#[test]
fn hierarchical_fixture_has_no_duplicates() {
    let root = fixtures().join("hierarchical/root.kicad_sch");
    assert!(check_cross_sheet_duplicates(&root).is_empty());
}

// ----------------------------------------------------------------- explain

fn labels() -> Vec<String> {
    [
        "+3.3VA", "+3V3", "VCC", "VCC_3V3", "GND", "CLK", "DATA", "VCC_5V",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[test]
fn sequence_matcher_matches_difflib() {
    let close = |a: f64, b: f64| (a - b).abs() < 1e-12;
    assert!(close(
        sequence_ratio("hello world", "world hello"),
        0.45454545454545453
    ));
    // autojunk kicks in at 200+ items.
    let a = "a".repeat(250) + "b";
    let b = "a".repeat(200) + "bb";
    assert!(close(sequence_ratio(&a, &b), 0.8874172185430463));
}

#[test]
fn similar_labels_fuzzy() {
    let e = ERCExplainer::new(None, labels());
    let s = e.find_similar_labels("VCC_3V3A", 0.6);
    let got: Vec<(&str, f64)> = s.iter().map(|x| (x.name.as_str(), x.similarity)).collect();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, "VCC_3V3");
    assert!((got[0].1 - 0.9333333333333333).abs() < 1e-12);
    assert_eq!(got[1].0, "VCC_5V");
    let s = e.find_similar_labels("+3V3A", 0.6);
    assert_eq!(s[0].name, "+3V3");
    assert!((s[1].similarity - 0.7272727272727273).abs() < 1e-12);
    assert!(!e
        .find_similar_labels("VCC", 0.6)
        .iter()
        .any(|x| x.name == "VCC"));
    assert!(e
        .find_similar_labels("COMPLETELY_DIFFERENT_LABEL_XYZ", 0.6)
        .iter()
        .all(|x| x.similarity <= 0.8));
}

fn hier_violation() -> ERCViolation {
    let mut v = ERCViolation::new(
        ERCViolationType::HIER_LABEL_MISMATCH,
        "hier_label_mismatch",
        Severity::Error,
        "Hierarchical label mismatch between sheet and parent",
    )
    .with_pos(76.20, 106.68);
    v.sheet = "/Power".into();
    v.items = vec!["Hierarchical label 'VCC_3V3A' on subsheet".into()];
    v
}

#[test]
fn explain_hier_label_mismatch() {
    let e = ERCExplainer::new(None, labels());
    let x = e.explain(&hier_violation());
    assert_eq!(
        x.summary,
        "Sheet pin has no matching hierarchical label in sub-schematic"
    );
    assert_eq!(x.diagnosis.len(), 2);
    assert_eq!(x.similar_labels[0].name, "VCC_3V3");
    assert_eq!(
        x.possible_causes[0],
        "Label name mismatch: found 'VCC_3V3' (similar)"
    );
    assert_eq!(
        x.fixes[1].description,
        "Rename label 'VCC_3V3' to 'VCC_3V3A' in sub-schematic"
    );
    let d = x.to_json();
    assert_eq!(
        dumps(d.get("location").unwrap()),
        r#"{"sheet": "/Power", "x": 76.2, "y": 106.68}"#
    );
    assert_eq!(
        dumps(&d.get("similar_labels").unwrap().as_array().unwrap()[0]),
        r#"{"name": "VCC_3V3", "similarity": 0.93, "location": "", "direction": ""}"#
    );
}

#[test]
fn explain_other_types() {
    let e = ERCExplainer::default();
    let mut pin = ERCViolation::new(
        ERCViolationType::PIN_NOT_CONNECTED,
        "pin_not_connected",
        Severity::Error,
        "Pin not connected",
    );
    pin.items = vec!["Pin VCC (power_in) of U1".into()];
    let x = e.explain(&pin);
    assert_eq!(x.summary, "Pin 'VCC' on U1 is not connected to any net");
    assert_eq!(x.diagnosis[1].actual.as_deref(), Some("power_in"));
    assert_eq!(x.related_violations.len(), 1);

    let mut similar = ERCViolation::new(
        ERCViolationType::SIMILAR_LABELS,
        "similar_labels",
        Severity::Warning,
        "Similar labels",
    );
    similar.items = vec!["Label 'CLK'".into(), "Label 'clk'".into()];
    let x = e.explain(&similar);
    assert_eq!(
        x.summary,
        "Labels 'CLK' and 'clk' are similar (possible typo)"
    );
    assert_eq!(x.fixes.len(), 3);

    let mut dup = ERCViolation::new(
        ERCViolationType::DUPLICATE_REFERENCE,
        "duplicate_reference",
        Severity::Error,
        "Duplicate",
    );
    dup.items = vec!["Symbol R1 [10k]".into()];
    let x = e.explain(&dup);
    assert_eq!(
        x.summary,
        "Reference designator 'R1' is used by multiple symbols"
    );
    assert_eq!(
        x.fixes[0].command.as_deref(),
        Some("kct sch annotate <schematic>")
    );

    let mut generic = ERCViolation::new(
        ERCViolationType::UNKNOWN,
        "bus_thing",
        Severity::Warning,
        "Odd",
    );
    generic.suggestions = vec!["a".into(), "b".into()];
    let x = e.explain(&generic);
    assert_eq!(x.summary, "Bus Thing: Odd");
    assert_eq!(x.diagnosis[0].status, "warning");
    assert_eq!(x.fixes.len(), 2);
    assert_eq!(x.fixes[1].priority, 2);
}

// --------------------------------------------------------------------- CLI

fn run(args: &[&str]) -> i32 {
    kct::cli::run(args.iter().map(|s| s.to_string())).unwrap()
}

#[test]
fn cli_erc_exit_codes_and_shim() {
    let json = fixtures().join("sample_erc.json");
    let json = json.to_str().unwrap();
    // Back-compat shim: `kct erc <file>` == `kct erc parse <file>`.
    assert_eq!(run(&["erc", json]), 1);
    assert_eq!(run(&["erc", "parse", json, "--format", "json"]), 1);
    assert_eq!(run(&["erc", json, "--type", "label_dangling"]), 0);
    assert_eq!(
        run(&["erc", json, "--type", "label_dangling", "--strict"]),
        2
    );
    assert_eq!(run(&["erc", "parse", "--list-types"]), 0);
    assert_eq!(run(&["erc", "missing.json"]), 1);
    assert_eq!(run(&["erc", "design.txt"]), 1);
    assert_eq!(run(&["erc", "explain", json]), 1);
    assert_eq!(run(&["erc", "explain", json, "--type", "nomatch"]), 0);
    assert_eq!(run(&["erc", "explain", "missing.json"]), 1);
}
