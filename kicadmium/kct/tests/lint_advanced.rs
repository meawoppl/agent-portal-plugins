use kct::lint::{lint, Config, Report};
use serde_json::json;
const BAD: &str = include_str!("fixtures/lint/advanced-bad.kicad_pcb");
fn configuration() -> Config {
    serde_json::from_str(include_str!("fixtures/lint/advanced-bad.json")).unwrap()
}
fn count(r: &Report, id: &str) -> usize {
    r.findings.iter().filter(|f| f.rule == id).count()
}
fn selected(rule: &str) -> Report {
    let mut c = configuration();
    c.enabled_only.insert(rule.into());
    lint(BAD, "fixture", c).unwrap()
}
macro_rules! detection {
    ($name:ident,$rule:literal) => {
        #[test]
        fn $name() {
            let r = selected($rule);
            assert!(
                count(&r, $rule) > 0,
                "{} did not detect fixture fault",
                $rule
            );
            assert!(r.findings.iter().all(|f| f.rule == $rule));
            assert_eq!(
                r.coverage.iter().find(|r| r.rule == $rule).unwrap().status,
                "evaluated"
            );
        }
    };
}
detection!(copper_island, "copper.island");
detection!(copper_stale_stub, "copper.stale_stub");
detection!(copper_duplicate_overlap, "copper.duplicate_overlap");
detection!(via_avoidable, "via.avoidable");
detection!(via_role_missing, "via.role_missing");
detection!(via_return_distance, "via.return_distance");
detection!(route_legal_shortcut, "route.legal_shortcut");
detection!(route_redundant_bend, "route.redundant_bend");
detection!(route_backtrack, "route.backtrack");
detection!(pad_grazing, "pad.grazing");
detection!(route_tuning_integrity, "route.tuning_integrity");
detection!(placement_airwire_cost, "placement.airwire_cost");
detection!(placement_congestion, "placement.congestion");
detection!(placement_rotation, "placement.rotation");
detection!(placement_pitch, "placement.pitch");
detection!(placement_orientation, "placement.orientation");
detection!(channel_topology, "channel.topology");
detection!(channel_geometry, "channel.geometry");
detection!(channel_polarity, "channel.polarity");
detection!(block_flow, "block.flow");
detection!(power_corridor, "power.corridor");
detection!(noise_partition, "noise.partition");
detection!(decoupling_pin_distance, "decoupling.pin_distance");
detection!(decoupling_loop_area, "decoupling.loop_area");
detection!(regulator_feedback, "regulator.feedback");
detection!(regulator_hot_loop, "regulator.hot_loop");
detection!(schematic_collisions, "schematic.collisions");
detection!(schematic_junction, "schematic.junction");
detection!(schematic_stem, "schematic.stem");
detection!(schematic_reading_order, "schematic.reading_order");
detection!(netlist_parity, "netlist.parity");
detection!(connectivity_regression, "connectivity.regression");
detection!(power_width_capacity, "power.width_capacity");
detection!(neck_clearance_reason, "neck.clearance_reason");
detection!(width_taper, "width.taper");
detection!(feature_unrequested, "feature.unrequested");
detection!(pin_swap_candidate, "pin.swap_candidate");
detection!(pin_swap_legality, "pin.swap_legality");
detection!(net_naming, "net.naming");
detection!(interface_completeness, "interface.completeness");
detection!(connector_mating, "connector.mating");
detection!(testpoint_coverage, "testpoint.coverage");
detection!(testpoint_access, "testpoint.access");
detection!(testpoint_ground, "testpoint.ground");
detection!(testpoint_label, "testpoint.label");
detection!(pair_skew, "pair.skew");
detection!(pair_spacing, "pair.spacing");
detection!(reference_discontinuity, "reference.discontinuity");
detection!(rf_launch, "rf.launch");
detection!(power_pin_roles, "power.pin_roles");
detection!(strap_truth_table, "strap.truth_table");
detection!(source_series_parallel, "source.series_parallel");
detection!(source_backfeed, "source.backfeed");
detection!(source_polarity, "source.polarity");
detection!(capacitor_role, "capacitor.role");
detection!(capacitor_rating, "capacitor.rating");
detection!(parts_package, "parts.package");
detection!(mechanics_edge_clearance, "mechanics.edge_clearance");
detection!(mechanics_access, "mechanics.access");
detection!(thermal_spokes, "thermal.spokes");
detection!(bom_completeness, "bom.completeness");
detection!(cpl_orientation, "cpl.orientation");
detection!(silk_collisions, "silk.collisions");
detection!(silk_content, "silk.content");
detection!(export_freshness, "export.freshness");
detection!(release_manifest, "release.manifest");
detection!(checker_transform, "checker.transform");
detection!(checker_differential, "checker.differential");
detection!(checker_noise, "checker.noise");
detection!(review_reattach, "review.reattach");
detection!(annotation_stale, "annotation.stale");
#[test]
fn all_102_are_registered_and_executable() {
    let catalog = kct::lint::rules::catalog();
    assert_eq!(catalog.len(), 102);
    assert_eq!(kct::lint::advanced::IDS.len(), 71);
    assert!(catalog.iter().all(|r| r.status == "implemented"));
    let r = lint(BAD, "fixture", configuration()).unwrap();
    assert_eq!(r.coverage.len(), 102);
    for id in kct::lint::advanced::IDS {
        assert!(count(&r, id) > 0, "{id}");
    }
}
#[test]
fn no_contract_is_not_a_pass() {
    let r = lint(
        r#"(kicad_pcb (layers (0 "F.Cu" signal)))"#,
        "empty",
        Config::default(),
    )
    .unwrap();
    for id in [
        "source.series_parallel",
        "capacitor.rating",
        "reference.discontinuity",
        "bom.completeness",
        "checker.transform",
    ] {
        assert_eq!(
            r.coverage.iter().find(|c| c.rule == id).unwrap().status,
            "needs_input"
        );
        assert_eq!(count(&r, id), 0);
    }
}
#[test]
fn stale_native_snapshot_is_not_compared() {
    let mut c = configuration();
    c.intent.native.as_mut().unwrap().source_sha256 = "wrong".into();
    let r = lint(BAD, "fixture", c).unwrap();
    assert_eq!(count(&r, "checker.transform"), 0);
    assert_eq!(
        r.coverage
            .iter()
            .find(|r| r.rule == "checker.transform")
            .unwrap()
            .status,
        "stale_input"
    );
}
#[test]
fn two_isolated_batteries_and_one_psu_pass() {
    let mut c = Config::default();
    let mode = json!({"name":"batteries","sources":[{"name":"low","negative":"GND","positive":"MID","volts":12,"isolation_group":"a"},{"name":"high","negative":"MID","positive":"VIN","volts":12,"isolation_group":"b"}],"links":[],"output_positive":"VIN","output_negative":"GND","target_volts":24,"tolerance_volts":0.1,"allow_parallel":false,"connector_pins":[]});
    c.intent.modes.push(serde_json::from_value(mode).unwrap());
    let mut one = c.intent.modes[0].clone();
    one.sources.remove(1);
    one.sources[0].volts = 24.;
    one.sources[0].positive = "VIN".into();
    one.name = "single psu".into();
    c.intent.modes.push(one);
    let r = lint(
        r#"(kicad_pcb (layers (0 "F.Cu" signal)))"#,
        "power",
        c.clone(),
    )
    .unwrap();
    assert_eq!(count(&r, "source.series_parallel"), 0);
    assert_eq!(count(&r, "source.backfeed"), 0);
    c.intent.modes[0].sources[1].isolation_group = "a".into();
    assert_eq!(
        count(
            &lint(r#"(kicad_pcb (layers (0 "F.Cu" signal)))"#, "power", c).unwrap(),
            "source.series_parallel"
        ),
        1
    );
}
#[test]
fn insufficient_search_budget_is_visible() {
    let mut c: Config = serde_json::from_value(
        json!({"intent":{"route":{"outline":[[-5,-5],[10,-5],[10,10],[-5,10]]}}}),
    )
    .unwrap();
    c.intent.route.max_nodes = 8;
    // Obstruct both simple shortcuts so the bounded graph search is required.
    let board = r#"(kicad_pcb (layers (0 "F.Cu" signal))
      (segment (uuid "a") (net "N") (layer "F.Cu") (width 0.3) (start 0 0) (end 2 0))
      (segment (uuid "b") (net "N") (layer "F.Cu") (width 0.3) (start 2 0) (end 2 2))
      (segment (uuid "block") (net "OTHER") (layer "F.Cu") (width 1.2) (start 0.8 0.8) (end 1.2 1.2)))"#;
    let r = lint(board, "fixture", c).unwrap();
    assert!(r
        .coverage
        .iter()
        .any(|r| r.rule == "route.legal_shortcut" && r.status == "budget_exhausted"));
}
#[test]
fn fixed_pins_are_not_swap_candidates() {
    let mut c = configuration();
    for g in &mut c.intent.swaps {
        for p in &mut g.pins {
            p.mate_fixed = true;
        }
    }
    let r = lint(BAD, "fixture", c).unwrap();
    assert_eq!(count(&r, "pin.swap_candidate"), 0);
}
#[test]
fn known_good_contracts_stay_quiet() {
    let mut c = configuration();
    let b = kct::lint::model::Board::read(BAD).unwrap();
    let i = &mut c.intent;
    i.features.as_mut().unwrap().allowed_references =
        b.parts.iter().map(|p| p.reference.clone()).collect();
    i.features.as_mut().unwrap().required_references = vec!["U1".into()];
    i.peers[0].pitch_mm = 10.;
    i.peers[0].angle_tolerance_deg = 180.;
    i.flows[0].references.reverse();
    i.corridors[0].max_separation_mm = 100.;
    i.naming[0].pattern = "^N$".into();
    i.capacitors[0].role = "decoupling".into();
    i.capacitors[0].net = "A".into();
    i.capacitors[0].source = "manufacturer".into();
    i.capacitors[0].rating_volts = 50.;
    i.capacitors[0].effective_uf = 1.;
    i.straps[0].target_net = "GND".into();
    i.straps[0].high_ohm = 20000.;
    i.mating[0].expected[0].net = "V24".into();
    i.pin_roles[0].pin.net = "V24".into();
    i.interfaces[0].pins[0].net = "V24".into();
    i.interfaces[0].required_references = vec!["J1".into()];
    i.currents[0].amperes = 0.001;
    i.pairs[0].max_skew_mm = 3.;
    i.pairs[0].max_length_mm = 11.;
    i.pairs[0].gap_mm = 1.7;
    i.labels[0].text = "U1".into();
    i.assembly.as_mut().unwrap().fields = vec!["Reference".into()];
    i.assembly.as_mut().unwrap().placements = b
        .parts
        .iter()
        .map(|p| kct::lint::intent::Cpl {
            reference: p.reference.clone(),
            at: p.at,
            angle_deg: p.angle,
            side: "F".into(),
        })
        .collect();
    i.netlist = Some(
        b.pads
            .iter()
            .map(|p| kct::lint::intent::Pin {
                reference: p.reference.clone(),
                pad: p.number.clone(),
                net: p.net.clone(),
            })
            .collect(),
    );
    i.observations[0].predicted = false;
    i.annotations[0].subjects = vec!["U1".into()];
    i.annotations[0].source_sha256 = kct::lint::hash(BAD);
    i.artifacts[0].actual_sha256 = "a".into();
    i.artifacts[0].built_from_sha256 = kct::lint::hash(BAD);
    i.release
        .as_mut()
        .unwrap()
        .checks
        .push(kct::lint::intent::CheckRun {
            tool: "DRC".into(),
            version: "10".into(),
            source_sha256: kct::lint::hash(BAD),
            unwaived_errors: 0,
        });
    let r = lint(BAD, "fixture", c).unwrap();
    for id in [
        "feature.unrequested",
        "placement.pitch",
        "placement.orientation",
        "block.flow",
        "power.corridor",
        "net.naming",
        "capacitor.role",
        "capacitor.rating",
        "strap.truth_table",
        "connector.mating",
        "power.pin_roles",
        "interface.completeness",
        "power.width_capacity",
        "pair.skew",
        "pair.spacing",
        "silk.content",
        "bom.completeness",
        "cpl.orientation",
        "netlist.parity",
        "checker.noise",
        "annotation.stale",
        "export.freshness",
        "release.manifest",
    ] {
        assert_eq!(count(&r, id), 0, "unexpected {id}");
    }
}
#[test]
fn arc_connectivity_and_outline_cutouts() {
    let s = r#"(kicad_pcb (layers (0 "F.Cu" signal)) (gr_rect (start -5 -5) (end 5 5) (layer "Edge.Cuts")) (gr_rect (start -1 -1) (end 1 1) (layer "Edge.Cuts")) (arc (start -2 0) (mid 0 2) (end 2 0) (width .2) (layer "F.Cu") (net "N") (uuid "arc")) (footprint "X" (layer "F.Cu") (at -2 0) (uuid "f") (property "Reference" "J") (pad "1" smd circle (at 0 0) (size 1 1) (layers "F.Cu") (net "N") (uuid "p"))))"#;
    let b = kct::lint::model::Board::read(s).unwrap();
    let g = kct::lint::copper::Geometry::new(
        &b,
        kct::lint::copper::Extra::read(s, &b).unwrap(),
        &Default::default(),
    );
    assert_eq!(g.islands().len(), 1);
    assert!(!g.legal(
        &[
            kct::lint::model::Point { x: -2., y: 0. },
            kct::lint::model::Point { x: 2., y: 0. }
        ],
        0.2,
        "N",
        "F.Cu",
        &Default::default()
    ));
}
#[test]
fn unsupported_geometry_never_certifies_connectivity() {
    let r = lint(
        r#"(kicad_pcb (layers (0 "F.Cu" signal)) (zone (net "N") (layer "F.Cu") (polygon (pts (xy 0 0) (xy 1 0) (xy 1 1)))))"#,
        "x",
        Config::default(),
    )
    .unwrap();
    assert_eq!(
        r.coverage
            .iter()
            .find(|r| r.rule == "copper.island")
            .unwrap()
            .status,
        "unsupported_geometry"
    );
}
#[test]
fn ordinary_disabled_prerequisite_does_not_hide_advanced_rule() {
    for id in ["width.taper", "via.avoidable", "neck.clearance_reason"] {
        assert!(count(&selected(id), id) > 0);
    }
}

#[test]
fn invalid_contract_inputs_are_errors() {
    let mut c = configuration();
    c.intent.pairs[0].gap_mm = -1.;
    assert!(lint(BAD, "fixture", c).is_err());
    let mut c = configuration();
    c.intent.placement[0].step_mm = f64::NAN;
    assert!(lint(BAD, "fixture", c).is_err());
    let mut c = configuration();
    c.intent.observations[0].max_false_positive_rate = 1.1;
    assert!(lint(BAD, "fixture", c).is_err());
}
