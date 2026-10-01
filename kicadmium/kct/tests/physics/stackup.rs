//! Port of upstream `tests/test_physics_stackup.py`, the physics-only cases of
//! `tests/test_named_stackup.py`, and the physics-only cases of
//! `tests/test_physics_integration.py`.

use kct::physics::stackup::{PcbStackupData, PcbStackupLayer};
use kct::physics::{
    copper_thickness_from_oz, get_material, get_material_or_default, CopperWeight, LayerType,
    Stackup, StackupLayer, TransmissionLine, COPPER_1OZ, COPPER_2OZ, COPPER_CONDUCTIVITY,
    COPPER_HALF_OZ, FR4_HIGH_TG, FR4_STANDARD, ROGERS_4350B, SPEED_OF_LIGHT,
};

fn layer(name: &str, layer_type: LayerType, thickness_mm: f64) -> StackupLayer {
    StackupLayer {
        thickness_mm,
        ..StackupLayer::new(name, layer_type)
    }
}

fn dielectric(name: &str, thickness_mm: f64, epsilon_r: f64, loss_tangent: f64) -> StackupLayer {
    StackupLayer {
        epsilon_r,
        loss_tangent,
        ..layer(name, LayerType::Dielectric, thickness_mm)
    }
}

fn h(stackup: &Stackup, name: &str) -> f64 {
    stackup.get_dielectric_height(name).unwrap()
}

fn assert_pair_approx(actual: (f64, f64), expected: (f64, f64)) {
    assert_approx!(actual.0, expected.0);
    assert_approx!(actual.1, expected.1);
}

// =========================================================================
// test_physics_stackup.py
// =========================================================================

// --- TestPhysicalConstants ---

#[test]
fn test_speed_of_light() {
    assert_eq!(SPEED_OF_LIGHT, 299792458.0);
}

#[test]
fn test_copper_conductivity() {
    assert_eq!(COPPER_CONDUCTIVITY, 5.8e7);
}

#[test]
fn test_bottom_microstrip_uses_inward_substrate_on_asymmetric_board() {
    let stack = Stackup {
        layers: vec![
            layer("F.Cu", LayerType::Copper, 0.035),
            dielectric("top prepreg", 0.1, 4.1, 0.01),
            layer("In1.Cu", LayerType::Copper, 0.0175),
            dielectric("core", 1.0, 4.5, 0.0),
            layer("In2.Cu", LayerType::Copper, 0.0175),
            dielectric("bottom prepreg", 0.2, 3.7, 0.005),
            layer("B.Cu", LayerType::Copper, 0.035),
            StackupLayer {
                epsilon_r: 3.3,
                ..layer("B.Mask", LayerType::SolderMask, 0.01)
            },
            // KiCad's parsed paste entry must never become the substrate.
            layer("B.Paste", LayerType::Dielectric, 0.0),
        ],
        board_thickness_mm: 1.4,
        ..Default::default()
    };
    assert_approx!(h(&stack, "F.Cu"), 0.1);
    assert_approx!(h(&stack, "B.Cu"), 0.2);
    assert_approx!(stack.get_dielectric_constant("B.Cu"), 3.7);
    assert_approx!(stack.get_loss_tangent("B.Cu"), 0.005);
    // Mirroring the physical stack preserves impedance at the mirrored face.
    let mirrored = Stackup {
        layers: stack.layers.iter().rev().cloned().collect(),
        board_thickness_mm: 1.4,
        ..Default::default()
    };
    let z = TransmissionLine::new(stack)
        .microstrip(0.2, "B.Cu", 1.0)
        .unwrap()
        .z0;
    let z_mirrored = TransmissionLine::new(mirrored)
        .microstrip(0.2, "B.Cu", 1.0)
        .unwrap()
        .z0;
    assert_approx!(z, z_mirrored);
}

#[test]
fn test_default_two_layer_has_equal_top_and_bottom_impedance() {
    let line = TransmissionLine::new(Stackup::default_2layer(1.6));
    assert_approx!(
        line.microstrip(0.3, "F.Cu", 1.0).unwrap().z0,
        line.microstrip(0.3, "B.Cu", 1.0).unwrap().z0
    );
}

// --- TestCopperWeight ---

#[test]
fn test_half_oz_copper() {
    assert_eq!(COPPER_HALF_OZ.oz, 0.5);
    assert_approx!(COPPER_HALF_OZ.thickness_um, 17.5, rel = 0.01);
    assert_approx!(COPPER_HALF_OZ.thickness_mm, 0.0175, rel = 0.01);
}

#[test]
fn test_1oz_copper() {
    assert_eq!(COPPER_1OZ.oz, 1.0);
    assert_approx!(COPPER_1OZ.thickness_um, 35.0, rel = 0.01);
    assert_approx!(COPPER_1OZ.thickness_mm, 0.035, rel = 0.01);
}

#[test]
fn test_2oz_copper() {
    assert_eq!(COPPER_2OZ.oz, 2.0);
    assert_approx!(COPPER_2OZ.thickness_um, 70.0, rel = 0.01);
    assert_approx!(COPPER_2OZ.thickness_mm, 0.070, rel = 0.01);
}

#[test]
fn test_copper_thickness_from_oz() {
    assert_approx!(copper_thickness_from_oz(1.0), 0.035, rel = 0.01);
    assert_approx!(copper_thickness_from_oz(0.5), 0.0175, rel = 0.01);
    assert_approx!(copper_thickness_from_oz(2.0), 0.070, rel = 0.01);
}

#[test]
fn test_custom_copper_weight() {
    let custom = CopperWeight::from_oz(1.5);
    assert_eq!(custom.oz, 1.5);
    assert_approx!(custom.thickness_um, 52.5, rel = 0.01);
}

// --- TestDielectricMaterials ---

#[test]
fn test_fr4_standard() {
    assert_eq!(FR4_STANDARD.name, "FR4");
    assert_approx!(FR4_STANDARD.epsilon_r, 4.5, rel = 0.1);
    assert_approx!(FR4_STANDARD.loss_tangent, 0.02, rel = 0.1);
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn test_fr4_high_tg() {
    assert_approx!(FR4_HIGH_TG.epsilon_r, 4.4, rel = 0.1);
    assert!(FR4_HIGH_TG.loss_tangent < FR4_STANDARD.loss_tangent);
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn test_rogers_4350b() {
    assert_approx!(ROGERS_4350B.epsilon_r, 3.48, rel = 0.05);
    assert_approx!(ROGERS_4350B.loss_tangent, 0.0037, rel = 0.1);
    // Rogers should have lower loss than FR4
    assert!(ROGERS_4350B.loss_tangent < FR4_STANDARD.loss_tangent);
}

#[test]
fn test_get_material_case_insensitive() {
    assert_eq!(get_material("fr4"), Some(&FR4_STANDARD));
    assert_eq!(get_material("FR4"), Some(&FR4_STANDARD));
    assert_eq!(get_material("Fr4"), Some(&FR4_STANDARD));
}

#[test]
fn test_get_material_not_found() {
    assert!(get_material("unknown_material").is_none());
}

#[test]
fn test_get_material_or_default() {
    // Upstream default material is FR4_STANDARD.
    let result = get_material_or_default(Some("unknown"), &FR4_STANDARD);
    assert_eq!(*result, FR4_STANDARD);

    let result = get_material_or_default(None, &FR4_STANDARD);
    assert_eq!(*result, FR4_STANDARD);
}

// --- TestStackupLayer ---

#[test]
fn test_copper_layer() {
    let l = StackupLayer {
        thickness_mm: 0.035,
        material: "copper".into(),
        copper_weight_oz: Some(1.0),
        ..StackupLayer::new("F.Cu", LayerType::Copper)
    };
    assert!(l.is_copper());
    assert!(!l.is_dielectric());
    assert!(l.is_signal_layer());
}

#[test]
fn test_dielectric_layer() {
    let l = StackupLayer {
        material: "FR4".into(),
        ..dielectric("prepreg 1", 0.2, 4.5, 0.02)
    };
    assert!(!l.is_copper());
    assert!(l.is_dielectric());
    assert!(!l.is_signal_layer());
}

#[test]
fn test_inner_layer_is_signal() {
    let l = layer("In1.Cu", LayerType::Copper, 0.0175);
    assert!(l.is_signal_layer());
}

// --- TestStackupPresets ---

#[test]
fn test_default_2layer() {
    let stackup = Stackup::default_2layer(1.6);

    assert_eq!(stackup.num_copper_layers(), 2);
    assert_approx!(stackup.board_thickness_mm, 1.6, rel = 0.1);
    assert_eq!(stackup.layers.len(), 3); // F.Cu, core, B.Cu

    assert_eq!(stackup.layers[0].name, "F.Cu");
    assert_eq!(stackup.layers[1].name, "core");
    assert_eq!(stackup.layers[2].name, "B.Cu");

    assert_approx!(stackup.get_copper_thickness("F.Cu"), 0.035, rel = 0.1);
    assert_approx!(stackup.get_copper_thickness("B.Cu"), 0.035, rel = 0.1);
}

#[test]
fn test_default_2layer_custom_thickness() {
    let stackup = Stackup::default_2layer(1.0);
    assert_approx!(stackup.board_thickness_mm, 1.0, rel = 0.1);
}

#[test]
fn test_jlcpcb_4layer() {
    let stackup = Stackup::jlcpcb_4layer();

    assert_eq!(stackup.num_copper_layers(), 4);
    assert_approx!(stackup.board_thickness_mm, 1.6, rel = 0.1);
    assert_eq!(stackup.layers.len(), 7); // 4 copper + 3 dielectric

    let copper_names: Vec<&str> = stackup
        .copper_layers()
        .iter()
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(copper_names, ["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"]);

    let f_cu = stackup.get_layer("F.Cu").expect("F.Cu");
    assert_eq!(f_cu.copper_weight_oz, Some(1.0));

    let in1_cu = stackup.get_layer("In1.Cu").expect("In1.Cu");
    assert_eq!(in1_cu.copper_weight_oz, Some(0.5));

    let prepreg1 = stackup.get_layer("prepreg 1").expect("prepreg 1");
    assert_approx!(prepreg1.epsilon_r, 4.05, rel = 0.1);
}

#[test]
fn test_oshpark_4layer() {
    let stackup = Stackup::oshpark_4layer();

    assert_eq!(stackup.num_copper_layers(), 4);
    assert_approx!(stackup.board_thickness_mm, 1.6, rel = 0.1);
    assert_eq!(stackup.copper_finish, "ENIG");

    // OSH Park uses FR408 which has lower loss
    let prepreg1 = stackup.get_layer("prepreg 1").expect("prepreg 1");
    assert!(prepreg1.loss_tangent < FR4_STANDARD.loss_tangent);
}

#[test]
fn test_default_6layer() {
    let stackup = Stackup::default_6layer();

    assert_eq!(stackup.num_copper_layers(), 6);
    assert_approx!(stackup.board_thickness_mm, 1.6, rel = 0.1);

    let copper_names: Vec<&str> = stackup
        .copper_layers()
        .iter()
        .map(|l| l.name.as_str())
        .collect();
    for name in ["F.Cu", "In1.Cu", "In2.Cu", "In3.Cu", "In4.Cu", "B.Cu"] {
        assert!(copper_names.contains(&name), "{name} missing");
    }
}

// --- TestStackupQueries ---

#[test]
fn test_is_outer_layer() {
    let stackup = Stackup::jlcpcb_4layer();

    assert!(stackup.is_outer_layer("F.Cu"));
    assert!(stackup.is_outer_layer("B.Cu"));
    assert!(!stackup.is_outer_layer("In1.Cu"));
    assert!(!stackup.is_outer_layer("In2.Cu"));
}

#[test]
fn test_get_dielectric_height_outer() {
    let stackup = Stackup::jlcpcb_4layer();
    assert_approx!(h(&stackup, "F.Cu"), 0.2104, rel = 0.1);
}

#[test]
fn test_get_dielectric_height_inner() {
    let stackup = Stackup::jlcpcb_4layer();
    // In1.Cu should return smaller of prepreg (0.21) or core (1.065)
    assert_approx!(h(&stackup, "In1.Cu"), 0.2104, rel = 0.1);
}

#[test]
fn test_get_dielectric_constant_outer() {
    let stackup = Stackup::jlcpcb_4layer();
    assert_approx!(stackup.get_dielectric_constant("F.Cu"), 4.05, rel = 0.1);
}

#[test]
fn test_get_dielectric_constant_inner() {
    let stackup = Stackup::jlcpcb_4layer();
    // In1.Cu is between prepreg (er=4.05) and core (er=4.6); average.
    let expected = (4.05 + 4.6) / 2.0;
    assert_approx!(
        stackup.get_dielectric_constant("In1.Cu"),
        expected,
        rel = 0.1
    );
}

#[test]
fn test_get_copper_thickness() {
    let stackup = Stackup::jlcpcb_4layer();
    assert_approx!(stackup.get_copper_thickness("F.Cu"), 0.035, rel = 0.1);
    assert_approx!(stackup.get_copper_thickness("In1.Cu"), 0.0175, rel = 0.1);
}

#[test]
fn test_get_reference_plane_distance() {
    let stackup = Stackup::jlcpcb_4layer();
    assert_eq!(
        stackup.get_reference_plane_distance("F.Cu").unwrap(),
        h(&stackup, "F.Cu")
    );
}

#[test]
fn test_get_loss_tangent() {
    let stackup = Stackup::jlcpcb_4layer();
    assert_approx!(stackup.get_loss_tangent("F.Cu"), 0.02, rel = 0.1);
}

#[test]
fn test_summary() {
    let summary = Stackup::jlcpcb_4layer().summary();
    assert_eq!(summary.num_copper_layers, 4);
    assert_approx!(summary.board_thickness_mm, 1.6, rel = 0.1);
    assert_eq!(summary.layers.len(), 7);
}

// --- TestStackupFromPCB ---

#[test]
fn test_from_pcb_no_stackup() {
    let stackup = Stackup::load(crate::fixture("routing-diagnostic.kicad_pcb")).unwrap();
    assert_eq!(stackup.num_copper_layers(), 2);
    assert_approx!(stackup.board_thickness_mm, 1.6, rel = 0.1);
}

#[test]
fn test_repr() {
    let repr_str = Stackup::jlcpcb_4layer().to_string();
    assert!(repr_str.contains("4L"), "{repr_str}");
    assert!(repr_str.contains("1.6mm"), "{repr_str}");
}

// --- composite six-layer board (`composite_six_layer_pcb` fixture) ---

fn composite_six_layer_pcb() -> PcbStackupData {
    let root = kct::sexp::parse_file(crate::fixture("physics/composite_six_layer.kicad_pcb"))
        .expect("parse composite fixture");
    PcbStackupData::from_sexp(&root)
}

#[test]
fn test_native_composite_dielectric_preserves_all_strata() {
    let pcb = composite_six_layer_pcb();
    let strata: Vec<&PcbStackupLayer> = pcb
        .setup_stackup
        .as_ref()
        .expect("setup stackup")
        .iter()
        .filter(|l| l.name.starts_with("dielectric 3"))
        .collect();
    let thickness: Vec<f64> = strata.iter().map(|l| l.thickness).collect();
    assert_eq!(thickness, [0.1164, 0.7, 0.1164]);
    let epsilon: Vec<f64> = strata.iter().map(|l| l.epsilon_r).collect();
    assert_eq!(epsilon, [4.16, 4.6, 4.16]);
    let material: Vec<&str> = strata.iter().map(|l| l.material.as_str()).collect();
    assert_eq!(material, ["FR4 2116", "FR4 core", "FR4 2116"]);
    let stackup = Stackup::from_pcb_data(&pcb);
    assert_approx!(stackup.board_thickness_mm, 1.5564);
    assert_eq!(
        stackup
            .get_layer("dielectric 3 (sublayer 2)")
            .expect("sublayer 2")
            .loss_tangent,
        0.018
    );
    // Skipped: upstream also saves the PCB and checks the round-tripped file
    // (strata count, two `addsublayer` atoms). The PCB schema / writer is not
    // part of the physics port, so there is no save path to exercise here.
}

#[test]
fn test_stripline_uses_declared_planes_across_signal_copper() {
    let stackup = Stackup::from_pcb_data(&composite_six_layer_pcb());
    assert!(!stackup.get_layer("In1.Cu").unwrap().is_signal_layer());
    assert!(stackup.get_layer("In2.Cu").unwrap().is_signal_layer());
    assert_pair_approx(
        stackup.get_stripline_geometry("In2.Cu").unwrap(),
        (0.13, 1.078),
    );
    assert_pair_approx(
        stackup.get_stripline_geometry("In3.Cu").unwrap(),
        (1.078, 0.13),
    );
    assert_approx!(h(&stackup, "In2.Cu"), 0.13);
}

#[test]
fn test_legacy_roles_still_sum_composite_dielectric() {
    let mut pcb = composite_six_layer_pcb();
    for l in &mut pcb.layers {
        l.layer_type = "signal".into();
    }
    let stackup = Stackup::from_pcb_data(&pcb);
    assert_pair_approx(
        stackup.get_stripline_geometry("In2.Cu").unwrap(),
        (0.13, 0.9328),
    );
}

#[test]
fn test_explicit_planes_do_not_fall_back_to_signal_when_missing() {
    let mut pcb = composite_six_layer_pcb();
    pcb.layers
        .iter_mut()
        .find(|l| l.number == 4)
        .expect("layer 4")
        .layer_type = "signal".into();
    let stackup = Stackup::from_pcb_data(&pcb);
    let e = stackup
        .get_stripline_geometry("In2.Cu")
        .expect_err("expected ValueError");
    assert!(e.0.contains("No declared reference plane"), "{:?}", e.0);
}

// --- TestOuterInnerCopperOz ---

fn explicit(layers: Vec<StackupLayer>) -> Stackup {
    Stackup {
        layers,
        has_explicit_data: true,
        ..Default::default()
    }
}

fn copper(name: &str, thickness_mm: f64) -> StackupLayer {
    StackupLayer {
        material: "copper".into(),
        ..layer(name, LayerType::Copper, thickness_mm)
    }
}

fn diel(name: &str) -> StackupLayer {
    StackupLayer {
        material: "FR4".into(),
        ..layer(name, LayerType::Dielectric, 0.2)
    }
}

#[test]
fn test_absent_stackup_returns_none() {
    let stackup = Stackup::jlcpcb_4layer(); // preset -> has_explicit_data false
    assert!(!stackup.has_explicit_data);
    assert!(stackup.outer_inner_copper_oz().is_none());
}

#[test]
fn test_two_oz_outer_half_oz_inner_4layer() {
    let stackup = explicit(vec![
        copper("F.Cu", 0.070), // 2 oz
        diel("prepreg"),
        copper("In1.Cu", 0.0175), // 0.5 oz
        diel("core"),
        copper("In2.Cu", 0.0175), // 0.5 oz
        diel("prepreg2"),
        copper("B.Cu", 0.070), // 2 oz
    ]);
    let (outer, inner) = stackup.outer_inner_copper_oz().unwrap();
    assert_approx!(outer.unwrap(), 2.0, abs = 1e-6);
    assert_approx!(inner.unwrap(), 0.5, abs = 1e-6);
}

#[test]
fn test_two_layer_has_no_inner() {
    let stackup = explicit(vec![
        copper("F.Cu", 0.070),
        diel("core"),
        copper("B.Cu", 0.070),
    ]);
    let (outer, inner) = stackup.outer_inner_copper_oz().unwrap();
    assert_approx!(outer.unwrap(), 2.0, abs = 1e-6);
    assert!(inner.is_none());
}

#[test]
fn test_asymmetric_outer_picks_thinner_safer_face() {
    let stackup = explicit(vec![
        copper("F.Cu", 0.070), // 2 oz
        diel("core"),
        copper("B.Cu", 0.035), // 1 oz (thinner -> safer)
    ]);
    let (outer, _inner) = stackup.outer_inner_copper_oz().unwrap();
    assert_approx!(outer.unwrap(), 1.0, abs = 1e-6);
}

#[test]
fn test_uses_035_conversion() {
    let stackup = explicit(vec![
        copper("F.Cu", 0.105), // 3 oz
        diel("core"),
        copper("B.Cu", 0.105),
    ]);
    let (outer, _) = stackup.outer_inner_copper_oz().unwrap();
    assert_approx!(outer.unwrap(), 0.105 / 0.035, abs = 1e-6);
}

// =========================================================================
// test_named_stackup.py (physics-only cases)
// =========================================================================
// Skipped there: ordering-record / manufacturing-export / export-CLI tests
// (`kicad_tools.export`, not physics); the CLI half of
// test_named_cli_and_explicit_pcb_priority and test_cli_parser_accepts_named_preset
// live in physics/cli.rs.

fn check_named_construction(identifier: &str, height: f64, er: f64, core: f64) {
    let stack = Stackup::jlcpcb_named(identifier).unwrap();
    assert_approx!(h(&stack, "F.Cu"), height);
    assert_eq!(stack.layers[1].epsilon_r, er);
    assert_eq!(stack.layers[3].thickness_mm, core);
    assert_eq!(stack.layers[2].thickness_mm, 0.0152);
    let construction = stack.summary().construction.expect("construction");
    assert_eq!(construction.factory_id(), Some(identifier));
    assert_eq!(
        construction.source_url(),
        Some("https://jlcpcb.com/impedance")
    );
    let tl = TransmissionLine::new(stack);
    let width = tl.width_for_impedance(50.0, "F.Cu", "auto").unwrap();
    assert_approx!(
        tl.microstrip(width, "F.Cu", 1.0).unwrap().z0,
        50.0,
        abs = 0.5
    );
}

/// `test_named_construction[JLC04161H-3313-0.0994-4.1-1.265]`
#[test]
fn test_named_construction_3313() {
    check_named_construction("JLC04161H-3313", 0.0994, 4.1, 1.265);
}

/// `test_named_construction[JLC04161H-7628-0.2104-4.4-1.065]`
#[test]
fn test_named_construction_7628() {
    check_named_construction("JLC04161H-7628", 0.2104, 4.4, 1.065);
}

#[test]
fn test_legacy_keeps_numbers_without_factory_identity() {
    let legacy = Stackup::jlcpcb_4layer();
    assert_eq!(legacy.layers[1].thickness_mm, 0.2104);
    assert_eq!(legacy.layers[1].epsilon_r, 4.05);
    assert_eq!(legacy.layers[2].thickness_mm, 0.0175);
    let construction = legacy.summary().construction.expect("construction");
    assert_eq!(construction.factory_id(), None);
    assert_eq!(construction.id(), "jlcpcb-4-legacy");
    assert_eq!(Stackup::jlcpcb_4layer_legacy().summary(), legacy.summary());
    let named = TransmissionLine::new(Stackup::jlcpcb_named("JLC04161H-3313").unwrap())
        .width_for_impedance(50.0, "F.Cu", "auto")
        .unwrap();
    let old = TransmissionLine::new(legacy)
        .width_for_impedance(50.0, "F.Cu", "auto")
        .unwrap();
    assert!(named < old);
}

/// Upstream `authored_pcb`: `PCB.create(layers=4)` (board layer table and
/// `(setup (stackup ...))` as written by `PCB._build_layers_sexp` /
/// `_build_setup_sexp`) with copper/core/prepreg thickness and epsilon_r
/// overwritten from `Stackup.jlcpcb_named(identifier)`.
fn authored_pcb_text(identifier: &str) -> String {
    let stack = Stackup::jlcpcb_named(identifier).unwrap();
    assert_eq!(stack.layers.len(), 7);
    let names = [
        "F.Cu",
        "dielectric 1",
        "In1.Cu",
        "dielectric 2",
        "In2.Cu",
        "dielectric 3",
        "B.Cu",
    ];
    let kinds = [
        "copper", "prepreg", "copper", "core", "copper", "prepreg", "copper",
    ];
    let mut physical = String::new();
    for ((name, kind), l) in names.iter().zip(kinds).zip(&stack.layers) {
        if l.is_dielectric() {
            physical.push_str(&format!(
                "(layer \"{name}\" (type \"{kind}\") (thickness {}) (material \"FR4\") \
                 (epsilon_r {}) (loss_tangent 0.02))\n",
                l.thickness_mm, l.epsilon_r
            ));
        } else {
            physical.push_str(&format!(
                "(layer \"{name}\" (type \"{kind}\") (thickness {}))\n",
                l.thickness_mm
            ));
        }
    }
    format!(
        r#"(kicad_pcb (version 20240108) (generator "kicad-tools")
  (general (thickness 1.6))
  (paper "A4")
  (layers
    (0 "F.Cu" signal) (1 "In1.Cu" signal) (2 "In2.Cu" signal) (31 "B.Cu" signal)
    (32 "B.Adhes" user "B.Adhesive") (33 "F.Adhes" user "F.Adhesive")
    (34 "B.Paste" user) (35 "F.Paste" user)
    (36 "B.SilkS" user "B.Silkscreen") (37 "F.SilkS" user "F.Silkscreen")
    (38 "B.Mask" user) (39 "F.Mask" user)
    (40 "Dwgs.User" user "User.Drawings") (44 "Edge.Cuts" user)
    (46 "B.CrtYd" user "B.Courtyard") (47 "F.CrtYd" user "F.Courtyard")
    (48 "B.Fab" user) (49 "F.Fab" user))
  (setup
    (stackup
      (layer "F.SilkS" (type "Top Silk Screen"))
      (layer "F.Paste" (type "Top Solder Paste"))
      (layer "F.Mask" (type "Top Solder Mask") (thickness 0.01))
{physical}      (layer "B.Mask" (type "Bottom Solder Mask") (thickness 0.01))
      (layer "B.Paste" (type "Bottom Solder Paste"))
      (layer "B.SilkS" (type "Bottom Silk Screen"))))
  (net 0 ""))
"#
    )
}

/// Parsing half of `test_named_cli_and_explicit_pcb_priority`: a board with
/// an explicit stackup authored from the named construction is parsed as
/// explicit data carrying the factory dielectric height. (The CLI preset vs
/// `--pcb` priority half is in physics/cli.rs.)
#[test]
fn test_named_cli_and_explicit_pcb_priority() {
    let root = kct::sexp::parse(&authored_pcb_text("JLC04161H-3313")).unwrap();
    let selected = Stackup::from_pcb(&root);
    assert!(selected.has_explicit_data);
    assert_approx!(h(&selected, "F.Cu"), 0.0994);
}

// =========================================================================
// test_physics_integration.py (physics-only cases)
// =========================================================================
// Skipped: TestRouterPhysicsIntegration (kicad_tools.router.core.Autorouter),
// TestPlacementCrosstalkRiskDataclass (kicad_tools.optim.signal_integrity),
// TestImpedanceGuideDataclass (kicad_tools.reasoning.diagnosis),
// TestNetImpedanceSpec (kicad_tools.validate.rules.impedance) -- not physics.

#[test]
fn test_stackup_to_transmission_line() {
    let tl = TransmissionLine::new(Stackup::jlcpcb_4layer());
    let result = tl.microstrip(0.2, "F.Cu", 1.0).unwrap();
    assert!(30.0 < result.z0 && result.z0 < 100.0);
}

#[test]
fn test_stackup_to_width_calculation() {
    let tl = TransmissionLine::new(Stackup::jlcpcb_4layer());
    let width = tl.width_for_impedance(50.0, "F.Cu", "auto").unwrap();
    let result = tl.microstrip(width, "F.Cu", 1.0).unwrap();
    assert!((result.z0 - 50.0).abs() < 5.0);
}

#[test]
fn test_different_stackups_different_widths() {
    let tl_jlc = TransmissionLine::new(Stackup::jlcpcb_4layer());
    let tl_osh = TransmissionLine::new(Stackup::oshpark_4layer());
    let width_jlc = tl_jlc.width_for_impedance(50.0, "F.Cu", "auto").unwrap();
    let width_osh = tl_osh.width_for_impedance(50.0, "F.Cu", "auto").unwrap();
    assert_ne!(width_jlc, width_osh);
}

#[test]
fn test_microstrip_vs_stripline_impedance() {
    let tl = TransmissionLine::new(Stackup::jlcpcb_4layer());
    let width = 0.15;
    let z0_outer = tl.microstrip(width, "F.Cu", 1.0).unwrap().z0;
    let z0_inner = tl.stripline(width, "In1.Cu", 1.0).unwrap().z0;
    assert!(20.0 < z0_outer && z0_outer < 150.0);
    assert!(20.0 < z0_inner && z0_inner < 150.0);
}

#[test]
fn test_transmission_line_attributes() {
    let tl = TransmissionLine::new(Stackup::jlcpcb_4layer());
    let result = tl.microstrip(0.2, "F.Cu", 1.0).unwrap();
    // Attribute existence (z0, epsilon_eff, loss_db_per_m) is enforced by the type.
    assert!(result.z0 > 0.0);
    assert!(result.epsilon_eff > 1.0);
    assert!(result.loss_db_per_m >= 0.0);
}
