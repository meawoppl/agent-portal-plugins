//! Port of upstream `tests/test_stripline_capacitance.py`.

use crate::assert_approx;
use kct::physics::stripline::stripline_impedance;
use kct::physics::CoupledLines;

fn z(w: f64, h1: f64, h2: f64, t: f64, er: f64) -> f64 {
    stripline_impedance(w, h1, h2, t, er).unwrap()
}

#[test]
fn test_centered_thin_strip_matches_exact_conformal_solution() {
    // Exact thin centered strip: eta0/(4 sqrt(Er)) K(k')/K(k),
    // k=tanh(pi*w/(2*H)); H=1, Er=4.5.
    let cases = [
        (0.05, 111.107350),
        (0.2, 72.138691),
        (0.5, 47.344311),
        (1.0, 30.807994),
        (2.0, 18.186467),
    ];
    for (width, expected) in cases {
        assert_approx!(z(width, 0.5, 0.5, 0.0, 4.5), expected, rel = 1e-4);
    }
}

#[test]
fn test_offset_strip_matches_independent_finite_volume_solution() {
    assert_approx!(z(0.16, 0.13, 1.078, 0.0152, 4.5), 50.2, abs = 0.15);
}

#[test]
fn test_centered_thick_strip_matches_independent_finite_volume_solution() {
    assert_approx!(z(0.16, 0.13, 0.13, 0.0152, 4.5), 38.9, abs = 0.15);
}

#[test]
fn test_second_centered_geometry_matches_finite_volume_solution() {
    assert_approx!(z(0.2, 0.2, 0.2, 0.035, 4.5), 42.10279, abs = 0.05);
}

#[test]
fn test_offset_reflection_and_geometric_scale_invariance() {
    let expected = z(0.16, 0.13, 1.078, 0.0152, 4.5);
    assert_eq!(z(0.16, 1.078, 0.13, 0.0152, 4.5), expected);
    for scale in [0.001, 1000.0] {
        assert_approx!(
            z(
                0.16 * scale,
                0.13 * scale,
                1.078 * scale,
                0.0152 * scale,
                4.5
            ),
            expected,
            rel = 1e-9
        );
    }
}

#[test]
fn test_homogeneous_permittivity_scaling() {
    let vacuum = z(0.16, 0.13, 1.078, 0.0152, 1.0);
    assert_approx!(
        z(0.16, 0.13, 1.078, 0.0152, 4.5),
        vacuum / 4.5f64.sqrt(),
        rel = 1e-12
    );
}

#[test]
fn test_remote_plane_approaches_finite_single_plane_limit() {
    let close = z(0.16, 0.13, 1.0, 0.0152, 4.5);
    let far = z(0.16, 0.13, 100.0, 0.0152, 4.5);
    let farther = z(0.16, 0.13, 1000.0, 0.0152, 4.5);
    assert!(close < far && far < farther && farther < 51.0);
    assert_approx!(farther, far, abs = 0.001);
}

#[test]
fn test_no_artificial_ten_ohm_clamp_on_wide_trace() {
    // Parallel-plate leading term with edge fringing lowering Z further.
    let (width, gap, er) = (10.0, 0.1, 4.0_f64);
    let parallel_plate = 376.730313668 * gap / (2.0 * width * er.sqrt());
    let result = z(width, gap, gap, 0.01, er);
    assert!(0.0 < result && result < parallel_plate && parallel_plate < 10.0);
}

#[test]
fn test_invalid_geometry_fails_instead_of_returning_plausible_impedance() {
    let cases = [
        (0.0, 0.1, 0.2, 0.01, 4.0),
        (0.1, 0.0, 0.2, 0.01, 4.0),
        (0.1, 0.2, 0.3, -0.01, 4.0),
        (0.1, 0.2, 0.3, 0.01, 0.0),
        (0.1, 0.2, f64::INFINITY, 0.01, 4.0),
        (0.1, 0.2, 0.3, f64::NAN, 4.0),
    ];
    for (w, h1, h2, t, er) in cases {
        assert!(
            stripline_impedance(w, h1, h2, t, er).is_err(),
            "expected error for {:?}",
            (w, h1, h2, t, er)
        );
    }
}

#[test]
fn test_coupled_stripline_uses_correct_offset_single_line_limit() {
    // Upstream calls `CoupledLines(Stackup.jlcpcb_4layer())._edge_coupled_stripline_calc`;
    // the Rust port exposes it as an associated function (no stackup needed).
    let result =
        CoupledLines::edge_coupled_stripline_calc(0.16, 10.0, 0.13, 1.078, 4.5, 0.0152).unwrap();
    // The empirical model retains a 1% coupling floor, but must approach
    // twice the independently verified single-line impedance, not ~150 ohm.
    assert_approx!(result.zdiff, 2.0 * 50.25155, rel = 0.011);
}
