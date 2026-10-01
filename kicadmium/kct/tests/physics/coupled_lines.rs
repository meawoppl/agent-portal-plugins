//! Port of upstream `tests/test_coupled_lines.py`.

use kct::physics::{CoupledLines, DifferentialPairResult, Stackup, SPEED_OF_LIGHT};

fn jlc() -> CoupledLines {
    CoupledLines::new(Stackup::jlcpcb_4layer())
}

fn assert_err_contains<T: std::fmt::Debug>(r: kct::physics::PhysResult<T>, needle: &str) {
    let e = r.expect_err("expected ValueError");
    assert!(
        e.0.contains(needle),
        "{:?} does not contain {needle:?}",
        e.0
    );
}

// --- TestDifferentialPairResult ---

#[test]
fn test_basic_result() {
    let result = DifferentialPairResult {
        zdiff: 90.0,
        zcommon: 25.0,
        z0_even: 50.0,
        z0_odd: 45.0,
        coupling_coefficient: 0.1,
        epsilon_eff_even: 3.5,
        epsilon_eff_odd: 3.3,
    };
    assert_eq!(result.zdiff, 90.0);
    assert_eq!(result.zcommon, 25.0);
    assert_eq!(result.coupling_coefficient, 0.1);
}

#[test]
fn test_phase_velocity_even() {
    let result = DifferentialPairResult {
        zdiff: 90.0,
        zcommon: 25.0,
        z0_even: 50.0,
        z0_odd: 45.0,
        coupling_coefficient: 0.1,
        epsilon_eff_even: 4.0,
        epsilon_eff_odd: 3.5,
    };
    let expected = SPEED_OF_LIGHT / 4.0_f64.sqrt();
    assert_approx!(result.phase_velocity_even(), expected, rel = 0.01);
}

#[test]
fn test_phase_velocity_odd() {
    let result = DifferentialPairResult {
        zdiff: 90.0,
        zcommon: 25.0,
        z0_even: 50.0,
        z0_odd: 45.0,
        coupling_coefficient: 0.1,
        epsilon_eff_even: 4.0,
        epsilon_eff_odd: 3.5,
    };
    let expected = SPEED_OF_LIGHT / 3.5_f64.sqrt();
    assert_approx!(result.phase_velocity_odd(), expected, rel = 0.01);
}

#[test]
fn test_repr() {
    let result = DifferentialPairResult {
        zdiff: 90.123,
        zcommon: 25.456,
        z0_even: 50.0,
        z0_odd: 45.0,
        coupling_coefficient: 0.123,
        epsilon_eff_even: 4.0,
        epsilon_eff_odd: 3.5,
    };
    let repr_str = result.to_string();
    assert!(repr_str.contains("90.1"));
    assert!(repr_str.contains("25.5"));
    assert!(repr_str.contains("0.123"));
}

// --- TestEdgeCoupledMicrostrip ---

#[test]
fn test_edge_coupled_microstrip_basic() {
    let cl = jlc();
    let result = cl.edge_coupled_microstrip(0.15, 0.15, "F.Cu").unwrap();
    assert!(result.zdiff > 0.0);
    assert!(result.zcommon > 0.0);
    assert!(result.z0_even > result.z0_odd);
    assert!(0.0 < result.coupling_coefficient && result.coupling_coefficient < 1.0);
}

#[test]
fn test_differential_pair_geometry() {
    let cl = jlc();
    let result = cl.edge_coupled_microstrip(0.127, 0.127, "F.Cu").unwrap();
    assert!(result.zdiff > 0.0);
    assert!(100.0 < result.zdiff && result.zdiff < 180.0);
}

#[test]
fn test_90ohm_achievable_geometry() {
    let cl = jlc();
    let gap = cl
        .gap_for_differential_impedance(90.0, 0.35, "F.Cu", "edge_microstrip")
        .unwrap();
    let result = cl.edge_coupled_microstrip(0.35, gap, "F.Cu").unwrap();
    assert_approx!(result.zdiff, 90.0, rel = 0.05);
}

#[test]
fn test_coupling_increases_with_tighter_gap() {
    let cl = jlc();
    let tight = cl.edge_coupled_microstrip(0.15, 0.1, "F.Cu").unwrap();
    let loose = cl.edge_coupled_microstrip(0.15, 0.3, "F.Cu").unwrap();
    assert!(tight.coupling_coefficient > loose.coupling_coefficient);
}

#[test]
fn test_zdiff_increases_with_looser_gap() {
    let cl = jlc();
    let tight = cl.edge_coupled_microstrip(0.15, 0.1, "F.Cu").unwrap();
    let loose = cl.edge_coupled_microstrip(0.15, 0.3, "F.Cu").unwrap();
    assert!(loose.zdiff > tight.zdiff);
}

#[test]
fn test_zdiff_approx_twice_z0_odd() {
    let cl = jlc();
    let result = cl.edge_coupled_microstrip(0.15, 0.15, "F.Cu").unwrap();
    assert_approx!(result.zdiff, 2.0 * result.z0_odd, rel = 0.001);
}

#[test]
fn test_zcommon_approx_half_z0_even() {
    let cl = jlc();
    let result = cl.edge_coupled_microstrip(0.15, 0.15, "F.Cu").unwrap();
    assert_approx!(result.zcommon, result.z0_even / 2.0, rel = 0.001);
}

#[test]
fn test_coupling_coefficient_formula() {
    let cl = jlc();
    let result = cl.edge_coupled_microstrip(0.15, 0.15, "F.Cu").unwrap();
    let expected_k = (result.z0_even - result.z0_odd) / (result.z0_even + result.z0_odd);
    assert_approx!(result.coupling_coefficient, expected_k, rel = 0.001);
}

#[test]
fn test_bottom_layer_similar_to_top() {
    let cl = jlc();
    let top = cl.edge_coupled_microstrip(0.15, 0.15, "F.Cu").unwrap();
    let bottom = cl.edge_coupled_microstrip(0.15, 0.15, "B.Cu").unwrap();
    assert_approx!(top.zdiff, bottom.zdiff, rel = 0.15);
}

#[test]
fn test_invalid_width_raises_error() {
    let cl = jlc();
    assert_err_contains(cl.edge_coupled_microstrip(0.0, 0.15, "F.Cu"), "positive");
    assert_err_contains(cl.edge_coupled_microstrip(-0.1, 0.15, "F.Cu"), "positive");
}

#[test]
fn test_invalid_gap_raises_error() {
    let cl = jlc();
    assert_err_contains(cl.edge_coupled_microstrip(0.15, 0.0, "F.Cu"), "positive");
    assert_err_contains(cl.edge_coupled_microstrip(0.15, -0.1, "F.Cu"), "positive");
}

// --- TestEdgeCoupledStripline ---

#[test]
fn test_edge_coupled_stripline_basic() {
    let cl = jlc();
    let result = cl.edge_coupled_stripline(0.15, 0.15, "In1.Cu").unwrap();
    assert!(result.zdiff > 0.0);
    assert!(result.zcommon > 0.0);
    assert!(result.z0_even > result.z0_odd);
}

#[test]
fn test_stripline_eps_eff_equals_er() {
    let stackup = Stackup::jlcpcb_4layer();
    let cl = CoupledLines::new(stackup.clone());
    let result = cl.edge_coupled_stripline(0.15, 0.15, "In1.Cu").unwrap();
    let er = stackup.get_dielectric_constant("In1.Cu");
    assert_approx!(result.epsilon_eff_even, er, rel = 0.01);
    assert_approx!(result.epsilon_eff_odd, er, rel = 0.01);
}

#[test]
fn test_stripline_coupling_effect() {
    let cl = jlc();
    let tight = cl.edge_coupled_stripline(0.15, 0.1, "In1.Cu").unwrap();
    let loose = cl.edge_coupled_stripline(0.15, 0.3, "In1.Cu").unwrap();
    assert!(tight.coupling_coefficient > loose.coupling_coefficient);
    assert!(loose.zdiff > tight.zdiff);
}

#[test]
fn test_stripline_invalid_parameters() {
    let cl = jlc();
    assert_err_contains(cl.edge_coupled_stripline(0.0, 0.15, "In1.Cu"), "positive");
    assert_err_contains(cl.edge_coupled_stripline(0.15, 0.0, "In1.Cu"), "positive");
}

// --- TestBroadsideCoupledStripline ---

#[test]
fn test_broadside_coupled_basic() {
    let cl = jlc();
    let result = cl
        .broadside_coupled_stripline(0.15, "In1.Cu", "In2.Cu")
        .unwrap();
    assert!(result.zdiff > 0.0);
    assert!(result.zcommon > 0.0);
    assert!(result.z0_even > result.z0_odd);
}

#[test]
fn test_broadside_stronger_coupling() {
    let stackup = Stackup::jlcpcb_4layer();
    let cl = CoupledLines::new(stackup.clone());
    let (h1, _) = stackup.get_stripline_geometry("In1.Cu").unwrap();
    let edge = cl.edge_coupled_stripline(0.15, h1, "In1.Cu").unwrap();
    let broadside = cl
        .broadside_coupled_stripline(0.15, "In1.Cu", "In2.Cu")
        .unwrap();
    assert!(0.0 < edge.coupling_coefficient && edge.coupling_coefficient < 1.0);
    assert!(0.0 < broadside.coupling_coefficient && broadside.coupling_coefficient < 1.0);
}

#[test]
fn test_broadside_invalid_width() {
    let cl = jlc();
    assert_err_contains(
        cl.broadside_coupled_stripline(0.0, "In1.Cu", "In2.Cu"),
        "positive",
    );
}

// --- TestGapForDifferentialImpedance ---

#[test]
fn test_gap_for_90ohm() {
    let cl = jlc();
    let gap = cl
        .gap_for_differential_impedance(90.0, 0.35, "F.Cu", "edge_microstrip")
        .unwrap();
    let result = cl.edge_coupled_microstrip(0.35, gap, "F.Cu").unwrap();
    assert_approx!(result.zdiff, 90.0, rel = 0.05);
}

#[test]
fn test_gap_for_100ohm() {
    let cl = jlc();
    let gap = cl
        .gap_for_differential_impedance(100.0, 0.3, "F.Cu", "edge_microstrip")
        .unwrap();
    let result = cl.edge_coupled_microstrip(0.3, gap, "F.Cu").unwrap();
    assert_approx!(result.zdiff, 100.0, rel = 0.05);
}

#[test]
fn test_gap_for_stripline() {
    let cl = jlc();
    let gap = cl
        .gap_for_differential_impedance(135.0, 0.15, "In1.Cu", "edge_stripline")
        .unwrap();
    let result = cl.edge_coupled_stripline(0.15, gap, "In1.Cu").unwrap();
    assert_approx!(result.zdiff, 135.0, rel = 0.10);
}

#[test]
fn test_gap_auto_mode() {
    let cl = jlc();
    let gap_outer = cl
        .gap_for_differential_impedance(100.0, 0.3, "F.Cu", "auto")
        .unwrap();
    let result_outer = cl.edge_coupled_microstrip(0.3, gap_outer, "F.Cu").unwrap();
    assert_approx!(result_outer.zdiff, 100.0, rel = 0.05);

    let gap_inner = cl
        .gap_for_differential_impedance(140.0, 0.15, "In1.Cu", "auto")
        .unwrap();
    let result_inner = cl
        .edge_coupled_stripline(0.15, gap_inner, "In1.Cu")
        .unwrap();
    assert_approx!(result_inner.zdiff, 140.0, rel = 0.10);
}

#[test]
fn test_higher_zdiff_requires_wider_gap() {
    let cl = jlc();
    let gap_100 = cl
        .gap_for_differential_impedance(100.0, 0.3, "F.Cu", "edge_microstrip")
        .unwrap();
    let gap_110 = cl
        .gap_for_differential_impedance(110.0, 0.3, "F.Cu", "edge_microstrip")
        .unwrap();
    assert!(gap_110 > gap_100);
}

#[test]
fn test_invalid_zdiff_target() {
    let cl = jlc();
    assert_err_contains(
        cl.gap_for_differential_impedance(0.0, 0.127, "F.Cu", "edge_microstrip"),
        "positive",
    );
    assert_err_contains(
        cl.gap_for_differential_impedance(-90.0, 0.127, "F.Cu", "edge_microstrip"),
        "positive",
    );
}

#[test]
fn test_invalid_width() {
    let cl = jlc();
    assert_err_contains(
        cl.gap_for_differential_impedance(90.0, 0.0, "F.Cu", "edge_microstrip"),
        "positive",
    );
}

#[test]
fn test_invalid_mode() {
    let cl = jlc();
    assert_err_contains(
        cl.gap_for_differential_impedance(90.0, 0.127, "F.Cu", "invalid"),
        "Invalid mode",
    );
}

// --- TestStackupIntegration ---

#[test]
fn test_2layer_stackup() {
    let cl = CoupledLines::new(Stackup::default_2layer(1.6));
    let result = cl.edge_coupled_microstrip(0.2, 0.2, "F.Cu").unwrap();
    assert!(result.zdiff > 0.0);
    let gap = cl
        .gap_for_differential_impedance(90.0, 0.2, "F.Cu", "edge_microstrip")
        .unwrap();
    assert!(gap > 0.0);
}

#[test]
fn test_6layer_stackup() {
    let cl = CoupledLines::new(Stackup::default_6layer());
    let result_outer = cl.edge_coupled_microstrip(0.15, 0.15, "F.Cu").unwrap();
    assert!(result_outer.zdiff > 0.0);
    let result_inner = cl.edge_coupled_stripline(0.12, 0.12, "In2.Cu").unwrap();
    assert!(result_inner.zdiff > 0.0);
}

#[test]
fn test_oshpark_4layer() {
    let cl = CoupledLines::new(Stackup::oshpark_4layer());
    let result = cl.edge_coupled_microstrip(0.15, 0.15, "F.Cu").unwrap();
    assert!(result.zdiff > 0.0);
    assert!(0.0 < result.coupling_coefficient && result.coupling_coefficient < 1.0);
}

// --- TestAccuracyValidation ---

#[test]
fn test_microstrip_zdiff_reasonable_range() {
    let cl = jlc();
    for width in [0.1, 0.15, 0.2] {
        for gap in [0.1, 0.2, 0.3] {
            let result = cl.edge_coupled_microstrip(width, gap, "F.Cu").unwrap();
            assert!(40.0 < result.zdiff && result.zdiff < 200.0);
        }
    }
}

#[test]
fn test_coupling_coefficient_range() {
    let cl = jlc();
    for gap in [0.1, 0.15, 0.2, 0.3] {
        let result = cl.edge_coupled_microstrip(0.15, gap, "F.Cu").unwrap();
        assert!(0.0 < result.coupling_coefficient && result.coupling_coefficient < 1.0);
        assert!(0.01 < result.coupling_coefficient && result.coupling_coefficient < 0.5);
    }
}

#[test]
fn test_z0_even_greater_than_z0_odd() {
    let cl = jlc();
    for gap in [0.05, 0.1, 0.2, 0.5] {
        let result = cl.edge_coupled_microstrip(0.15, gap, "F.Cu").unwrap();
        assert!(result.z0_even > result.z0_odd, "Failed at gap={gap}");
    }
}

#[test]
fn test_inverse_calculation_consistency() {
    let cl = jlc();
    for target_zdiff in [85.0, 90.0, 95.0, 100.0] {
        let gap = cl
            .gap_for_differential_impedance(target_zdiff, 0.35, "F.Cu", "edge_microstrip")
            .unwrap();
        let result = cl.edge_coupled_microstrip(0.35, gap, "F.Cu").unwrap();
        assert!(
            crate::approx(result.zdiff, target_zdiff, Some(0.05), None),
            "Target {target_zdiff}Ω, got {:.1}Ω at gap={gap:.3}mm",
            result.zdiff
        );
    }
}
