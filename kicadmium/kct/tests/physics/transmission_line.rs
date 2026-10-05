//! Port of upstream `tests/test_transmission_line.py`.

use kct::physics::{ImpedanceResult, Stackup, TransmissionLine, SPEED_OF_LIGHT};

fn jlc_stackup() -> Stackup {
    Stackup::jlcpcb_4layer()
}

fn jlc() -> TransmissionLine {
    TransmissionLine::new(jlc_stackup())
}

fn assert_err_contains<T: std::fmt::Debug>(r: kct::physics::PhysResult<T>, needle: &str) {
    let e = r.expect_err("expected ValueError");
    assert!(
        e.0.contains(needle),
        "{:?} does not contain {needle:?}",
        e.0
    );
}

// --- TestImpedanceResult ---

#[test]
fn test_propagation_delay_ps_per_mm() {
    let eps_eff = 3.0_f64;
    let v_p = SPEED_OF_LIGHT / eps_eff.sqrt();
    let result = ImpedanceResult {
        z0: 50.0,
        epsilon_eff: eps_eff,
        loss_db_per_m: 0.5,
        phase_velocity: v_p,
    };
    let expected_delay = 1e12 * 0.001 / v_p;
    assert_approx!(
        result.propagation_delay_ps_per_mm(),
        expected_delay,
        rel = 0.01
    );
}

#[test]
fn test_propagation_delay_ns_per_inch() {
    let eps_eff = 3.0_f64;
    let v_p = SPEED_OF_LIGHT / eps_eff.sqrt();
    let result = ImpedanceResult {
        z0: 50.0,
        epsilon_eff: eps_eff,
        loss_db_per_m: 0.5,
        phase_velocity: v_p,
    };
    let expected = result.propagation_delay_ps_per_mm() * 25.4 / 1000.0;
    assert_approx!(result.propagation_delay_ns_per_inch(), expected, rel = 0.01);
}

#[test]
fn test_repr() {
    let result = ImpedanceResult {
        z0: 50.123,
        epsilon_eff: 3.456,
        loss_db_per_m: 0.789,
        phase_velocity: 1.5e8,
    };
    let repr_str = result.to_string();
    assert!(repr_str.contains("50.12"));
    assert!(repr_str.contains("3.456"));
}

// --- TestMicrostripImpedance ---

#[test]
fn test_microstrip_impedance_jlcpcb_4layer() {
    let result = jlc().microstrip(0.2, "F.Cu", 1.0).unwrap();
    assert!(60.0 < result.z0 && result.z0 < 75.0);
}

#[test]
fn test_microstrip_narrow_high_impedance() {
    let tl = jlc();
    let result_narrow = tl.microstrip(0.1, "F.Cu", 1.0).unwrap();
    let result_wide = tl.microstrip(0.3, "F.Cu", 1.0).unwrap();
    assert!(result_narrow.z0 > result_wide.z0);
}

#[test]
fn test_microstrip_effective_epsilon() {
    let stackup = jlc_stackup();
    let tl = TransmissionLine::new(stackup.clone());
    let result = tl.microstrip(0.2, "F.Cu", 1.0).unwrap();
    let er = stackup.get_dielectric_constant("F.Cu");
    assert!(1.0 < result.epsilon_eff && result.epsilon_eff < er);
    assert!(2.0 < result.epsilon_eff && result.epsilon_eff < 4.5);
}

#[test]
fn test_microstrip_phase_velocity() {
    let result = jlc().microstrip(0.2, "F.Cu", 1.0).unwrap();
    let expected_v = SPEED_OF_LIGHT / result.epsilon_eff.sqrt();
    assert_approx!(result.phase_velocity, expected_v, rel = 0.01);
    assert!(result.phase_velocity < SPEED_OF_LIGHT);
}

#[test]
fn test_microstrip_loss_positive() {
    let result = jlc().microstrip(0.2, "F.Cu", 1.0).unwrap();
    assert!(result.loss_db_per_m > 0.0);
    assert!(1.0 < result.loss_db_per_m && result.loss_db_per_m < 50.0);
}

#[test]
fn test_microstrip_loss_increases_with_frequency() {
    let tl = jlc();
    let loss_1ghz = tl.microstrip(0.2, "F.Cu", 1.0).unwrap().loss_db_per_m;
    let loss_5ghz = tl.microstrip(0.2, "F.Cu", 5.0).unwrap().loss_db_per_m;
    assert!(loss_5ghz > loss_1ghz);
}

#[test]
fn test_microstrip_bottom_layer() {
    let tl = jlc();
    let result_top = tl.microstrip(0.2, "F.Cu", 1.0).unwrap();
    let result_bottom = tl.microstrip(0.2, "B.Cu", 1.0).unwrap();
    assert_approx!(result_bottom.z0, result_top.z0, rel = 0.10);
}

#[test]
fn test_microstrip_invalid_width() {
    let tl = jlc();
    assert_err_contains(tl.microstrip(0.0, "F.Cu", 1.0), "positive");
    assert_err_contains(tl.microstrip(-0.1, "F.Cu", 1.0), "positive");
}

// --- TestStriplineImpedance ---

#[test]
fn test_stripline_inner_layer() {
    let result = jlc().stripline(0.15, "In1.Cu", 1.0).unwrap();
    assert!(60.0 < result.z0 && result.z0 < 100.0);
}

#[test]
fn test_stripline_eps_eff_equals_er() {
    let stackup = jlc_stackup();
    let tl = TransmissionLine::new(stackup.clone());
    let result = tl.stripline(0.15, "In1.Cu", 1.0).unwrap();
    let er = stackup.get_dielectric_constant("In1.Cu");
    assert_approx!(result.epsilon_eff, er, rel = 0.01);
}

#[test]
fn test_stripline_slower_than_microstrip() {
    let tl = jlc();
    let microstrip_result = tl.microstrip(0.2, "F.Cu", 1.0).unwrap();
    let stripline_result = tl.stripline(0.2, "In1.Cu", 1.0).unwrap();
    assert!(stripline_result.phase_velocity < microstrip_result.phase_velocity);
}

#[test]
fn test_stripline_narrow_high_impedance() {
    let tl = jlc();
    let result_narrow = tl.stripline(0.1, "In1.Cu", 1.0).unwrap();
    let result_wide = tl.stripline(0.3, "In1.Cu", 1.0).unwrap();
    assert!(result_narrow.z0 > result_wide.z0);
}

#[test]
fn test_stripline_symmetric_vs_asymmetric() {
    let result = jlc().stripline(0.15, "In1.Cu", 1.0).unwrap();
    assert!(60.0 < result.z0 && result.z0 < 100.0);
}

// --- TestWidthForImpedance ---

#[test]
fn test_width_for_50ohm_microstrip() {
    let tl = jlc();
    let width = tl.width_for_impedance(50.0, "F.Cu", "auto").unwrap();
    let result = tl.microstrip(width, "F.Cu", 1.0).unwrap();
    assert_approx!(result.z0, 50.0, rel = 0.02);
}

#[test]
fn test_width_for_75ohm_microstrip() {
    let tl = jlc();
    let width = tl.width_for_impedance(75.0, "F.Cu", "auto").unwrap();
    let result = tl.microstrip(width, "F.Cu", 1.0).unwrap();
    assert_approx!(result.z0, 75.0, rel = 0.02);
}

#[test]
fn test_width_for_50ohm_stripline() {
    let tl = jlc();
    let width = tl.width_for_impedance(50.0, "In1.Cu", "stripline").unwrap();
    let result = tl.stripline(width, "In1.Cu", 1.0).unwrap();
    assert_approx!(result.z0, 50.0, rel = 0.05);
}

#[test]
fn test_width_for_impedance_auto_mode() {
    let tl = jlc();
    let width_outer = tl.width_for_impedance(50.0, "F.Cu", "auto").unwrap();
    let result_outer = tl.microstrip(width_outer, "F.Cu", 1.0).unwrap();
    assert_approx!(result_outer.z0, 50.0, rel = 0.05);

    let width_inner = tl.width_for_impedance(50.0, "In1.Cu", "auto").unwrap();
    let result_inner = tl.stripline(width_inner, "In1.Cu", 1.0).unwrap();
    assert_approx!(result_inner.z0, 50.0, rel = 0.05);
}

#[test]
fn test_width_for_impedance_higher_z0_narrower() {
    let tl = jlc();
    let width_50 = tl.width_for_impedance(50.0, "F.Cu", "auto").unwrap();
    let width_75 = tl.width_for_impedance(75.0, "F.Cu", "auto").unwrap();
    assert!(width_75 < width_50);
}

#[test]
fn test_width_for_impedance_invalid_target() {
    let tl = jlc();
    assert_err_contains(tl.width_for_impedance(0.0, "F.Cu", "auto"), "positive");
    assert_err_contains(tl.width_for_impedance(-50.0, "F.Cu", "auto"), "positive");
}

#[test]
fn test_width_for_impedance_invalid_mode() {
    let tl = jlc();
    assert_err_contains(
        tl.width_for_impedance(50.0, "F.Cu", "invalid"),
        "Invalid mode",
    );
}

// --- TestDifferentialMicrostrip ---

#[test]
fn test_differential_impedance() {
    let (single, z_diff) = jlc()
        .differential_microstrip(0.2, 0.2, "F.Cu", 1.0)
        .unwrap();
    assert!(1.5 * single.z0 < z_diff && z_diff < 2.2 * single.z0);
}

#[test]
fn test_differential_spacing_effect() {
    let tl = jlc();
    let (_, z_diff_tight) = tl.differential_microstrip(0.2, 0.1, "F.Cu", 1.0).unwrap();
    let (_, z_diff_loose) = tl.differential_microstrip(0.2, 0.5, "F.Cu", 1.0).unwrap();
    assert!(z_diff_loose > z_diff_tight);
}

// --- TestStackupIntegration ---

#[test]
fn test_2layer_stackup() {
    let tl = TransmissionLine::new(Stackup::default_2layer(1.6));
    let result = tl.microstrip(0.3, "F.Cu", 1.0).unwrap();
    assert!(result.z0 > 0.0);
    let width = tl.width_for_impedance(50.0, "F.Cu", "auto").unwrap();
    assert!(width > 0.2);
}

#[test]
fn test_6layer_stackup() {
    let tl = TransmissionLine::new(Stackup::default_6layer());
    let result_outer = tl.microstrip(0.2, "F.Cu", 1.0).unwrap();
    assert!(result_outer.z0 > 0.0);
    let result_inner = tl.stripline(0.15, "In2.Cu", 1.0).unwrap();
    assert!(result_inner.z0 > 0.0);
}

#[test]
fn test_oshpark_4layer() {
    let tl = TransmissionLine::new(Stackup::oshpark_4layer());
    let result = tl.microstrip(0.2, "F.Cu", 1.0).unwrap();
    assert!(40.0 < result.z0 && result.z0 < 70.0);
}

// --- TestAccuracyValidation ---

#[test]
fn test_microstrip_50ohm_width() {
    let tl = jlc();
    let width = tl.width_for_impedance(50.0, "F.Cu", "auto").unwrap();
    let result = tl.microstrip(width, "F.Cu", 1.0).unwrap();
    assert_approx!(result.z0, 50.0, rel = 0.02);
}

#[test]
fn test_effective_epsilon_reasonable_range() {
    let tl = jlc();
    for width in [0.1, 0.2, 0.3, 0.5] {
        let result = tl.microstrip(width, "F.Cu", 1.0).unwrap();
        assert!(2.0 < result.epsilon_eff && result.epsilon_eff < 4.5);
    }
}

#[test]
fn test_propagation_delay_typical_range() {
    let result = jlc().microstrip(0.2, "F.Cu", 1.0).unwrap();
    let delay_ps_mm = result.propagation_delay_ps_per_mm();
    assert!(5.0 < delay_ps_mm && delay_ps_mm < 8.0);
    let delay_ns_inch = result.propagation_delay_ns_per_inch();
    assert!(0.12 < delay_ns_inch && delay_ns_inch < 0.20);
}

// --- TestCPWGImpedance ---

#[test]
fn test_cpwg_basic_impedance() {
    let result = jlc().cpwg(0.25, 0.15, "F.Cu", 1.0).unwrap();
    assert!(40.0 < result.z0 && result.z0 < 120.0);
}

#[test]
fn test_cpwg_50ohm_geometry() {
    let tl = jlc();
    let (width, gap) = tl
        .cpwg_geometry_for_impedance(50.0, "F.Cu", Some(0.3), None)
        .unwrap();
    let result = tl.cpwg(width, gap, "F.Cu", 1.0).unwrap();
    assert_approx!(result.z0, 50.0, rel = 0.05);
}

#[test]
fn test_cpwg_narrow_gap_lower_impedance() {
    let tl = jlc();
    let result_narrow = tl.cpwg(0.2, 0.1, "F.Cu", 1.0).unwrap();
    let result_wide = tl.cpwg(0.2, 0.3, "F.Cu", 1.0).unwrap();
    assert!(result_wide.z0 > result_narrow.z0);
}

#[test]
fn test_cpwg_wide_trace_lower_impedance() {
    let tl = jlc();
    let result_narrow = tl.cpwg(0.1, 0.15, "F.Cu", 1.0).unwrap();
    let result_wide = tl.cpwg(0.4, 0.15, "F.Cu", 1.0).unwrap();
    assert!(result_wide.z0 < result_narrow.z0);
}

#[test]
fn test_cpwg_effective_epsilon() {
    let stackup = jlc_stackup();
    let tl = TransmissionLine::new(stackup.clone());
    let result = tl.cpwg(0.25, 0.15, "F.Cu", 1.0).unwrap();
    let er = stackup.get_dielectric_constant("F.Cu");
    assert!(1.0 < result.epsilon_eff && result.epsilon_eff < er);
}

#[test]
fn test_cpwg_phase_velocity() {
    let result = jlc().cpwg(0.25, 0.15, "F.Cu", 1.0).unwrap();
    let expected_v = SPEED_OF_LIGHT / result.epsilon_eff.sqrt();
    assert_approx!(result.phase_velocity, expected_v, rel = 0.01);
    assert!(result.phase_velocity < SPEED_OF_LIGHT);
}

#[test]
fn test_cpwg_loss_positive() {
    let result = jlc().cpwg(0.25, 0.15, "F.Cu", 1.0).unwrap();
    assert!(result.loss_db_per_m > 0.0);
    assert!(0.5 < result.loss_db_per_m && result.loss_db_per_m < 50.0);
}

#[test]
fn test_cpwg_loss_increases_with_frequency() {
    let tl = jlc();
    let loss_1ghz = tl.cpwg(0.25, 0.15, "F.Cu", 1.0).unwrap().loss_db_per_m;
    let loss_5ghz = tl.cpwg(0.25, 0.15, "F.Cu", 5.0).unwrap().loss_db_per_m;
    assert!(loss_5ghz > loss_1ghz);
}

#[test]
fn test_cpwg_invalid_width() {
    let tl = jlc();
    assert_err_contains(tl.cpwg(0.0, 0.15, "F.Cu", 1.0), "positive");
    assert_err_contains(tl.cpwg(-0.1, 0.15, "F.Cu", 1.0), "positive");
}

#[test]
fn test_cpwg_invalid_gap() {
    let tl = jlc();
    assert_err_contains(tl.cpwg(0.25, 0.0, "F.Cu", 1.0), "positive");
    assert_err_contains(tl.cpwg(0.25, -0.1, "F.Cu", 1.0), "positive");
}

// --- TestCPWGGeometryForImpedance ---

#[test]
fn test_cpwg_geometry_fixed_width() {
    let tl = jlc();
    let (width, gap) = tl
        .cpwg_geometry_for_impedance(50.0, "F.Cu", Some(0.25), None)
        .unwrap();
    assert_eq!(width, 0.25);
    let result = tl.cpwg(width, gap, "F.Cu", 1.0).unwrap();
    assert_approx!(result.z0, 50.0, rel = 0.03);
}

#[test]
fn test_cpwg_geometry_fixed_gap() {
    let tl = jlc();
    let (width, gap) = tl
        .cpwg_geometry_for_impedance(60.0, "F.Cu", None, Some(0.15))
        .unwrap();
    assert_eq!(gap, 0.15);
    let result = tl.cpwg(width, gap, "F.Cu", 1.0).unwrap();
    assert_approx!(result.z0, 60.0, rel = 0.05);
}

#[test]
fn test_cpwg_geometry_balanced() {
    let tl = jlc();
    let (width, gap) = tl
        .cpwg_geometry_for_impedance(50.0, "F.Cu", None, None)
        .unwrap();
    assert!(width > 0.0);
    assert!(gap > 0.0);
    let result = tl.cpwg(width, gap, "F.Cu", 1.0).unwrap();
    assert_approx!(result.z0, 50.0, rel = 0.05);
}

#[test]
fn test_cpwg_geometry_high_impedance() {
    let tl = jlc();
    let (width, gap) = tl
        .cpwg_geometry_for_impedance(75.0, "F.Cu", Some(0.2), None)
        .unwrap();
    let result = tl.cpwg(width, gap, "F.Cu", 1.0).unwrap();
    assert_approx!(result.z0, 75.0, rel = 0.05);
}

#[test]
fn test_cpwg_geometry_low_impedance() {
    let tl = jlc();
    let (width, gap) = tl
        .cpwg_geometry_for_impedance(40.0, "F.Cu", Some(0.4), None)
        .unwrap();
    let result = tl.cpwg(width, gap, "F.Cu", 1.0).unwrap();
    assert_approx!(result.z0, 40.0, rel = 0.10);
}

#[test]
fn test_cpwg_geometry_invalid_target() {
    let tl = jlc();
    assert_err_contains(
        tl.cpwg_geometry_for_impedance(0.0, "F.Cu", None, None),
        "positive",
    );
    assert_err_contains(
        tl.cpwg_geometry_for_impedance(-50.0, "F.Cu", None, None),
        "positive",
    );
}

#[test]
fn test_cpwg_geometry_both_specified_error() {
    let tl = jlc();
    assert_err_contains(
        tl.cpwg_geometry_for_impedance(50.0, "F.Cu", Some(0.25), Some(0.15)),
        "either width_mm or gap_mm",
    );
}

// --- TestCPWGvsOtherModes ---

#[test]
fn test_cpwg_vs_microstrip_impedance_range() {
    let tl = jlc();
    let microstrip_50 = tl.width_for_impedance(50.0, "F.Cu", "microstrip").unwrap();
    let microstrip_result = tl.microstrip(microstrip_50, "F.Cu", 1.0).unwrap();

    let (cpwg_w, cpwg_g) = tl
        .cpwg_geometry_for_impedance(50.0, "F.Cu", None, None)
        .unwrap();
    let cpwg_result = tl.cpwg(cpwg_w, cpwg_g, "F.Cu", 1.0).unwrap();

    assert_approx!(microstrip_result.z0, 50.0, rel = 0.03);
    assert_approx!(cpwg_result.z0, 50.0, rel = 0.05);
}

#[test]
fn test_cpwg_effective_epsilon_vs_microstrip() {
    let stackup = jlc_stackup();
    let tl = TransmissionLine::new(stackup.clone());
    let microstrip = tl.microstrip(0.25, "F.Cu", 1.0).unwrap();
    let cpwg = tl.cpwg(0.25, 0.15, "F.Cu", 1.0).unwrap();

    let er = stackup.get_dielectric_constant("F.Cu");
    assert!(1.0 < microstrip.epsilon_eff && microstrip.epsilon_eff < er);
    assert!(1.0 < cpwg.epsilon_eff && cpwg.epsilon_eff < er);

    assert!(cpwg.epsilon_eff > 1.0);
    assert!(cpwg.epsilon_eff < er);
}

#[test]
fn test_cpwg_propagation_delay_comparison() {
    let tl = jlc();
    let microstrip = tl.microstrip(0.25, "F.Cu", 1.0).unwrap();
    let cpwg = tl.cpwg(0.25, 0.15, "F.Cu", 1.0).unwrap();
    let delay_ratio = cpwg.propagation_delay_ps_per_mm() / microstrip.propagation_delay_ps_per_mm();
    assert!(0.7 < delay_ratio && delay_ratio < 1.3);
}
