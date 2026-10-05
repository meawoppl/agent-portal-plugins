//! Port of upstream hierarchy tests: `tests/test_schema.py` (hierarchy
//! parts), `tests/test_hierarchy_validation.py`, plus hierarchy building
//! and instance-path resolution against the copied fixtures.

use std::path::{Path, PathBuf};

use kct::parse;
use kct::schema::hierarchy::{
    build_hierarchy, print_hierarchy_tree, HierarchicalLabelInfo, HierarchyNode, SheetInstance,
    SheetPin,
};
use kct::schema::hierarchy_validation::{
    apply_fix, directions_compatible, find_similar_name, format_validation_report, sequence_ratio,
    validate_hierarchy, FixSuggestion, FixType, ValidationIssue, ValidationIssueType,
    ValidationResult,
};
use kct::schema::instances::{build_instance_path, find_project_name};
use kct::SExp;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

// ------------------------------------------------------- SheetPin / Sheet

#[test]
fn sheet_pin_from_sexp() {
    let pin = SheetPin::from_sexp(
        &parse(r#"(pin "CLK" input (at 100 50 0) (uuid "pin-uuid"))"#).unwrap(),
    );
    assert_eq!(pin.name, "CLK");
    assert_eq!(pin.direction, "input");
    assert_eq!(pin.position, (100.0, 50.0));
    assert_eq!(pin.uuid, "pin-uuid");
}

#[test]
fn sheet_instance_from_sexp() {
    let sheet = SheetInstance::from_sexp(
        &parse(
            r#"(sheet (at 100 50) (size 76.2 50.8) (uuid "sheet-uuid")
                (property "Sheetname" "Power") (property "Sheetfile" "power.kicad_sch")
                (pin "VIN" input (at 100 60 0) (uuid "pin1"))
                (pin "VOUT" output (at 176.2 60 180) (uuid "pin2")))"#,
        )
        .unwrap(),
    );
    assert_eq!(sheet.name, "Power");
    assert_eq!(sheet.filename, "power.kicad_sch");
    assert_eq!(sheet.pins.len(), 2);
    assert_eq!(sheet.size, (76.2, 50.8));
    let empty = SheetInstance::from_sexp(&parse("(sheet)").unwrap());
    assert_eq!(empty.size, (50.8, 25.4));
}

fn pin(name: &str, dir: &str, uuid: &str) -> SheetPin {
    SheetPin::new(name, dir, (0.0, 0.0), 0.0, uuid)
}

#[test]
fn sheet_instance_pin_filters() {
    let sheet = SheetInstance {
        name: "Test".into(),
        filename: "test.kicad_sch".into(),
        uuid: "uuid".into(),
        position: (0.0, 0.0),
        size: (50.0, 25.0),
        pins: vec![
            pin("IN1", "input", "1"),
            pin("OUT1", "output", "2"),
            pin("IN2", "input", "3"),
        ],
    };
    assert_eq!(sheet.input_pins().len(), 2);
    assert_eq!(sheet.output_pins().len(), 1);
    assert!(sheet.to_string().contains("pins=3"));
}

// ---------------------------------------------------------- HierarchyNode

#[test]
fn hierarchy_node_depth_and_flags() {
    let root = HierarchyNode::new("Root", "/root.kicad_sch", "1");
    let child = HierarchyNode::new("Child", "/child.kicad_sch", "2").with_parent(&root);
    let grandchild = HierarchyNode::new("Grandchild", "/gc.kicad_sch", "3").with_parent(&child);
    assert_eq!(root.depth(), 0);
    assert_eq!(child.depth(), 1);
    assert_eq!(grandchild.depth(), 2);
    assert!(root.is_root() && !child.is_root());
    assert_eq!(grandchild.parent().unwrap().name, "Child");
    assert_eq!(grandchild.uuid_path(), ["1", "2", "3"]);

    let mut root = root;
    root.children.push(child.clone());
    assert!(!root.is_leaf());
    assert!(child.is_leaf());
    assert!(HierarchyNode::new("T", "/t", "u")
        .hierarchical_label_info
        .is_empty());
}

#[test]
fn hierarchy_node_paths() {
    let root = HierarchyNode::new("Root", "/root.kicad_sch", "1");
    assert_eq!(root.get_path_string(), "/");
    let child = HierarchyNode::new("Power", "/power.kicad_sch", "2").with_parent(&root);
    let grandchild = HierarchyNode::new("Regulator", "/reg.kicad_sch", "3").with_parent(&child);
    assert_eq!(child.get_path_string(), "/Power");
    assert_eq!(grandchild.get_path_string(), "/Power/Regulator");

    let mut child = child;
    child.children.push(grandchild.clone());
    let mut root = root;
    root.children.push(child.clone());
    assert_eq!(root.find_by_path("/Power"), Some(&child));
    assert_eq!(root.find_by_path("/Power/Regulator"), Some(&grandchild));
    assert!(root.find_by_path("/NonExistent").is_none());
}

#[test]
fn hierarchy_node_find_and_all_nodes() {
    let mut root = HierarchyNode::new("Root", "/root.kicad_sch", "1");
    let child1 = HierarchyNode::new("Power", "/power.kicad_sch", "2").with_parent(&root);
    let mut child2 = HierarchyNode::new("Audio", "/audio.kicad_sch", "3").with_parent(&root);
    let grandchild = HierarchyNode::new("Amp", "/amp.kicad_sch", "4").with_parent(&child2);
    child2.children = vec![grandchild.clone()];
    root.children = vec![child1.clone(), child2.clone()];
    assert_eq!(root.find_by_name("Power"), Some(&child1));
    assert_eq!(root.find_by_name("Amp"), Some(&grandchild));
    assert!(root.find_by_name("NonExistent").is_none());
    let all = root.all_nodes();
    assert_eq!(all.len(), 4);
    let names: Vec<&str> = all.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["Root", "Power", "Audio", "Amp"]);
}

// -------------------------------------------------------- build_hierarchy

#[test]
fn build_hierarchy_project_fixture() {
    let root = build_hierarchy(fixtures().join("projects/hierarchical_main.kicad_sch"));
    assert!(root.is_root());
    assert_eq!(root.name, "Root");
    assert_eq!(root.uuid, "00000000-0000-0000-0000-000000000001");
    let names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Logic", "Output"]);
    let logic = root.find_by_path("/Logic").unwrap();
    assert_eq!(logic.hierarchical_labels, ["VCC", "GND", "OUT"]);
    assert_eq!(logic.hierarchical_label_info[2].shape, "output");
    assert_eq!(logic.uuid, "00000000-0000-0000-0000-000000000002");
    assert_eq!(logic.get_path_string(), "/Logic");
    assert_eq!(logic.depth(), 1);
    assert_eq!(root.sheets[0].pins.len(), 3);

    let tree = print_hierarchy_tree(&root, "");
    assert!(tree.contains("hierarchical_main.kicad_sch (root)"));
    assert!(tree.contains("Logic") && tree.contains("Output"));
}

#[test]
fn build_hierarchy_nested_shared_and_missing() {
    let root = build_hierarchy(fixtures().join("hierarchical/root.kicad_sch"));
    let names: Vec<String> = root
        .all_nodes()
        .iter()
        .map(|n| n.get_path_string())
        .collect();
    assert_eq!(
        names,
        ["/", "/SubSheetA", "/SubSheetA/Nested", "/SubSheetB"]
    );
    assert_eq!(root.find_by_name("Nested").unwrap().depth(), 2);

    // The same file twice: second instance is a leaf reference; a missing
    // file yields an empty node.
    let shared = build_hierarchy(fixtures().join("hierarchical/root_shared.kicad_sch"));
    let names: Vec<&str> = shared.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["SubB_1", "SubB_2", "Missing", "Empty"]);
    assert_eq!(shared.children[0].uuid, shared.children[1].uuid);
    assert_eq!(shared.children[2].uuid, "");
    assert!(shared.children[2].is_leaf());

    let missing = build_hierarchy("/nonexistent/none.kicad_sch");
    assert_eq!(missing.uuid, "");
    assert!(missing.children.is_empty());
}

// ------------------------------------------------------------- validation

#[test]
fn directions() {
    for d in ["input", "output", "bidirectional", "passive"] {
        assert!(directions_compatible(d, d));
    }
    for other in ["input", "output"] {
        assert!(directions_compatible("bidirectional", other));
        assert!(directions_compatible(other, "bidirectional"));
        assert!(directions_compatible("passive", other));
        assert!(directions_compatible(other, "passive"));
    }
    assert!(!directions_compatible("input", "output"));
    assert!(!directions_compatible("output", "input"));
    assert!(directions_compatible("INPUT", "input"));
    assert!(directions_compatible("Output", "OUTPUT"));
}

#[test]
fn similar_names() {
    assert_eq!(
        find_similar_name("VCC", &["VCC", "GND", "DATA"], 0.8),
        Some("VCC")
    );
    assert_eq!(
        find_similar_name("vcc", &["VCC", "GND", "DATA"], 0.8),
        Some("VCC")
    );
    assert_eq!(
        find_similar_name("VCC_3V3", &["VCC_3V3A", "GND", "DATA"], 0.8),
        Some("VCC_3V3A")
    );
    assert_eq!(find_similar_name("XYZ", &["ABC", "DEF", "GHI"], 0.8), None);
    assert_eq!(find_similar_name("VCC", &[], 0.8), None);
    // difflib parity spot checks.
    assert!((sequence_ratio("vcc", "vdd") - 1.0 / 3.0).abs() < 1e-12);
    assert!((sequence_ratio("abcd", "bcde") - 0.75).abs() < 1e-12);
    assert!((sequence_ratio("sda_in", "sda_out") - 2.0 * 4.0 / 13.0).abs() < 1e-12);
    assert_eq!(sequence_ratio("", ""), 1.0);
}

fn issue(kind: ValidationIssueType) -> ValidationIssue {
    ValidationIssue {
        issue_type: kind,
        sheet_name: "Power".into(),
        sheet_file: "power.kicad_sch".into(),
        parent_sheet_name: "Root".into(),
        parent_sheet_file: "project.kicad_sch".into(),
        pin_name: Some("VCC".into()),
        label_name: None,
        pin: None,
        label: None,
        message: "Test message".into(),
        suggestions: vec![],
        possible_causes: vec![],
    }
}

#[test]
fn severities_and_counts() {
    assert_eq!(issue(ValidationIssueType::MissingLabel).severity(), "error");
    assert_eq!(
        issue(ValidationIssueType::DirectionMismatch).severity(),
        "warning"
    );
    assert_eq!(issue(ValidationIssueType::MissingPin).severity(), "warning");

    let mut result = ValidationResult::new("test.kicad_sch");
    result
        .issues
        .push(issue(ValidationIssueType::DirectionMismatch));
    assert!(!result.has_errors());
    result.issues.push(issue(ValidationIssueType::MissingLabel));
    assert!(result.has_errors());

    let mut result = ValidationResult::new("test.kicad_sch");
    for _ in 0..2 {
        result.issues.push(issue(ValidationIssueType::MissingLabel));
    }
    for _ in 0..3 {
        result
            .issues
            .push(issue(ValidationIssueType::DirectionMismatch));
    }
    assert_eq!(result.error_count(), 2);
    assert_eq!(result.warning_count(), 3);
    assert_eq!(result.issues_for_sheet("power.kicad_sch").len(), 5);
}

#[test]
fn report_formatting() {
    let result = ValidationResult {
        root_schematic: "test.kicad_sch".into(),
        sheets_checked: 5,
        pins_checked: 10,
        labels_checked: 10,
        ..Default::default()
    };
    let report = format_validation_report(&result, false);
    assert!(report.contains("No issues found"));
    assert!(report.contains("Sheets checked: 5"));

    let mut result = ValidationResult::new("test.kicad_sch");
    let mut i = issue(ValidationIssueType::MissingLabel);
    i.pin = Some(SheetPin::new(
        "VCC",
        "output",
        (10.0, 20.0),
        0.0,
        "pin-uuid",
    ));
    i.message = "Missing label VCC".into();
    i.suggestions = vec![FixSuggestion::new(
        FixType::AddLabel,
        "Add label VCC",
        "power.kicad_sch",
    )];
    i.possible_causes = vec!["Label was deleted".into()];
    result.issues.push(i);
    let report = format_validation_report(&result, false);
    assert!(report.contains("Power") && report.contains("VCC") && report.contains("Missing"));
    assert!(report.contains("[FAIL] Sheet: Power (power.kicad_sch)"));
    assert!(report.contains("Pin: \"VCC\" (output) @ (10.00, 20.00)"));
    assert!(report.contains("Summary: 1 error(s), 0 warning(s) in 1 sheet(s)"));
}

#[test]
fn fix_suggestion_display() {
    let mut s = FixSuggestion::new(
        FixType::FixDirection,
        "Change direction to output",
        "t.kicad_sch",
    );
    s.auto_fixable = true;
    assert!(s.to_string().contains("[Auto-fixable]"));
    let s = FixSuggestion::new(FixType::AddLabel, "Add label VCC", "t.kicad_sch");
    assert!(!s.to_string().contains("[Auto-fixable]"));
}

#[test]
fn hierarchical_label_info_from_sexp() {
    let sexp = SExp::list(
        "hierarchical_label",
        [
            SExp::quoted("DATA_OUT"),
            SExp::list("shape", [SExp::symbol("output")]),
            SExp::list(
                "at",
                [SExp::atom(10.0), SExp::atom(20.0), SExp::atom(180i64)],
            ),
            SExp::list("uuid", [SExp::quoted("label-uuid-123")]),
        ],
    );
    let l = HierarchicalLabelInfo::from_sexp(&sexp);
    assert_eq!(l.name, "DATA_OUT");
    assert_eq!(l.shape, "output");
    assert_eq!(l.position, (10.0, 20.0));
    assert_eq!(l.rotation, 180.0);
    assert_eq!(l.uuid, "label-uuid-123");

    let sexp = SExp::list(
        "hierarchical_label",
        [
            SExp::quoted("DATA_IN"),
            SExp::list("at", [SExp::atom(10.0), SExp::atom(20.0)]),
        ],
    );
    assert_eq!(HierarchicalLabelInfo::from_sexp(&sexp).shape, "input");
}

#[test]
fn validate_simple_schematic() {
    let path = fixtures().join("simple_rc.kicad_sch");
    let path = path.to_string_lossy();
    let result = validate_hierarchy(&path, None);
    assert_eq!(result.root_schematic, path);
    assert!(result.sheets_checked >= 1);
    let result = validate_hierarchy(&path, Some("nonexistent.kicad_sch"));
    assert_eq!(result.sheets_checked, 0);
    // Missing file: empty hierarchy, no issues.
    let result = validate_hierarchy("nonexistent.kicad_sch", None);
    assert!(result.issues.is_empty());
    assert!(format_validation_report(&result, false).contains("No issues found"));
}

#[test]
fn validate_project_fixture() {
    let path = fixtures().join("projects/hierarchical_main.kicad_sch");
    let result = validate_hierarchy(&path.to_string_lossy(), None);
    assert_eq!(result.sheets_checked, 3);
    assert_eq!(result.pins_checked, 5);
    assert_eq!(result.labels_checked, 4);
    // Output sheet: pin LED has no label (error).
    let kinds: Vec<(&str, Option<&str>)> = result
        .issues
        .iter()
        .map(|i| {
            (
                i.issue_type.value(),
                i.pin_name.as_deref().or(i.label_name.as_deref()),
            )
        })
        .collect();
    assert_eq!(kinds, [("missing_label", Some("LED"))]);
    assert_eq!(result.error_count(), 1);
}

const PARENT: &str = r#"(kicad_sch (version 20231120) (generator "t") (uuid "p-uuid")
  (sheet (at 0 0) (size 20 20) (uuid "s-uuid")
    (property "Sheetname" "Child") (property "Sheetfile" "child.kicad_sch")
    (pin "DATA_OUT" output (at 20 5 0) (uuid "pin-a"))
    (pin "SDA" input (at 0 5 180) (uuid "pin-b"))
    (pin "VCC_3V3" input (at 0 10 180) (uuid "pin-c"))))
"#;

const CHILD: &str = r#"(kicad_sch (version 20231120) (generator "t") (uuid "c-uuid")
  (hierarchical_label "DATA_OUT"
    (shape input)
    (at 10 10 0)
    (uuid "lbl-a")
  )
  (hierarchical_label "SDA"
    (shape bidirectional)
    (at 10 20 0)
    (uuid "lbl-b")
  )
  (hierarchical_label "VCC_3V3A"
    (shape input)
    (at 10 30 0)
    (uuid "lbl-c")
  )
)
"#;

#[test]
fn validate_mismatches_and_apply_fix() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("parent.kicad_sch");
    let child = dir.path().join("child.kicad_sch");
    std::fs::write(&parent, PARENT).unwrap();
    std::fs::write(&child, CHILD).unwrap();

    let result = validate_hierarchy(&parent.to_string_lossy(), None);
    let kinds: Vec<&str> = result.issues.iter().map(|i| i.issue_type.value()).collect();
    assert_eq!(
        kinds,
        ["direction_mismatch", "missing_label", "missing_pin"]
    );

    let mismatch = &result.issues[0];
    let fix = &mismatch.suggestions[0];
    assert_eq!(fix.fix_type, FixType::FixDirection);
    assert!(fix.auto_fixable);
    assert_eq!(fix.details["label_uuid"], "lbl-a");
    assert_eq!(fix.details["new_direction"], "output");

    let missing = &result.issues[1];
    assert_eq!(missing.pin_name.as_deref(), Some("VCC_3V3"));
    assert_eq!(missing.suggestions[0].fix_type, FixType::RenameLabel);
    assert_eq!(missing.suggestions[0].details["old_name"], "VCC_3V3A");
    assert_eq!(missing.suggestions[1].fix_type, FixType::AddLabel);
    assert!(missing.possible_causes[0].contains("Possible typo"));

    let orphan = &result.issues[2];
    assert_eq!(orphan.label_name.as_deref(), Some("VCC_3V3A"));
    assert_eq!(orphan.suggestions[0].fix_type, FixType::RenamePin);

    assert!(!apply_fix(&missing.suggestions[1]));
    assert!(apply_fix(fix));
    let text = std::fs::read_to_string(&child).unwrap();
    assert!(text.contains("(shape output)"));
    let after = validate_hierarchy(&parent.to_string_lossy(), None);
    assert!(after
        .issues
        .iter()
        .all(|i| i.issue_type != ValidationIssueType::DirectionMismatch));
    let report = format_validation_report(&result, false);
    assert!(report.contains("[auto-fixable]"));
}

// --------------------------------------------------------------- instances

#[test]
fn instance_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("board.kicad_pro"), "{}").unwrap();
    std::fs::write(root.join("board-backup.kicad_pro"), "{}").unwrap();
    std::fs::write(
        root.join("board.kicad_sch"),
        r#"(kicad_sch (uuid "root-uuid")
            (sheet (at 0 0) (size 10 10) (uuid "sheet-uuid")
              (property "Sheetname" "Power") (property "Sheetfile" "power.kicad_sch")))"#,
    )
    .unwrap();
    std::fs::write(
        root.join("power.kicad_sch"),
        r#"(kicad_sch (uuid "power-uuid"))"#,
    )
    .unwrap();
    std::fs::write(root.join("orphan.kicad_sch"), r#"(kicad_sch (uuid "o"))"#).unwrap();

    // Stem match wins; otherwise the first in sorted order (as upstream).
    assert_eq!(find_project_name(&root.join("board.kicad_sch")), "board");
    assert_eq!(
        find_project_name(&root.join("power.kicad_sch")),
        "board-backup"
    );
    std::fs::remove_file(root.join("board-backup.kicad_pro")).unwrap();
    assert_eq!(find_project_name(&root.join("power.kicad_sch")), "board");
    assert_eq!(
        build_instance_path(&root.join("board.kicad_sch"), "root-uuid"),
        "/root-uuid"
    );
    assert_eq!(
        build_instance_path(&root.join("power.kicad_sch"), "power-uuid"),
        "/root-uuid/power-uuid"
    );
    assert_eq!(
        build_instance_path(&root.join("orphan.kicad_sch"), "o"),
        "/o"
    );

    let lone = tempfile::tempdir().unwrap();
    let p = lone.path().join("solo.kicad_sch");
    std::fs::write(&p, "(kicad_sch)").unwrap();
    assert_eq!(build_instance_path(&p, "x"), "/x");
}
