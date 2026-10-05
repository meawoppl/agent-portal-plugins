//! Ports of upstream kicad-tools physics + `impedance` CLI tests
//! (`tests/test_physics_*.py`, `test_transmission_line.py`,
//! `test_stripline_capacitance.py`, `test_coupled_lines.py`,
//! `test_crosstalk.py`, `test_timing.py`, `test_ampacity.py`,
//! `test_named_stackup.py`, wire-gauge cases of `test_pcb_reinforce.py`).

/// `pytest.approx` semantics: `|actual - expected| <= max(rel * |expected|, abs)`,
/// defaults `rel = 1e-6`, `abs = 1e-12`.
pub fn approx(actual: f64, expected: f64, rel: Option<f64>, abs: Option<f64>) -> bool {
    if actual == expected {
        return true;
    }
    let rel_tol = match (rel, abs) {
        (None, Some(_)) => 0.0,
        (r, _) => r.unwrap_or(1e-6),
    };
    let tol = (rel_tol * expected.abs()).max(abs.unwrap_or(1e-12));
    (actual - expected).abs() <= tol
}

/// `assert approx(a, b)` with optional `rel = x` / `abs = y`.
#[macro_export]
macro_rules! assert_approx {
    ($a:expr, $b:expr) => {
        assert_approx!(@ $a, $b, None, None)
    };
    ($a:expr, $b:expr, rel = $r:expr) => {
        assert_approx!(@ $a, $b, Some($r), None)
    };
    ($a:expr, $b:expr, abs = $t:expr) => {
        assert_approx!(@ $a, $b, None, Some($t))
    };
    ($a:expr, $b:expr, rel = $r:expr, abs = $t:expr) => {
        assert_approx!(@ $a, $b, Some($r), Some($t))
    };
    (@ $a:expr, $b:expr, $r:expr, $t:expr) => {{
        let (a, b): (f64, f64) = ($a, $b);
        let (r, t): (Option<f64>, Option<f64>) = ($r, $t);
        assert!(
            $crate::approx(a, b, r, t),
            "{} = {a} not approx {} = {b} (rel={r:?}, abs={t:?})",
            stringify!($a),
            stringify!($b),
        );
    }};
}

/// Path under `kct/tests/fixtures`.
pub fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

#[path = "physics/ampacity.rs"]
mod ampacity;
#[path = "physics/cli.rs"]
mod cli;
#[path = "physics/coupled_lines.rs"]
mod coupled_lines;
#[path = "physics/crosstalk.rs"]
mod crosstalk;
#[path = "physics/stackup.rs"]
mod stackup;
#[path = "physics/stripline.rs"]
mod stripline;
#[path = "physics/timing.rs"]
mod timing;
#[path = "physics/transmission_line.rs"]
mod transmission_line;
#[path = "physics/wire_gauge.rs"]
mod wire_gauge;
