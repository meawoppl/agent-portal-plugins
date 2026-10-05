//! Port of upstream `tests/test_ampacity.py`.

use kct::physics::ampacity::{
    adiabatic_fusing_current, rms_current_for_duty_cycle, width_for_current,
};
use kct::physics::PhysResult;

fn err_contains<T: std::fmt::Debug>(r: PhysResult<T>, needle: &str) {
    let e = r.expect_err("expected ValueError");
    assert!(
        e.0.contains(needle),
        "{:?} does not contain {needle:?}",
        e.0
    );
}

fn w(current: f64, oz: f64, dt: f64, layer: &str) -> f64 {
    width_for_current(current, oz, dt, layer).unwrap()
}

fn fuse(width: f64, oz: f64, dur: f64, ambient: f64) -> f64 {
    adiabatic_fusing_current(width, oz, dur, ambient).unwrap()
}

// --- TestWidthForCurrentGoldenValues ---

#[test]
fn test_golden_15a_2oz_external() {
    assert_approx!(w(15.0, 2.0, 10.0, "external"), 6.29, abs = 0.05);
}

#[test]
fn test_golden_15a_2oz_internal() {
    assert_approx!(w(15.0, 2.0, 10.0, "internal"), 16.37, abs = 0.05);
}

#[test]
fn test_internal_wider_than_external() {
    let ext = w(15.0, 2.0, 10.0, "external");
    let internal = w(15.0, 2.0, 10.0, "internal");
    assert!(internal > ext);
}

#[test]
fn test_golden_15a_1oz_external() {
    assert_approx!(w(15.0, 1.0, 10.0, "external"), 12.585, abs = 0.05);
}

#[test]
fn test_golden_15a_half_oz_internal() {
    assert_approx!(w(15.0, 0.5, 10.0, "internal"), 65.48, abs = 0.1);
}

// --- TestWidthForCurrentBehavior ---

#[test]
fn test_default_delta_t_is_10c() {
    // Rust has no default args; the upstream default is delta_t_c = 10.0,
    // so "implicit" is the documented default spelled out.
    let implicit = w(15.0, 2.0, 10.0, "external");
    let explicit = w(15.0, 2.0, 10.0, "external");
    assert_approx!(implicit, explicit);
}

#[test]
fn test_default_layer_is_external() {
    // Upstream default layer = "external".
    let implicit = w(15.0, 2.0, 10.0, "external");
    let explicit = w(15.0, 2.0, 10.0, "external");
    assert_approx!(implicit, explicit);
}

#[test]
fn test_higher_current_needs_wider_trace() {
    let narrow = w(5.0, 1.0, 10.0, "external");
    let wide = w(20.0, 1.0, 10.0, "external");
    assert!(wide > narrow);
}

#[test]
fn test_thicker_copper_needs_narrower_trace() {
    let thin = w(15.0, 1.0, 10.0, "external");
    let thick = w(15.0, 2.0, 10.0, "external");
    assert!(thick < thin);
}

#[test]
fn test_larger_delta_t_needs_narrower_trace() {
    let cold = w(15.0, 2.0, 10.0, "external");
    let hot = w(15.0, 2.0, 20.0, "external");
    assert!(hot < cold);
}

#[test]
fn test_returns_positive_float() {
    let v: f64 = w(1.0, 1.0, 10.0, "external");
    assert!(v > 0.0);
}

#[test]
fn test_exported_from_physics_package() {
    // Re-export identity: kct::physics::width_for_current is the same item.
    let a: fn(f64, f64, f64, &str) -> PhysResult<f64> = kct::physics::width_for_current;
    let b: fn(f64, f64, f64, &str) -> PhysResult<f64> = width_for_current;
    assert_eq!(
        a(15.0, 2.0, 10.0, "external"),
        b(15.0, 2.0, 10.0, "external")
    );
}

// --- TestWidthForCurrentValidation ---

#[test]
fn test_zero_current_raises() {
    err_contains(width_for_current(0.0, 2.0, 10.0, "external"), "current_a");
}

#[test]
fn test_negative_current_raises() {
    err_contains(width_for_current(-5.0, 2.0, 10.0, "external"), "current_a");
}

#[test]
fn test_zero_copper_weight_raises() {
    err_contains(
        width_for_current(15.0, 0.0, 10.0, "external"),
        "copper_weight_oz",
    );
}

#[test]
fn test_negative_copper_weight_raises() {
    err_contains(
        width_for_current(15.0, -1.0, 10.0, "external"),
        "copper_weight_oz",
    );
}

#[test]
fn test_zero_delta_t_raises() {
    err_contains(width_for_current(15.0, 2.0, 0.0, "external"), "delta_t_c");
}

#[test]
fn test_negative_delta_t_raises() {
    err_contains(width_for_current(15.0, 2.0, -10.0, "external"), "delta_t_c");
}

#[test]
fn test_invalid_layer_raises() {
    err_contains(width_for_current(15.0, 2.0, 10.0, "middle"), "layer");
}

// --- TestRmsCurrentForDutyCycle ---

#[test]
fn test_golden_30a_peak_quarter_duty_over_2a_baseline() {
    let i_rms = rms_current_for_duty_cycle(30.0, 0.25, 2.0).unwrap();
    assert_approx!(i_rms, 15.0997, abs = 1e-3);
}

#[test]
fn test_full_duty_is_the_peak() {
    assert_approx!(rms_current_for_duty_cycle(7.5, 1.0, 0.0).unwrap(), 7.5);
}

#[test]
fn test_zero_baseline_scales_as_sqrt_duty() {
    assert_approx!(rms_current_for_duty_cycle(10.0, 0.25, 0.0).unwrap(), 5.0);
}

#[test]
fn test_never_below_the_baseline() {
    let i_rms = rms_current_for_duty_cycle(30.0, 0.001, 2.0).unwrap();
    assert!(i_rms >= 2.0);
}

#[test]
fn test_monotonic_in_duty() {
    let low = rms_current_for_duty_cycle(10.0, 0.1, 0.0).unwrap();
    let high = rms_current_for_duty_cycle(10.0, 0.9, 0.0).unwrap();
    assert!(high > low);
}

#[test]
fn test_zero_duty_raises() {
    err_contains(rms_current_for_duty_cycle(10.0, 0.0, 0.0), "duty_cycle");
}

#[test]
fn test_duty_above_one_raises() {
    err_contains(rms_current_for_duty_cycle(10.0, 1.5, 0.0), "duty_cycle");
}

#[test]
fn test_non_positive_peak_raises() {
    err_contains(rms_current_for_duty_cycle(0.0, 0.5, 0.0), "peak_a");
}

#[test]
fn test_negative_baseline_raises() {
    err_contains(rms_current_for_duty_cycle(10.0, 0.5, -1.0), "baseline_a");
}

// --- TestAdiabaticFusingCurrent ---

#[test]
fn test_golden_1mm_1oz_one_second() {
    assert_approx!(fuse(1.0, 1.0, 1.0, 25.0), 10.10, abs = 0.05);
}

#[test]
fn test_linear_in_cross_section() {
    let narrow = fuse(1.0, 1.0, 1.0, 25.0);
    let wide = fuse(2.0, 1.0, 1.0, 25.0);
    assert_approx!(wide, 2.0 * narrow, rel = 1e-9);
}

#[test]
fn test_doubles_copper_weight_doubles_current() {
    let thin = fuse(1.0, 1.0, 1.0, 25.0);
    let thick = fuse(1.0, 2.0, 1.0, 25.0);
    assert_approx!(thick, 2.0 * thin, rel = 1e-9);
}

#[test]
fn test_quarter_duration_doubles_current() {
    let long_pulse = fuse(1.0, 1.0, 1.0, 25.0);
    let short_pulse = fuse(1.0, 1.0, 0.25, 25.0);
    assert_approx!(short_pulse, 2.0 * long_pulse, rel = 1e-9);
}

#[test]
fn test_hotter_ambient_lowers_fusing_current() {
    let cool = fuse(1.0, 1.0, 1.0, 25.0);
    let hot = fuse(1.0, 1.0, 1.0, 85.0);
    assert!(hot < cool);
}

#[test]
fn test_far_above_the_continuous_ipc_rating() {
    let continuous_width = w(2.3, 1.0, 10.0, "external");
    assert_approx!(continuous_width, 1.0, abs = 0.15);
    assert!(fuse(1.0, 1.0, 1.0, 25.0) > 2.3);
}

#[test]
fn test_non_positive_width_raises() {
    err_contains(adiabatic_fusing_current(0.0, 1.0, 1.0, 25.0), "width_mm");
}

#[test]
fn test_non_positive_duration_raises() {
    err_contains(adiabatic_fusing_current(1.0, 1.0, 0.0, 25.0), "duration_s");
}

#[test]
fn test_non_positive_copper_weight_raises() {
    err_contains(
        adiabatic_fusing_current(1.0, 0.0, 1.0, 25.0),
        "copper_weight_oz",
    );
}

#[test]
fn test_ambient_at_or_above_melting_raises() {
    err_contains(adiabatic_fusing_current(1.0, 1.0, 1.0, 1200.0), "ambient_c");
}

/// Second upstream `test_exported_from_physics_package` (in
/// `TestAdiabaticFusingCurrent`); suffixed since Rust test names share one module.
#[test]
fn test_exported_from_physics_package_fusing() {
    let a: fn(f64, f64, f64, f64) -> PhysResult<f64> = kct::physics::adiabatic_fusing_current;
    let b: fn(f64, f64, f64, f64) -> PhysResult<f64> = adiabatic_fusing_current;
    assert_eq!(a(1.0, 1.0, 1.0, 25.0), b(1.0, 1.0, 1.0, 25.0));
    let c: fn(f64, f64, f64) -> PhysResult<f64> = kct::physics::rms_current_for_duty_cycle;
    let d: fn(f64, f64, f64) -> PhysResult<f64> = rms_current_for_duty_cycle;
    assert_eq!(c(10.0, 0.5, 0.0), d(10.0, 0.5, 0.0));
}
