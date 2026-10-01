//! Ports of upstream DRC report tests: tests/test_drc.py (report side),
//! test_drc_report_formats.py, test_drc_suggestions.py, test_suggestions.py
//! (DRC half), test_violation_filters.py, test_cli_drc_waivers.py,
//! test_drc_summary.py, test_drc_category.py, test_drc_net_compat.py.

use std::path::{Path, PathBuf};

use kct::cli::drc::{create_summary, get_severity, summary_table, IssueSeverity};
use kct::drc::checker::{check_manufacturer_rules, summarize_checks, CheckResult};
use kct::drc::report::{extract_values, infer_segment_via_type, parse_json_report};
use kct::drc::suggestions::{
    calculate_clearance_fix, direction_name, generate_fix_suggestions, FixAction, FixSuggestion,
};
use kct::drc::waivers::{apply_waivers_to_report, waivers_from_json, Waivers};
use kct::drc::{DRCReport, DRCViolation, Location, Severity, ViolationCategory, ViolationType};
use kct::erc::{ERCViolation, ERCViolationType};
use kct::feedback::generate_drc_suggestions;
use kct::pyjson::{dumps, loads, Json};
use kct::validate::filters::{
    load_filters_from_toml, parse_filters_from_config, FilterEngine, FilterLoadError,
    ViolationFilter,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn sample() -> DRCReport {
    DRCReport::load(fixtures().join("sample_drc.rpt")).unwrap()
}

fn approx(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-9)
}

fn viol(vtype: ViolationType, severity: Severity) -> DRCViolation {
    DRCViolation::new(vtype, vtype.value(), severity, "Test")
}

// ----------------------------------------------------------- report parsing

#[test]
fn text_report_header_and_counts() {
    let r = sample();
    assert_eq!(r.pcb_name, "test-board.kicad_pcb");
    assert_eq!(r.violation_count(), 5);
    assert_eq!(r.footprint_errors, 1);
    assert_eq!(r.created_at.as_deref(), Some("2025-01-15T10:30:00-08:00"));
    assert_eq!(r.error_count(), 4);
    assert_eq!(r.warning_count(), 1);
}

#[test]
fn text_report_violation_details() {
    let r = sample();
    let clearance = r.by_type(ViolationType::CLEARANCE);
    assert_eq!(clearance.len(), 2);
    let v = clearance[0];
    assert_eq!(v.severity, Severity::Error);
    assert!(approx(v.required_value_mm, 0.2));
    assert!(approx(v.actual_value_mm, 0.15));
    assert_eq!(v.rule, "netclass 'Default'");
    assert!(v.is_clearance());

    let v0 = &r.violations[0];
    assert_eq!(v0.locations.len(), 2);
    assert_eq!(v0.locations[0].x_mm, 100.0);
    assert_eq!(v0.locations[0].y_mm, 50.0);
    assert_eq!(v0.locations[0].layer, "F.Cu");
    assert_eq!(v0.nets, vec!["VCC", "GND"]);

    let shorts = r.by_type(ViolationType::SHORTING_ITEMS);
    assert_eq!(shorts.len(), 1);
    assert!(shorts[0].is_connection());
    let unconnected = r.by_type(ViolationType::UNCONNECTED_ITEMS);
    assert_eq!(unconnected.len(), 1);
    assert!(unconnected[0].is_connection());
    // Text reports keep the flat shape (#4498).
    assert!(r.unconnected_items.is_empty());
    assert_eq!(r.connectivity_items().len(), 1);
    // Suggestions are attached at parse time.
    assert!(!v0.suggestions.is_empty());
}

#[test]
fn report_queries_and_summary() {
    let r = sample();
    assert!(!r.by_net("VCC").is_empty());
    let grouped = r.violations_by_type();
    let clearance = grouped
        .iter()
        .find(|(t, _)| *t == ViolationType::CLEARANCE)
        .unwrap();
    assert_eq!(clearance.1.len(), 2);
    assert!(!r.violations_near(100.0, 50.0, 2.0).is_empty());
    let s = r.summary();
    assert_eq!(s.get("total_violations"), Some(&Json::Int(5)));
    assert_eq!(s.get("errors"), Some(&Json::Int(4)));
    assert_eq!(s.get("warnings"), Some(&Json::Int(1)));
    assert!(s.get("by_type").unwrap().contains_key("clearance"));
}

#[test]
fn extract_values_wordings() {
    let cases = [
        (
            "Drilled holes too close together (netclass 'Default', min 0.4995 mm; actual 0.4500 mm)",
            0.4995,
            0.45,
        ),
        ("Clearance violation (clearance 0.2000 mm; actual 0.1500 mm)", 0.2, 0.15),
        ("Drill out of range (minimum 0.3000 mm; actual 0.2000 mm)", 0.3, 0.2),
        ("Track width too small (width 0.1500 mm; actual 0.1000 mm)", 0.15, 0.1),
        ("Hard short (clearance 0.2000 mm; actual -0.0500 mm)", 0.2, -0.05),
    ];
    for (msg, req, act) in cases {
        let mut v = viol(ViolationType::UNKNOWN, Severity::Error);
        extract_values(&mut v, msg).unwrap();
        assert!(approx(v.required_value_mm, req), "{msg}");
        assert!(approx(v.actual_value_mm, act), "{msg}");
    }
}

fn json_report(violations: Vec<Json>, unconnected: Vec<Json>) -> DRCReport {
    let data = kct::jobj! {
        "$schema" => "https://schemas.kicad.org/drc.v1.json",
        "source" => "board.kicad_pcb",
        "violations" => Json::Arr(violations),
        "unconnected_items" => Json::Arr(unconnected),
        "schematic_parity" => Json::Arr(vec![]),
    };
    parse_json_report(&dumps(&data), "report.json").unwrap()
}

fn unconnected(i: usize) -> Json {
    loads(&format!(
        r#"{{"type": "unconnected_items", "severity": "error",
        "description": "Missing connection between items",
        "items": [{{"description": "Zone [GND] on F.Cu", "pos": {{"x": {i}.0, "y": 1.0}}}},
                  {{"description": "Pad {i} [GND] of U1 on B.Cu", "pos": {{"x": 2.0, "y": 2.0}}}}]}}"#
    ))
    .unwrap()
}

fn clearance_json() -> Json {
    loads(
        r#"{"type": "clearance", "severity": "error",
        "description": "Clearance violation (clearance 0.2000 mm; actual 0.1000 mm)",
        "items": [{"description": "Track [VCC] on F.Cu", "pos": {"x": 5.0, "y": 5.0}}]}"#,
    )
    .unwrap()
}

#[test]
fn unconnected_items_are_not_geometric_violations() {
    let r = json_report(vec![], (0..12).map(unconnected).collect());
    assert!(r.violations.is_empty());
    assert_eq!(r.error_count(), 0);
    assert_eq!(r.unconnected_item_count(), 12);
    assert_eq!(r.connectivity_items().len(), 12);
    assert!(r
        .unconnected_items
        .iter()
        .all(|v| v.vtype == ViolationType::UNCONNECTED_ITEMS && v.is_connection()));
    assert_eq!(r.unconnected_items[0].nets, vec!["GND"]);

    let r = json_report(vec![clearance_json()], (0..12).map(unconnected).collect());
    assert_eq!(r.violation_count(), 1);
    assert_eq!(r.error_count(), 1);
    assert_eq!(r.unconnected_item_count(), 12);
}

#[test]
fn apply_filters_preserves_connectivity_items() {
    let r = json_report(vec![clearance_json()], (0..3).map(unconnected).collect());
    let filtered = r.apply_filters(&[ViolationFilter::by_type("clearance", "ignore").unwrap()]);
    assert_eq!(filtered.violation_count(), 0);
    assert_eq!(filtered.unconnected_item_count(), 3);
}

#[test]
fn kicad_cli_json_report_with_net_keys() {
    let content = r#"{
        "source": "test.kicad_pcb", "date": "2025-01-15T12:00:00",
        "violations": [{"type": "clearance", "severity": "error",
            "description": "Clearance violation (required 0.2mm, actual 0.15mm)",
            "pos": {"x": 100.0, "y": 50.0},
            "items": [{"description": "Pad 1 of R1", "net": "VCC"}, {"description": "Via", "net": "GND"}]}],
        "footprint_errors": 0}"#;
    let r = parse_json_report(content, "test.json").unwrap();
    assert_eq!(r.violation_count(), 1);
    assert_eq!(r.violations[0].vtype, ViolationType::CLEARANCE);
    assert_eq!(r.violations[0].nets, vec!["VCC", "GND"]);
    assert_eq!(r.created_at.as_deref(), Some("2025-01-15T12:00:00"));
}

#[test]
fn golden_kicad_cli_json_report() {
    let r = DRCReport::load(fixtures().join("drc/kicad_cli_drc_golden.json")).unwrap();
    assert!(r.violation_count() > 0);
    assert!(r
        .violations
        .iter()
        .any(|v| v.vtype == ViolationType::SOLDER_MASK_BRIDGE));
    for v in &r.violations {
        assert!(!v.locations.is_empty());
    }
}

#[test]
fn kct_check_json_report() {
    let content = r#"{
        "file": "/x/board.kicad_pcb", "manufacturer": "jlcpcb", "layers": 2,
        "summary": {"errors": 1, "warnings": 1, "rules_checked": 4, "passed": false},
        "violations": [
            {"rule_id": "clearance_pad_pad", "severity": "warning", "message": "Pad clearance",
             "location": [10, 20.25], "layer": "F.Cu", "actual_value": 0.1, "required_value": 0.2,
             "items": ["Pad 1 of U1", "Pad 2 of U1"], "nets": ["A", "B"]},
            {"rule_id": "dimension_trace_width", "severity": "error", "message": "Trace too thin",
             "items": ["Track"]}]}"#;
    let r = parse_json_report(content, "check.json").unwrap();
    assert_eq!(r.pcb_name, "/x/board.kicad_pcb");
    assert_eq!(r.violation_count(), 2);
    let v = &r.violations[0];
    assert_eq!(v.vtype, ViolationType::CLEARANCE_PAD_PAD);
    assert_eq!(v.rule, "clearance_pad_pad");
    assert!(v.is_same_component_pad_clearance());
    assert_eq!(v.category(), ViolationCategory::Placement);
    // Int coordinates stay ints in JSON output, like Python.
    let loc = v.to_json();
    let loc0 = &loc.get("locations").unwrap().as_array().unwrap()[0];
    assert_eq!(
        dumps(loc0),
        r#"{"x_mm": 10, "y_mm": 20.25, "layer": "F.Cu"}"#
    );
    assert_eq!(r.violations[1].vtype, ViolationType::DIMENSION_TRACE_WIDTH);
    assert!(r.passed() == (r.error_count() == 0));
}

#[test]
fn mask_copper_assessment_schema_is_enforced() {
    let bad =
        r#"{"summary": {}, "violations": [], "mask_copper_assessments": [{"schema": "nope"}]}"#;
    assert!(parse_json_report(bad, "x.json").is_err());
}

#[test]
fn segment_via_inference() {
    let items = vec![
        "Track [A] on F.Cu".to_string(),
        "Via [B] on F.Cu - B.Cu".to_string(),
    ];
    assert_eq!(
        infer_segment_via_type(ViolationType::CLEARANCE, &items),
        ViolationType::CLEARANCE_SEGMENT_VIA
    );
    assert_eq!(
        infer_segment_via_type(ViolationType::CLEARANCE, &items[..1]),
        ViolationType::CLEARANCE
    );
    assert_eq!(
        infer_segment_via_type(ViolationType::TRACK_WIDTH, &items),
        ViolationType::TRACK_WIDTH
    );
}

// ---------------------------------------------------------- violation types

#[test]
fn violation_type_from_string() {
    use ViolationType as T;
    let cases = [
        ("clearance", T::CLEARANCE),
        ("unconnected_items", T::UNCONNECTED_ITEMS),
        ("shorting_items", T::SHORTING_ITEMS),
        ("some_random_type", T::UNKNOWN),
        ("copper_edge_clearance", T::COPPER_EDGE_CLEARANCE),
        ("edge clearance violation", T::COPPER_EDGE_CLEARANCE),
        ("Courtyard overlapping", T::COURTYARD_OVERLAP),
        ("Track width too small", T::TRACK_WIDTH),
        ("Via annular ring too small", T::VIA_ANNULAR_WIDTH),
        ("Via hole larger than pad", T::VIA_HOLE_LARGER_THAN_PAD),
        ("Micro via hole", T::MICRO_VIA_HOLE_TOO_SMALL),
        ("Drill size too small", T::DRILL_HOLE_TOO_SMALL),
        ("Silk over copper pad", T::SILK_OVER_COPPER),
        ("Silkscreen overlap", T::SILK_OVERLAP),
        ("Solder mask bridge", T::SOLDER_MASK_BRIDGE),
        ("Duplicate footprint found", T::DUPLICATE_FOOTPRINT),
        ("Extra footprint on board", T::EXTRA_FOOTPRINT),
        ("Missing footprint", T::MISSING_FOOTPRINT),
        ("footprint", T::FOOTPRINT),
        ("Board outline malformed", T::MALFORMED_OUTLINE),
        // validate rule_ids (test_drc_violation_type_validate.py)
        ("clearance_pad_trace", T::CLEARANCE_PAD_SEGMENT),
        ("clearance_trace_via", T::CLEARANCE_SEGMENT_VIA),
        ("diffpair_clearance_intra", T::DIFFPAIR_CLEARANCE_INTRA),
        ("diffpair_length_skew", T::DIFFPAIR_LENGTH_SKEW),
        ("match_group_length_skew", T::MATCH_GROUP_LENGTH_SKEW),
        ("dimension_drill_clearance", T::HOLE_TO_HOLE_CLEARANCE),
        ("hole_to_hole_clearance", T::HOLE_TO_HOLE_CLEARANCE),
        ("clearance_net0_bridge", T::CLEARANCE_NET0_BRIDGE),
        ("connectivity", T::CONNECTIVITY),
        ("copper_sliver", T::COPPER_SLIVER),
        ("silk_geometry_unmodeled", T::SILK_GEOMETRY_UNMODELED),
        ("via_in_pad", T::VIA_IN_PAD),
        ("drill clearance", T::DRILL_CLEARANCE),
        ("  CLEARANCE  ", T::CLEARANCE),
    ];
    for (s, t) in cases {
        assert_eq!(ViolationType::from_string(s), t, "{s}");
    }
    for t in ViolationType::ALL {
        assert_eq!(ViolationType::from_string(t.value()), *t);
    }
}

#[test]
fn categories() {
    use ViolationCategory as C;
    let cat = |t: ViolationType| viol(t, Severity::Error).category();
    assert_eq!(cat(ViolationType::CLEARANCE), C::Routing);
    assert_eq!(cat(ViolationType::CLEARANCE_PAD_PAD), C::Placement);
    assert_eq!(cat(ViolationType::DRILL_HOLE_TOO_SMALL), C::Manufacturing);
    assert_eq!(cat(ViolationType::UNCONNECTED_ITEMS), C::Connectivity);
    assert_eq!(cat(ViolationType::SILK_OVERLAP), C::Cosmetic);
    assert_eq!(cat(ViolationType::UNKNOWN), C::Routing);

    let mut same = viol(ViolationType::SOLDER_MASK_BRIDGE, Severity::Error);
    same.items = vec!["Pad 1 of U3".into(), "Pad 2 of U3".into()];
    assert_eq!(same.category(), C::Placement);
    let mut diff = same.clone();
    diff.items = vec!["Pad 1 of U3".into(), "Pad 2 of C4".into()];
    assert_eq!(diff.category(), C::Routing);
    let mut via = same.clone();
    via.items = vec!["Via [GND] on F.Cu".into()];
    assert_eq!(via.category(), C::Placement);
}

#[test]
fn fine_pitch_inherent() {
    let mut v = viol(ViolationType::SOLDER_MASK_BRIDGE, Severity::Error);
    v.items = vec!["Pad 1 of U3".into(), "Pad 2 of U3".into()];
    v.actual_value_mm = Some(0.05);
    assert!(v.is_fine_pitch_inherent(0.1));
    v.actual_value_mm = Some(0.12);
    assert!(!v.is_fine_pitch_inherent(0.1));
    v.actual_value_mm = None;
    assert!(!v.is_fine_pitch_inherent(0.1));
}

#[test]
fn location_parsing_and_display() {
    let l = Location::from_string("@(162.4500 mm, 100.3250 mm)").unwrap();
    assert_eq!((l.x_mm, l.y_mm), (162.45, 100.325));
    let l = Location::from_string("@( 100.0 mm , 50.0 mm )").unwrap();
    assert_eq!((l.x_mm, l.y_mm), (100.0, 50.0));
    let l = Location::from_string(r#"{"x": 123.45, "y": 67.89}"#).unwrap();
    assert_eq!((l.x_mm, l.y_mm), (123.45, 67.89));
    assert!(Location::from_string("invalid string").is_none());
    assert_eq!(
        Location::new(100.0, 50.0, "").to_string(),
        "(100.00, 50.00) mm"
    );
    assert_eq!(
        Location::new(100.0, 50.0, "F.Cu").to_string(),
        "(100.00, 50.00) mm on F.Cu"
    );
}

#[test]
fn violation_properties_and_to_json() {
    let mut v = DRCViolation::new(
        ViolationType::CLEARANCE,
        "clearance",
        Severity::Error,
        "Test message",
    );
    assert!(v.primary_location().is_none());
    assert_eq!(v.delta_mm(), None);
    v.locations.push(Location::new(100.0, 50.0, ""));
    assert_eq!(
        v.to_string(),
        "[clearance]: Test message at (100.00, 50.00) mm"
    );
    v.required_value_mm = Some(0.2);
    v.actual_value_mm = Some(0.15);
    assert!((v.delta_mm().unwrap() - 0.05).abs() < 1e-9);
    let d = v.to_json();
    let keys: Vec<&str> = match &d {
        Json::Obj(o) => o.iter().map(|(k, _)| k.as_str()).collect(),
        _ => unreachable!(),
    };
    assert_eq!(
        keys,
        [
            "type",
            "type_str",
            "severity",
            "category",
            "message",
            "rule",
            "locations",
            "items",
            "nets",
            "required_value_mm",
            "actual_value_mm",
            "delta_mm"
        ]
    );
    v.waived = true;
    v.waiver_reason = Some("r".into());
    assert!(!v.is_error());
    let d = v.to_json();
    assert_eq!(d.get("status"), Some(&Json::from("waived")));
    assert_eq!(d.get("severity"), Some(&Json::from("error")));
}

// ------------------------------------------------------------- suggestions

#[test]
fn direction_names() {
    assert_eq!(direction_name(1.0, 0.0), "right");
    assert_eq!(direction_name(-1.0, 0.0), "left");
    assert_eq!(direction_name(0.0, -1.0), "up");
    assert_eq!(direction_name(0.0, 1.0), "down");
    assert_eq!(direction_name(1.0, -1.0), "up-right");
    assert_eq!(direction_name(-1.0, 1.0), "down-left");
    assert_eq!(direction_name(0.0, 0.0), "in place");
}

#[test]
fn fix_suggestion_to_json() {
    let alt = FixSuggestion::new(FixAction::Reroute, "NET1", "Reroute NET1", 1, "easy");
    let mut s = FixSuggestion::new(FixAction::Move, "R1", "Move R1 0.5mm right", 1, "easy");
    s.parameters = vec![("dx".into(), Json::Float(0.5))];
    s.alternatives = vec![alt];
    let d = s.to_json();
    assert_eq!(d.get("action"), Some(&Json::from("move")));
    assert_eq!(
        d.get("parameters").unwrap().get("dx"),
        Some(&Json::Float(0.5))
    );
    assert_eq!(
        d.get("alternatives").unwrap().as_array().unwrap()[0].get("action"),
        Some(&Json::from("reroute"))
    );
    assert_eq!(s.to_string(), "Move R1 0.5mm right");
}

fn clearance_violation() -> DRCViolation {
    let mut v = DRCViolation::new(
        ViolationType::CLEARANCE,
        "clearance",
        Severity::Error,
        "Clearance violation",
    );
    v.required_value_mm = Some(0.2);
    v.actual_value_mm = Some(0.15);
    v.locations = vec![
        Location::new(100.0, 50.0, "F.Cu"),
        Location::new(100.5, 50.0, "F.Cu"),
    ];
    v.items = vec!["Pad 1 of R1".into(), "Via [GND]".into()];
    v
}

#[test]
fn clearance_fix() {
    let v = clearance_violation();
    let s = calculate_clearance_fix(&v, 0.1).unwrap();
    assert_eq!(s.action, FixAction::Move);
    assert_eq!(s.target, "Pad 1 of R1");
    assert_eq!(s.description, "Move Pad 1 of R1 0.15mm left");
    assert_eq!(s.parameter("distance_mm"), Some(&Json::Float(0.15)));
    assert_eq!(s.parameter("dx"), Some(&Json::Float(-0.15)));
    assert_eq!(s.alternatives.len(), 2);
    assert_eq!(s.alternatives[0].description, "Move Via [GND] 0.15mm right");
    assert_eq!(s.alternatives[1].action, FixAction::Reroute);

    let mut nov = v.clone();
    nov.required_value_mm = None;
    let s = calculate_clearance_fix(&nov, 0.1).unwrap();
    assert_eq!(s.action, FixAction::Reroute);
    assert_eq!(s.complexity, "moderate");

    assert!(
        calculate_clearance_fix(&viol(ViolationType::TRACK_WIDTH, Severity::Error), 0.1).is_none()
    );
}

#[test]
fn fix_suggestions_for_sample_report() {
    let r = sample();
    let fix = |t| generate_fix_suggestions(r.by_type(t)[0]).unwrap();
    assert_eq!(fix(ViolationType::CLEARANCE).action, FixAction::Move);
    assert_eq!(fix(ViolationType::SHORTING_ITEMS).action, FixAction::Delete);
    let u = fix(ViolationType::UNCONNECTED_ITEMS);
    assert_eq!(u.action, FixAction::Connect);
    assert_eq!(u.target, "NET1");
    let tw = fix(ViolationType::TRACK_WIDTH);
    assert_eq!(tw.action, FixAction::Resize);
    assert_eq!(tw.description, "Increase track width to 0.100mm (+0.020mm)");

    let mut annular = viol(ViolationType::VIA_ANNULAR_WIDTH, Severity::Error);
    annular.required_value_mm = Some(0.15);
    annular.actual_value_mm = Some(0.1);
    let s = generate_fix_suggestions(&annular).unwrap();
    assert_eq!(s.description, "Increase via pad diameter by 0.100mm");
    assert_eq!(s.alternatives.len(), 1);

    let silk = generate_fix_suggestions(&viol(ViolationType::SILK_OVER_COPPER, Severity::Warning))
        .unwrap();
    assert_eq!(silk.complexity, "trivial");

    let mut edge = viol(ViolationType::COPPER_EDGE_CLEARANCE, Severity::Error);
    edge.required_value_mm = Some(0.5);
    edge.actual_value_mm = Some(0.3);
    let s = generate_fix_suggestions(&edge).unwrap();
    assert_eq!(s.description, "Move copper 0.30mm away from board edge");

    let mut pp = viol(ViolationType::CLEARANCE_PAD_PAD, Severity::Error);
    pp.items = vec!["Pad 1 of U1".into(), "Pad 2 of U1".into()];
    assert_eq!(
        generate_fix_suggestions(&pp).unwrap().action,
        FixAction::AdjustRule
    );
    assert!(generate_fix_suggestions(&viol(ViolationType::UNKNOWN, Severity::Error)).is_none());
}

#[test]
fn feedback_drc_suggestions() {
    let v = clearance_violation();
    let s = generate_drc_suggestions(&v);
    assert_eq!(
        s[0],
        "Move affected elements apart by at least 0.05mm to meet clearance requirement of 0.20mm"
    );
    assert!(s
        .iter()
        .any(|x| x.contains("Move via to a different location")));
    assert!(s.iter().any(|x| x.contains("pad-to-pad spacing")));

    let mut generic = viol(ViolationType::UNKNOWN, Severity::Error);
    generic.type_str = "weird".into();
    generic.nets = vec!["A".into(), "B".into()];
    generic.locations = vec![Location::new(1.0, 2.0, "")];
    let s = generate_drc_suggestions(&generic);
    assert_eq!(
        s[0],
        "Review 'weird' violation details and affected elements"
    );
    assert_eq!(s[3], "Check net class settings for affected nets: A, B");
    assert_eq!(s[4], "Inspect area around (1.00, 2.00)mm");

    // 0.0 is falsy upstream: no measurement line.
    let mut zero = clearance_violation();
    zero.actual_value_mm = Some(0.0);
    assert!(!generate_drc_suggestions(&zero)[0].starts_with("Move affected"));
}

// ---------------------------------------------------------- manufacturer

#[test]
fn manufacturer_checks() {
    let r = sample();
    let checks = check_manufacturer_rules(&r, "jlcpcb", 2, 1.0);
    assert!(!checks.is_empty());
    for c in checks.iter().filter(|c| c.rule_name == "connection") {
        assert_eq!(c.result, CheckResult::Fail);
    }
    let clearance: Vec<_> = checks
        .iter()
        .filter(|c| c.rule_name == "min_clearance")
        .collect();
    assert!(clearance.iter().any(|c| c.actual_value.is_some()));
    assert_eq!(
        clearance[0].message,
        "Clearance 0.1500mm vs JLCPCB min 0.1270mm"
    );

    let unknown = check_manufacturer_rules(&r, "unknown_mfr", 2, 1.0);
    assert_eq!(unknown.len(), 5);
    assert!(unknown.iter().all(|c| c.result == CheckResult::Unknown));

    let s = summarize_checks(&checks);
    assert_eq!(s.get("total"), Some(&Json::from(checks.len())));
    assert!(s.get("by_rule").unwrap().contains_key("connection"));
}

// ------------------------------------------------------------- drc summary

#[test]
fn summary_severity_rules() {
    use IssueSeverity::*;
    let sev = |t| get_severity(&viol(t, Severity::Error));
    assert_eq!(sev(ViolationType::CLEARANCE), Blocking);
    assert_eq!(sev(ViolationType::SHORTING_ITEMS), Blocking);
    assert_eq!(sev(ViolationType::TRACK_WIDTH), Blocking);
    assert_eq!(sev(ViolationType::UNCONNECTED_ITEMS), Warning);
    assert_eq!(sev(ViolationType::COURTYARD_OVERLAP), Warning);
    assert_eq!(sev(ViolationType::SILK_OVER_COPPER), Cosmetic);
    assert_eq!(sev(ViolationType::SILK_OVERLAP), Cosmetic);
    assert_eq!(sev(ViolationType::UNKNOWN), Warning);
}

#[test]
fn summary_table_matches_upstream_with_fab() {
    let mut r = sample();
    r.source_file = "sample_drc.rpt".into();
    let s = create_summary(&r, Some("jlcpcb"), 2).unwrap();
    let expected = "
DRC Summary: sample_drc.rpt
============================================================

BLOCKING (2 violations):
  X 1 shorting_items
  X 1 track_width

FAB-ACCEPTABLE (2 violations for JLCPCB):
  ~ 2 clearance (0.150mm >= JLCPCB min 0.127mm)

WARNINGS (1 violations):
  ! 1 unconnected_items
      NET1: 1

  Only 2 of 5 violations are blocking for JLCPCB

============================================================
VERDICT: BLOCKING - 2 violation(s) fail JLCPCB rules
";
    assert_eq!(summary_table(&s, false), expected);

    let none = create_summary(&r, None, 2).unwrap();
    assert_eq!(none.blocking.len(), 4);
    assert_eq!(none.verdict(), "BLOCKING - Fix issues before manufacturing");
    let j = none.to_json();
    assert!(j.get("manufacturer").is_none());
    assert_eq!(
        dumps(j.get("unconnected_by_net").unwrap()),
        r#"{"NET1": 1}"#
    );
}

// ------------------------------------------------------------------ filters

#[test]
fn filter_construction() {
    assert!(
        ViolationFilter::new(None, None, None, None, None, "suppress", "")
            .unwrap_err()
            .to_string()
            .contains("Invalid filter action 'suppress'")
    );
    assert!(ViolationFilter::by_type("[invalid", "ignore")
        .unwrap_err()
        .to_string()
        .contains("Invalid regex in type_pattern"));
}

#[test]
fn filter_matching_drc() {
    let mut v = viol(ViolationType::SILK_OVERLAP, Severity::Warning);
    v.message = "Silkscreen overlap on U1".into();
    v.items = vec!["Pad 1 of U1".into()];
    v.nets = vec!["GND".into()];
    let f =
        |t: Option<&str>, m: Option<&str>, c: Option<&str>, n: Option<&str>, s: Option<&str>| {
            ViolationFilter::new(t, m, c, n, s, "ignore", "").unwrap()
        };
    assert!(f(
        Some("silk_overlap|silkscreen_over_pad"),
        None,
        None,
        None,
        None
    )
    .matches(&v));
    assert!(f(None, Some("OVERLAP"), None, None, None).matches(&v));
    assert!(f(None, None, Some("^U\\d+$"), None, None).matches(&v));
    assert!(f(None, None, None, Some("GND|VCC"), None).matches(&v));
    assert!(!f(None, None, None, None, Some("/sub")).matches(&v));
    assert!(!f(Some("silk"), Some("nomatch"), None, None, None).matches(&v));
    assert!(f(None, None, None, None, None).matches(&v));
    let mut nonets = v.clone();
    nonets.nets.clear();
    assert!(!f(None, None, None, Some("GND"), None).matches(&nonets));
    let mut norefs = v.clone();
    norefs.items = vec!["Track".into()];
    assert!(!f(None, None, Some("U1"), None, None).matches(&norefs));
}

#[test]
fn filter_matching_erc_and_engine() {
    let mut e = ERCViolation::new(
        ERCViolationType::PIN_NOT_CONNECTED,
        "pin_not_connected",
        kct::erc::Severity::Error,
        "Pin not connected",
    );
    e.sheet = "/sub/power".into();
    e.items = vec!["Pin 1 of R1".into()];
    let sheet = ViolationFilter::new(None, None, None, None, Some("/sub"), "warning", "").unwrap();
    assert!(sheet.matches(&e));
    let result = FilterEngine::new(vec![sheet]).apply(std::slice::from_ref(&e));
    assert_eq!(result.reclassified_count(), 1);
    assert_eq!(result.kept[0].severity, kct::erc::Severity::Warning);
    // Original untouched.
    assert_eq!(e.severity, kct::erc::Severity::Error);

    let vs = vec![
        viol(ViolationType::SILK_OVERLAP, Severity::Warning),
        viol(ViolationType::CLEARANCE, Severity::Warning),
        viol(ViolationType::TRACK_WIDTH, Severity::Error),
    ];
    let engine = FilterEngine::new(vec![
        ViolationFilter::by_type("silk", "ignore").unwrap(),
        ViolationFilter::by_type("clearance", "error").unwrap(),
        ViolationFilter::by_type("clearance", "ignore").unwrap(),
    ]);
    let r = engine.apply(&vs);
    assert_eq!(r.raw_count, 3);
    assert_eq!(r.ignored_count(), 1);
    assert_eq!(r.reclassified_count(), 1);
    assert_eq!(r.kept_count(), 2);
    assert_eq!(r.kept[0].severity, Severity::Error);
    assert_eq!(FilterEngine::default().apply(&vs).kept_count(), 3);
}

#[test]
fn filter_config_parsing() {
    let cfg = loads(
        r#"{"drc": {"filters": [{"type_pattern": "silk"}, {"type_pattern": "x", "action": "warning"}]},
            "erc": {"filters": [{"sheet_pattern": "/a", "action": "error"}]}}"#,
    )
    .unwrap();
    let (drc, erc) = parse_filters_from_config(&cfg).unwrap();
    assert_eq!(drc.len(), 2);
    assert_eq!(drc[0].action, "ignore");
    assert_eq!(erc.len(), 1);
    assert_eq!(parse_filters_from_config(&Json::obj()).unwrap().0.len(), 0);
    let bad = loads(r#"{"drc": {"filters": [{"action": "nope"}]}}"#).unwrap();
    assert!(parse_filters_from_config(&bad)
        .unwrap_err()
        .to_string()
        .starts_with("[drc.filters] entry 0: Invalid filter action"));

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f.toml");
    std::fs::write(
        &p,
        "[[drc.filters]]\ntype_pattern = \"silk\"\naction = \"ignore\"\n\n[[erc.filters]]\ntype_pattern = \"label\"\n",
    )
    .unwrap();
    let (drc, erc) = load_filters_from_toml(&p).unwrap();
    assert_eq!((drc.len(), erc.len()), (1, 1));
    assert!(matches!(
        load_filters_from_toml(dir.path().join("missing.toml")),
        Err(FilterLoadError::NotFound(_))
    ));
    std::fs::write(&p, "[[drc.filters]\n").unwrap();
    assert!(matches!(
        load_filters_from_toml(&p),
        Err(FilterLoadError::Config(_))
    ));
}

// ------------------------------------------------------------------ waivers

fn waivers(json: &str) -> Waivers {
    waivers_from_json(&loads(json).unwrap()).unwrap()
}

#[test]
fn waiver_loading_validation() {
    let err = |j: &str| {
        waivers_from_json(&loads(j).unwrap())
            .unwrap_err()
            .to_string()
    };
    assert_eq!(err("[]"), "waivers file must be a JSON object, got list");
    assert_eq!(
        err("{}"),
        "waivers file is missing the required 'version' key"
    );
    assert!(err(r#"{"version": 1}"#).starts_with("unsupported waivers version 1"));
    assert_eq!(
        err(r#"{"version": 2, "waivers": [{"rule": "x", "reason": "r", "issue": "i"}]}"#),
        "waiver #0 must name at least one 'items' or 'nets' entry"
    );
    assert_eq!(
        err(r#"{"version": 2, "waivers": [{"rule": "x", "items": ["A"], "issue": "i"}]}"#),
        "waiver #0 is missing a non-empty 'reason'"
    );
    assert_eq!(waivers(r#"{"version": 2}"#).len(), 0);
}

#[test]
fn waivers_apply_to_kicad_report() {
    let content = r#"{"source": "b.kicad_pcb", "violations": [
        {"type": "courtyards_overlap", "severity": "error", "description": "Courtyards overlap",
         "items": [{"description": "Footprint C52"}, {"description": "Footprint U10"}]},
        {"type": "courtyards_overlap", "severity": "error", "description": "Courtyards overlap",
         "items": [{"description": "Footprint C52"}, {"description": "Footprint U10"}, {"description": "Footprint R1"}]},
        {"type": "clearance", "severity": "error", "description": "Clearance",
         "items": [{"description": "Pad 1 [GND] of U3 on F.Cu"}, {"description": "Track [VBUS] on F.Cu"}]}]}"#;
    let mut r = parse_json_report(content, "drc.json").unwrap();
    let w = waivers(
        r#"{"version": 2, "waivers": [
        {"rule": "courtyards_overlap", "items": ["U10", "C52"], "reason": "tight", "issue": "chorus#18"},
        {"rule": "clearance", "items": ["U3"], "nets": ["GND", "VBUS"], "reason": "tie", "issue": "chorus#20"},
        {"rule": "clearance_pad_pad", "nets": ["A"], "reason": "other engine", "issue": "chorus#21"}]}"#,
    );
    let applied = apply_waivers_to_report(&mut r, &w);
    assert_eq!(applied.waived, vec![0, 2]);
    assert!(r.violations[0].is_waived() && !r.violations[0].is_error());
    // Exact-set: a 2-item waiver does not waive a 3-item finding.
    assert!(!r.violations[1].is_waived());
    assert_eq!(r.violations[0].severity, Severity::Error);
    assert_eq!(r.error_count(), 1);
    assert_eq!(applied.unused.len(), 1);
    assert_eq!(
        applied.unused_messages()[0],
        "Waiver for rule 'clearance_pad_pad' (nets=['A']) matched no violation (tracking \
         chorus#21); the underlying defect may already be resolved, the refs may have changed, \
         or the entry may be keyed to another engine's rule id."
    );
    assert_eq!(
        dumps(&applied.to_json_list()),
        r#"[{"rule": "clearance_pad_pad", "items": [], "nets": ["A"], "reason": "other engine", "issue": "chorus#21", "message": "Waiver for rule 'clearance_pad_pad' (nets=['A']) matched no violation (tracking chorus#21); the underlying defect may already be resolved, the refs may have changed, or the entry may be keyed to another engine's rule id."}]"#
    );
}

#[test]
fn net_compat() {
    use std::collections::HashMap;
    let nets: HashMap<i64, String> = [(3, "VCC".to_string())].into();
    assert_eq!(
        kct::drc::resolve_net_atom(Some("3"), Some(&nets), None),
        (3, "VCC".to_string())
    );
    assert_eq!(
        kct::drc::resolve_net_atom(Some("VCC"), None, None),
        (0, "VCC".to_string())
    );
}

// ---------------------------------------------------------------------- CLI

fn run(args: &[&str]) -> i32 {
    kct::cli::run(args.iter().map(|s| s.to_string())).unwrap()
}

#[test]
fn cli_drc_exit_codes() {
    let rpt = fixtures().join("sample_drc.rpt");
    let rpt = rpt.to_str().unwrap();
    assert_eq!(run(&["drc", rpt]), 1);
    assert_eq!(run(&["drc", rpt, "--format", "json"]), 1);
    assert_eq!(run(&["drc", rpt, "--type", "nomatch"]), 0);
    assert_eq!(run(&["drc", rpt, "--net", "SIG1"]), 0);
    assert_eq!(run(&["drc", rpt, "--net", "SIG1", "--strict"]), 2);
    assert_eq!(run(&["drc", rpt, "--mfr", "jlcpcb"]), 1);
    assert_eq!(run(&["drc", "missing.json"]), 1);
    assert_eq!(run(&["drc", "board.txt"]), 1);
    assert_eq!(run(&["drc", "--compare"]), 0);
    assert_eq!(run(&["drc", "--rules", "--mfr", "seeed"]), 0);
    assert_eq!(run(&["drc"]), 1);
}

#[test]
fn cli_drc_waivers_gate() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("drc.json");
    std::fs::write(
        &report,
        r#"{"source": "b.kicad_pcb", "violations": [{"type": "courtyards_overlap", "severity": "error",
        "description": "Courtyards overlap", "items": [{"description": "Footprint C52"}, {"description": "Footprint U10"}]}]}"#,
    )
    .unwrap();
    let report = report.to_str().unwrap();
    assert_eq!(run(&["drc", report]), 1);
    std::fs::write(
        dir.path().join(".kct_waivers.json"),
        r#"{"version": 2, "waivers": [{"rule": "courtyards_overlap", "items": ["C52", "U10"], "reason": "ok", "issue": "x#1"}]}"#,
    )
    .unwrap();
    // Auto-discovered sidecar waives the only error.
    assert_eq!(run(&["drc", report]), 0);
    // --mfr mode does not apply waivers (no violation it can judge -> 0).
    assert_eq!(run(&["drc", report, "--mfr", "jlcpcb"]), 0);
    // Explicit malformed file is a hard error.
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "{").unwrap();
    assert_eq!(run(&["drc", report, "--waivers", bad.to_str().unwrap()]), 1);
}
