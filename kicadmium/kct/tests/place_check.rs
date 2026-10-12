use kct::{edit_plan::Plan, schema::pcb::Pcb, sexp::parse};
#[test]
fn moved_offset_silk_label_checks_remote_via_without_writing() {
    let source = r#"(kicad_pcb (layers (0 "F.Cu" signal) (31 "B.Cu" signal)) (net 1 "N")
    (footprint "R" (uuid "r") (layer "F.Cu") (at 0 0)
      (property "Reference" "R1" (at 0 -1) (layer "F.Fab"))
      (fp_text user "GPS_RX" (at 6 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15)))))
    (via (at 26 20) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (tenting (front no) (back no)) (net 1) (uuid "via")))"#;
    let pcb = Pcb::from_sexp(parse(source).unwrap()).unwrap();
    let before = pcb.sexp().clone();
    let plan: Plan =
        serde_yaml::from_str("version: 1\noperations:\n- {op: move, reference: R1, at: [20, 20]}")
            .unwrap();
    let report = kct::place_check::check(&pcb, &plan, 2.).unwrap();
    assert!(
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["rule"] == "silk_over_copper"),
        "{report}"
    );
    assert_eq!(pcb.sexp(), &before);
}
#[test]
fn preflight_catches_small_courtyard_overlap_without_mutation() {
    let mut s = String::from("(kicad_pcb (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal))");
    for (reference, x) in [("U1", 0), ("C1", 10), ("R99", 100)] {
        s += &format!(
            r#"(footprint "IC" (uuid "{reference}") (layer "F.Cu") (at {x} 0) (property "Reference" "{reference}") (fp_rect (start -1 -1) (end 1 1) (stroke (width 0.05) (type default)) (fill none) (layer "F.CrtYd")))"#
        );
    }
    s.push(')');
    let pcb = Pcb::from_sexp(parse(&s).unwrap()).unwrap();
    let before = pcb.sexp().clone();
    let plan: Plan = serde_yaml::from_str(
        "version: 1\noperations:\n- {op: move, reference: U1, at: [8.02, 0]}\n",
    )
    .unwrap();
    let report = kct::place_check::check(&pcb, &plan, 2.).unwrap();
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["rule"].as_str().unwrap().contains("courtyard")));
    assert_eq!(report["checked_footprints"], 2);
    assert_eq!(pcb.sexp(), &before);
}
#[test]
fn rejects_nonplacement_plan() {
    let pcb = Pcb::from_sexp(parse("(kicad_pcb)").unwrap()).unwrap();
    let plan: Plan = serde_yaml::from_str(
        "version: 1\noperations:\n- {op: add-via, net: N, at: [1, 2], size: 0.6, drill: 0.3}",
    )
    .unwrap();
    assert!(kct::place_check::check(&pcb, &plan, 2.).is_err());
}
#[test]
fn decoupling_uses_profile_distance_and_side() {
    let source = r#"(kicad_pcb (layers (0 "F.Cu" signal) (31 "B.Cu" signal)) (net 1 "VCC") (net 2 "GND") (footprint "IC" (uuid "u") (layer "F.Cu") (at 0 0) (property "Reference" "U1") (pad "1" smd rect (at 0 0) (size 0.5 0.5) (layers "F.Cu") (net 1 "VCC"))) (footprint "C" (uuid "c") (layer "B.Cu") (at 1 0) (property "Reference" "C1") (property "Value" "100n") (pad "1" smd rect (at 0 0) (size 0.5 0.5) (layers "B.Cu") (net 1 "VCC")) (pad "2" smd rect (at 1 0) (size 0.5 0.5) (layers "B.Cu") (net 2 "GND"))))"#;
    let pcb = Pcb::from_sexp(parse(source).unwrap()).unwrap();
    let plan: Plan =
        serde_yaml::from_str("version: 1\noperations:\n- {op: move, reference: U1, at: [5, 0]}")
            .unwrap();
    let count = |r: serde_json::Value| {
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["rule"] == "decoupling.max_pin_distance")
            .count()
    };
    assert_eq!(
        count(kct::place_check::check_profile(&pcb, &plan, 2., false).unwrap()),
        1
    );
    assert_eq!(
        count(kct::place_check::check_profile(&pcb, &plan, 5., false).unwrap()),
        0
    );
    assert_eq!(
        count(kct::place_check::check_profile(&pcb, &plan, 5., true).unwrap()),
        1
    );
}
