//! Port of the `TestWireGauge` physics cases of upstream
//! `tests/test_pcb_reinforce.py` (`kicad_tools.physics.wire_gauge`). The
//! reinforce-pass / CLI tests in that file exercise `pcb.reinforce` and are
//! not part of the physics port.

use kct::physics::wire_gauge::{
    anchor_drill_for_awg, anchor_pad_for_drill, bare_copper_diameter_mm, supported_gauges,
    wire_ampacity, DEFAULT_SLIP_FIT_CLEARANCE_MM,
};
use std::collections::BTreeSet;

fn drill(awg: i64) -> f64 {
    anchor_drill_for_awg(awg, DEFAULT_SLIP_FIT_CLEARANCE_MM).unwrap()
}

fn amp(awg: i64, temp_rise_c: f64) -> f64 {
    wire_ampacity(awg, temp_rise_c).unwrap()
}

#[test]
fn test_bare_copper_diameters() {
    assert_approx!(bare_copper_diameter_mm(16).unwrap(), 1.291, abs = 1e-3);
    assert_approx!(bare_copper_diameter_mm(14).unwrap(), 1.628, abs = 1e-3);
    assert_approx!(bare_copper_diameter_mm(12).unwrap(), 2.053, abs = 1e-3);
}

#[test]
fn test_supported_gauges() {
    let got: BTreeSet<i64> = supported_gauges().into_iter().collect();
    assert_eq!(got, BTreeSet::from([12, 14, 16]));
}

#[test]
fn test_unsupported_gauge_raises() {
    let e = bare_copper_diameter_mm(10).expect_err("expected ValueError");
    assert!(e.0.contains("unsupported wire gauge"), "{:?}", e.0);
}

#[test]
fn test_anchor_drill_16awg() {
    // 1.291 bare + 0.125 slip-fit => ~1.416 mm, within the issue's
    // ~1.40-1.45 mm band.
    let d = drill(16);
    assert_approx!(d, 1.416, abs = 1e-3);
    assert!((1.40..=1.45).contains(&d));
}

#[test]
fn test_anchor_drill_14_12() {
    assert!((1.75..=1.80).contains(&drill(14)));
    assert!((2.15..=2.20).contains(&drill(12)));
}

#[test]
fn test_anchor_drill_custom_clearance() {
    assert_approx!(anchor_drill_for_awg(16, 0.0).unwrap(), 1.291, abs = 1e-3);
}

#[test]
fn test_anchor_drill_negative_clearance_raises() {
    let e = anchor_drill_for_awg(16, -0.1).expect_err("expected ValueError");
    assert!(e.0.contains("slip_fit_clearance_mm"), "{:?}", e.0);
}

#[test]
fn test_anchor_pad_meets_annular_ring() {
    let d = drill(16);
    let pad = anchor_pad_for_drill(d, 0.25).unwrap();
    // pad within the issue's ~1.90-1.95 mm band at 0.25 annular ring.
    assert_approx!(pad, 1.916, abs = 1e-3);
    assert!((1.90..=1.95).contains(&pad));
    // Annular ring formula must hold.
    assert_approx!((pad - d) / 2.0, 0.25, abs = 1e-6);
}

#[test]
fn test_anchor_pad_sources_annular_from_rules() {
    let d = drill(16);
    // A larger annular-ring floor => larger pad (not hardcoded).
    let pad_small = anchor_pad_for_drill(d, 0.15).unwrap();
    let pad_large = anchor_pad_for_drill(d, 0.30).unwrap();
    assert!(pad_large > pad_small);
    assert_approx!((pad_large - d) / 2.0, 0.30, abs = 1e-6);
}

#[test]
fn test_anchor_pad_invalid_args() {
    assert!(anchor_pad_for_drill(0.0, 0.25).is_err());
    assert!(anchor_pad_for_drill(1.4, -0.1).is_err());
}

#[test]
fn test_wire_ampacity_monotonic_and_positive() {
    // Larger wire (lower AWG) carries more current.
    let a16 = amp(16, 10.0);
    let a14 = amp(14, 10.0);
    let a12 = amp(12, 10.0);
    assert!(0.0 < a16 && a16 < a14 && a14 < a12);
    // Sanity magnitude for 16 AWG at a 10 C rise.
    assert!((20.0..=45.0).contains(&a16));
}

#[test]
fn test_wire_ampacity_scales_with_temp_rise() {
    assert!(amp(16, 20.0) > amp(16, 10.0));
}

#[test]
fn test_wire_ampacity_invalid_temp() {
    assert!(wire_ampacity(16, 0.0).is_err());
}
