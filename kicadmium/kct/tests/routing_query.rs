use kct::{lint::model::Point, routing_query::Probe};
fn write_board(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("board.kicad_pcb");
    std::fs::write(&path,r#"(kicad_pcb
(layers (0 "F.Cu" signal) (2 "In1.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
(net 1 "N") (net 2 "OTHER")
(footprint "R" (layer "F.Cu") (at 1 1) (property "Reference" "R1") (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu") (net 1 "N")))
(segment (start 5 1) (end 5 9) (width 0.4) (layer "In1.Cu") (net 2 "OTHER") (uuid "inner"))
(gr_rect (start 0 0) (end 10 10) (stroke (width 0.05) (type default)) (fill none) (layer "Edge.Cuts")))"#).unwrap();
    path
}
#[test]
fn via_checks_inner_layer_and_ranks_by_distance() {
    let d = tempfile::tempdir().unwrap();
    let path = write_board(d.path());
    let p = Probe::load(&path).unwrap();
    let result = p
        .via_sites("N", Point { x: 5., y: 5. }, 1.5, 0.6, 0.3, 0.25, 10)
        .unwrap();
    let sites = result["sites"].as_array().unwrap();
    assert!(!sites.is_empty());
    assert!(sites
        .iter()
        .all(|s| (s["at"]["x"].as_f64().unwrap() - 5.).abs() > 0.65));
    assert!(result["blocking_objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "inner"));
}
#[test]
fn corridor_reports_blocker_on_selected_layer() {
    let d = tempfile::tempdir().unwrap();
    let path = write_board(d.path());
    let p = Probe::load(&path).unwrap();
    let blocked = p
        .corridor(
            "N",
            "In1.Cu",
            Point { x: 2., y: 5. },
            Point { x: 8., y: 5. },
            0.25,
        )
        .unwrap();
    assert_eq!(blocked["fits"], false);
    let free = p
        .corridor(
            "N",
            "F.Cu",
            Point { x: 2., y: 5. },
            Point { x: 8., y: 5. },
            0.25,
        )
        .unwrap();
    assert_eq!(free["fits"], true);
}
#[test]
fn custom_conditions_apply_conservatively_and_unknown_constraints_fail() {
    let d = tempfile::tempdir().unwrap();
    let path = write_board(d.path());
    std::fs::write(path.with_extension("kicad_dru"),"(version 1) (rule \"wide\" (condition \"A.NetName == 'OTHER'\") (constraint clearance (min 1mm)))").unwrap();
    let p = Probe::load(&path).unwrap();
    let r = p
        .corridor(
            "N",
            "In1.Cu",
            Point { x: 4., y: 2. },
            Point { x: 4., y: 8. },
            0.25,
        )
        .unwrap();
    assert_eq!(r["fits"], false);
    std::fs::write(
        path.with_extension("kicad_dru"),
        "(version 1) (rule \"unsupported\" (constraint disallow via))",
    )
    .unwrap();
    assert!(Probe::load(&path).is_err());
    // Same-net physical spacing cannot be modeled as electrical clearance.
    std::fs::write(
        path.with_extension("kicad_dru"),
        "(version 1) (rule \"physical\" (constraint physical_clearance (min 1mm)))",
    )
    .unwrap();
    assert!(Probe::load(&path).is_err());
}
