//! Physics-based calculations for PCB signal integrity (port of
//! `kicad_tools.physics`): transmission-line impedance, coupled lines,
//! crosstalk, timing, IPC-2221 ampacity, and wire gauges.
//!
//! These are analytical models fast enough for agent iteration; the stripline
//! solver is a small quasi-static boundary-element solve (see [`stripline`]).

pub mod ampacity;
pub mod constants;
pub mod coupled_lines;
pub mod crosstalk;
pub mod stackup;
pub mod stripline;
pub mod timing;
pub mod transmission_line;
pub mod wire_gauge;

use std::fmt;

pub use ampacity::{adiabatic_fusing_current, rms_current_for_duty_cycle, width_for_current};
pub use constants::{
    copper_oz_from_thickness, copper_thickness_from_oz, get_material, get_material_or_default,
    CopperWeight, DielectricMaterial, COPPER_1OZ, COPPER_2OZ, COPPER_CONDUCTIVITY, COPPER_HALF_OZ,
    FR4_HIGH_TG, FR4_STANDARD, ISOLA_370HR, ROGERS_4003C, ROGERS_4350B, SPEED_OF_LIGHT,
};
pub use coupled_lines::{CoupledLines, DifferentialPairResult};
pub use crosstalk::{CrosstalkAnalyzer, CrosstalkResult};
pub use stackup::{Construction, LayerType, Stackup, StackupLayer};
pub use timing::{DifferentialPairSkew, PropagationResult, TimingAnalyzer, TimingBudget};
pub use transmission_line::{ImpedanceResult, TransmissionLine};
pub use wire_gauge::{
    anchor_drill_for_awg, anchor_pad_for_drill, bare_copper_diameter_mm, supported_gauges,
    wire_ampacity, DEFAULT_SLIP_FIT_CLEARANCE_MM, DEFAULT_WIRE_GAUGE_AWG,
};

/// Invalid-argument error (Python `ValueError`). The message matches upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError(pub String);

impl ValueError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}

pub type PhysResult<T> = std::result::Result<T, ValueError>;

/// Python `repr(float)`: shortest round-trip digits, scientific notation
/// outside `1e-4 <= |x| < 1e16`, always with a decimal point or exponent.
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    if (-4..16).contains(&exp) {
        let s = format!("{x}");
        if s.contains('.') {
            s
        } else {
            format!("{s}.0")
        }
    } else {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exp.abs())
    }
}

/// Python (3.12+) `sum()` over floats: Neumaier compensated summation, so
/// totals such as stackup thickness print like upstream (`1.5564`, not
/// `1.5564000000000002`).
pub fn py_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut total = 0.0f64;
    let mut c = 0.0f64;
    for x in values {
        let t = total + x;
        if total.abs() >= x.abs() {
            c += (total - t) + x;
        } else {
            c += (x - t) + total;
        }
        total = t;
    }
    if c != 0.0 && c.is_finite() {
        total += c;
    }
    total
}

/// Python `round(x, ndigits)` for floats (correctly rounded decimal).
pub fn py_round(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}").parse().unwrap_or(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_repr_matches_python() {
        assert_eq!(py_float_repr(50.0), "50.0");
        assert_eq!(py_float_repr(0.035), "0.035");
        assert_eq!(py_float_repr(1e-5), "1e-05");
        assert_eq!(py_float_repr(0.0001), "0.0001");
        assert_eq!(py_float_repr(1e16), "1e+16");
        assert_eq!(py_float_repr(-2.5), "-2.5");
        assert_eq!(py_float_repr(1.5564), "1.5564");
        assert_eq!(py_round(0.12345, 3), 0.123);
    }
}
