//! IPC-2221 ampacity: minimum trace width from target current, plus pulsed /
//! duty-cycled helpers (port of `physics.ampacity`).
//!
//! `I = k * delta_t**0.44 * A**0.725` with `A` in mils^2, `k = 0.048`
//! (external) or `0.024` (internal). Inverted in closed form for width.

use super::{PhysResult, ValueError};

const K_EXTERNAL: f64 = 0.048;
const K_INTERNAL: f64 = 0.024;
const DELTA_T_EXPONENT: f64 = 0.44;
const AREA_EXPONENT: f64 = 0.725;
/// 1 oz/ft^2 copper ~= 1.378 mils thick.
const MILS_PER_OZ: f64 = 1.378;
const MM_PER_MIL: f64 = 0.0254;
const VALID_LAYERS: [&str; 2] = ["external", "internal"];

// Onderdonk adiabatic fusing constants (copper).
const COPPER_MELTING_C: f64 = 1083.0;
const COPPER_INVERSE_TEMP_COEFF_C: f64 = 234.0;
const ONDERDONK_TIME_CONSTANT: f64 = 33.0;
const CIRCULAR_MILS_PER_SQUARE_MIL: f64 = 4.0 / std::f64::consts::PI;

fn py(x: f64) -> String {
    super::py_float_repr(x)
}

/// Minimum trace width (mm) for `current_a` via IPC-2221.
/// `layer` is `"external"` or `"internal"`; upstream defaults are
/// `delta_t_c = 10.0`, `layer = "external"`.
pub fn width_for_current(
    current_a: f64,
    copper_weight_oz: f64,
    delta_t_c: f64,
    layer: &str,
) -> PhysResult<f64> {
    if current_a <= 0.0 {
        return Err(ValueError(format!(
            "current_a must be positive, got {}",
            py(current_a)
        )));
    }
    if copper_weight_oz <= 0.0 {
        return Err(ValueError(format!(
            "copper_weight_oz must be positive, got {}",
            py(copper_weight_oz)
        )));
    }
    if delta_t_c <= 0.0 {
        return Err(ValueError(format!(
            "delta_t_c must be positive, got {}",
            py(delta_t_c)
        )));
    }
    if !VALID_LAYERS.contains(&layer) {
        return Err(ValueError(format!(
            "layer must be one of ('external', 'internal'), got '{layer}'"
        )));
    }
    let k = if layer == "external" {
        K_EXTERNAL
    } else {
        K_INTERNAL
    };
    let area_mils2 = (current_a / (k * delta_t_c.powf(DELTA_T_EXPONENT))).powf(1.0 / AREA_EXPONENT);
    let thickness_mils = copper_weight_oz * MILS_PER_OZ;
    Ok(area_mils2 / thickness_mils * MM_PER_MIL)
}

/// Thermally equivalent RMS current of a rectangular pulse train
/// (`peak_a` for `duty_cycle`, `baseline_a` otherwise; upstream default 0).
pub fn rms_current_for_duty_cycle(
    peak_a: f64,
    duty_cycle: f64,
    baseline_a: f64,
) -> PhysResult<f64> {
    if peak_a <= 0.0 {
        return Err(ValueError(format!(
            "peak_a must be positive, got {}",
            py(peak_a)
        )));
    }
    if !(0.0 < duty_cycle && duty_cycle <= 1.0) {
        return Err(ValueError(format!(
            "duty_cycle must be in (0, 1], got {}",
            py(duty_cycle)
        )));
    }
    if baseline_a < 0.0 {
        return Err(ValueError(format!(
            "baseline_a must be non-negative, got {}",
            py(baseline_a)
        )));
    }
    let mean_square = duty_cycle * peak_a.powf(2.0) + (1.0 - duty_cycle) * baseline_a.powf(2.0);
    Ok(mean_square.sqrt())
}

/// Onderdonk adiabatic fusing current (A) for a copper trace
/// (upstream default `ambient_c = 25.0`).
pub fn adiabatic_fusing_current(
    width_mm: f64,
    copper_weight_oz: f64,
    duration_s: f64,
    ambient_c: f64,
) -> PhysResult<f64> {
    if width_mm <= 0.0 {
        return Err(ValueError(format!(
            "width_mm must be positive, got {}",
            py(width_mm)
        )));
    }
    if copper_weight_oz <= 0.0 {
        return Err(ValueError(format!(
            "copper_weight_oz must be positive, got {}",
            py(copper_weight_oz)
        )));
    }
    if duration_s <= 0.0 {
        return Err(ValueError(format!(
            "duration_s must be positive, got {}",
            py(duration_s)
        )));
    }
    if ambient_c >= COPPER_MELTING_C {
        return Err(ValueError(format!(
            "ambient_c must be below copper's melting point ({} C), got {}",
            py(COPPER_MELTING_C),
            py(ambient_c)
        )));
    }
    if ambient_c <= -COPPER_INVERSE_TEMP_COEFF_C {
        return Err(ValueError(format!(
            "ambient_c must be above {} C, got {}",
            py(-COPPER_INVERSE_TEMP_COEFF_C),
            py(ambient_c)
        )));
    }
    let width_mils = width_mm / MM_PER_MIL;
    let thickness_mils = copper_weight_oz * MILS_PER_OZ;
    let area_cmil = width_mils * thickness_mils * CIRCULAR_MILS_PER_SQUARE_MIL;
    let temp_ratio = (COPPER_MELTING_C - ambient_c) / (COPPER_INVERSE_TEMP_COEFF_C + ambient_c);
    let numerator = (temp_ratio + 1.0).log10();
    Ok(area_cmil * (numerator / (ONDERDONK_TIME_CONSTANT * duration_s)).sqrt())
}
