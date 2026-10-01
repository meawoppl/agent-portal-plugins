//! AWG wire-gauge constants and buttress-wire anchor geometry (port of
//! `physics.wire_gauge`).

use super::{PhysResult, ValueError};

/// Standard AWG bare (solid) copper diameters, mm.
const AWG_DIAMETER_MM: &[(i64, f64)] = &[(12, 2.053), (14, 1.628), (16, 1.291)];

/// Slip-fit clearance added to the wire diameter for the anchor drill.
pub const DEFAULT_SLIP_FIT_CLEARANCE_MM: f64 = 0.125;
/// Default reinforce wire gauge (16 AWG solid core).
pub const DEFAULT_WIRE_GAUGE_AWG: i64 = 16;

const AMPACITY_K: f64 = 0.048;
const DELTA_T_EXPONENT: f64 = 0.44;
const AREA_EXPONENT: f64 = 0.725;
const MM2_PER_MIL2: f64 = 1.0 / (0.0254 * 0.0254);
const MATH_PI: f64 = std::f64::consts::PI;

fn py(x: f64) -> String {
    super::py_float_repr(x)
}

/// Supported gauges, ascending.
pub fn supported_gauges() -> Vec<i64> {
    let mut g: Vec<i64> = AWG_DIAMETER_MM.iter().map(|(a, _)| *a).collect();
    g.sort();
    g
}

/// Bare copper diameter (mm) for an AWG gauge.
pub fn bare_copper_diameter_mm(awg: i64) -> PhysResult<f64> {
    AWG_DIAMETER_MM
        .iter()
        .find(|(a, _)| *a == awg)
        .map(|(_, d)| *d)
        .ok_or_else(|| {
            let supported: Vec<String> = supported_gauges().iter().map(|g| g.to_string()).collect();
            ValueError(format!(
                "unsupported wire gauge {awg} AWG; supported gauges: {}",
                supported.join(", ")
            ))
        })
}

/// Anchor drill diameter: wire diameter plus slip-fit clearance
/// (upstream default [`DEFAULT_SLIP_FIT_CLEARANCE_MM`]).
pub fn anchor_drill_for_awg(awg: i64, slip_fit_clearance_mm: f64) -> PhysResult<f64> {
    if slip_fit_clearance_mm < 0.0 {
        return Err(ValueError(format!(
            "slip_fit_clearance_mm must be >= 0, got {}",
            py(slip_fit_clearance_mm)
        )));
    }
    Ok(bare_copper_diameter_mm(awg)? + slip_fit_clearance_mm)
}

/// Pad diameter meeting the annular-ring floor: `drill + 2 * ring`.
pub fn anchor_pad_for_drill(drill_mm: f64, min_annular_ring_mm: f64) -> PhysResult<f64> {
    if drill_mm <= 0.0 {
        return Err(ValueError(format!(
            "drill_mm must be positive, got {}",
            py(drill_mm)
        )));
    }
    if min_annular_ring_mm < 0.0 {
        return Err(ValueError(format!(
            "min_annular_ring_mm must be >= 0, got {}",
            py(min_annular_ring_mm)
        )));
    }
    Ok(drill_mm + 2.0 * min_annular_ring_mm)
}

/// IPC-2221 external-form ampacity (A) of a round wire of this gauge
/// (upstream default `temp_rise_c = 10.0`).
pub fn wire_ampacity(awg: i64, temp_rise_c: f64) -> PhysResult<f64> {
    if temp_rise_c <= 0.0 {
        return Err(ValueError(format!(
            "temp_rise_c must be positive, got {}",
            py(temp_rise_c)
        )));
    }
    let diameter_mm = bare_copper_diameter_mm(awg)?;
    let radius_mm = diameter_mm / 2.0;
    let area_mils2 = MATH_PI * radius_mm * radius_mm * MM2_PER_MIL2;
    Ok(AMPACITY_K * temp_rise_c.powf(DELTA_T_EXPONENT) * area_mils2.powf(AREA_EXPONENT))
}
