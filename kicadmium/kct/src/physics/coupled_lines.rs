//! Coupled transmission lines for differential pairs (port of
//! `physics.coupled_lines`): edge-coupled microstrip/stripline and
//! broadside-coupled stripline.

use std::f64::consts::PI;

use super::constants::SPEED_OF_LIGHT;
use super::stackup::Stackup;
use super::stripline::stripline_impedance;
use super::transmission_line::TransmissionLine;
use super::{py_float_repr as py, PhysResult, ValueError};

/// Result of differential pair analysis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DifferentialPairResult {
    pub zdiff: f64,
    pub zcommon: f64,
    pub z0_even: f64,
    pub z0_odd: f64,
    /// `k = (Z0e - Z0o) / (Z0e + Z0o)`.
    pub coupling_coefficient: f64,
    pub epsilon_eff_even: f64,
    pub epsilon_eff_odd: f64,
}

impl DifferentialPairResult {
    pub fn phase_velocity_even(&self) -> f64 {
        if self.epsilon_eff_even <= 0.0 {
            return SPEED_OF_LIGHT;
        }
        SPEED_OF_LIGHT / self.epsilon_eff_even.sqrt()
    }

    pub fn phase_velocity_odd(&self) -> f64 {
        if self.epsilon_eff_odd <= 0.0 {
            return SPEED_OF_LIGHT;
        }
        SPEED_OF_LIGHT / self.epsilon_eff_odd.sqrt()
    }

    fn from_modes(z0_even: f64, z0_odd: f64, eps_even: f64, eps_odd: f64) -> Self {
        Self {
            zdiff: 2.0 * z0_odd,
            zcommon: z0_even / 2.0,
            z0_even,
            z0_odd,
            coupling_coefficient: (z0_even - z0_odd) / (z0_even + z0_odd),
            epsilon_eff_even: eps_even,
            epsilon_eff_odd: eps_odd,
        }
    }
}

impl std::fmt::Display for DifferentialPairResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DifferentialPairResult(Zdiff={:.1}Ω, Zcommon={:.1}Ω, k={:.3})",
            self.zdiff, self.zcommon, self.coupling_coefficient
        )
    }
}

/// Python `max(lo, min(x, hi))`.
fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    let m = if hi < x { hi } else { x };
    if m > lo {
        m
    } else {
        lo
    }
}

/// Coupled line analysis over a stackup.
#[derive(Debug, Clone)]
pub struct CoupledLines {
    pub stackup: Stackup,
}

fn check_wg(width_mm: f64, gap_mm: f64) -> PhysResult<()> {
    if width_mm <= 0.0 {
        return Err(ValueError(format!(
            "Trace width must be positive, got {}",
            py(width_mm)
        )));
    }
    if gap_mm <= 0.0 {
        return Err(ValueError(format!(
            "Gap must be positive, got {}",
            py(gap_mm)
        )));
    }
    Ok(())
}

impl CoupledLines {
    pub fn new(stackup: Stackup) -> Self {
        Self { stackup }
    }

    /// Edge-coupled microstrip pair on an outer layer.
    pub fn edge_coupled_microstrip(
        &self,
        width_mm: f64,
        gap_mm: f64,
        layer: &str,
    ) -> PhysResult<DifferentialPairResult> {
        check_wg(width_mm, gap_mm)?;
        let h = self.stackup.get_reference_plane_distance(layer)?;
        if h <= 0.0 {
            return Err(ValueError(format!(
                "Could not determine dielectric height for layer {layer}"
            )));
        }
        let er = self.stackup.get_dielectric_constant(layer);
        let t = self.stackup.get_copper_thickness(layer);
        Ok(Self::edge_coupled_microstrip_calc(
            width_mm, gap_mm, h, er, t,
        ))
    }

    pub fn edge_coupled_microstrip_calc(
        w: f64,
        s: f64,
        h: f64,
        er: f64,
        t: f64,
    ) -> DifferentialPairResult {
        let u = w / h;
        let g = s / h;
        let single = TransmissionLine::microstrip_calc(w, h, er, t, 0.02, 1.0);
        let kc = (-1.9 * g).exp() * (1.0 - (-0.8 * u).exp());
        let kc = clamp(kc, 0.01, 0.7);
        let z0_even = single.z0 * ((1.0 + kc) / (1.0 - kc)).sqrt();
        let z0_odd = single.z0 * ((1.0 - kc) / (1.0 + kc)).sqrt();
        DifferentialPairResult::from_modes(
            z0_even,
            z0_odd,
            single.epsilon_eff * (1.0 + 0.1 * kc),
            single.epsilon_eff * (1.0 - 0.15 * kc),
        )
    }

    /// Edge-coupled stripline pair on an inner layer.
    pub fn edge_coupled_stripline(
        &self,
        width_mm: f64,
        gap_mm: f64,
        layer: &str,
    ) -> PhysResult<DifferentialPairResult> {
        check_wg(width_mm, gap_mm)?;
        let (h1, h2) = self.stackup.get_stripline_geometry(layer)?;
        if h1 <= 0.0 || h2 <= 0.0 {
            return Err(ValueError(format!(
                "Could not determine geometry for layer {layer}"
            )));
        }
        let er = self.stackup.get_dielectric_constant(layer);
        let t = self.stackup.get_copper_thickness(layer);
        Self::edge_coupled_stripline_calc(width_mm, gap_mm, h1, h2, er, t)
    }

    pub fn edge_coupled_stripline_calc(
        w: f64,
        s: f64,
        h1: f64,
        h2: f64,
        er: f64,
        t: f64,
    ) -> PhysResult<DifferentialPairResult> {
        let z0_single = stripline_impedance(w, h1, h2, t, er)?;
        let h_eff = h1.min(h2);
        let g = if h_eff > 0.0 { s / h_eff } else { 1.0 };
        let u = if h_eff > 0.0 { w / h_eff } else { 0.5 };
        let kc = (-1.6 * g).exp() * (1.0 - (-0.6 * u).exp());
        let kc = clamp(kc, 0.01, 0.7);
        let z0_even = clamp(z0_single * ((1.0 + kc) / (1.0 - kc)).sqrt(), 20.0, 200.0);
        let z0_odd = clamp(z0_single * ((1.0 - kc) / (1.0 + kc)).sqrt(), 15.0, 180.0);
        Ok(DifferentialPairResult::from_modes(z0_even, z0_odd, er, er))
    }

    /// Broadside-coupled stripline pair on two layers.
    pub fn broadside_coupled_stripline(
        &self,
        width_mm: f64,
        layer1: &str,
        layer2: &str,
    ) -> PhysResult<DifferentialPairResult> {
        if width_mm <= 0.0 {
            return Err(ValueError(format!(
                "Trace width must be positive, got {}",
                py(width_mm)
            )));
        }
        let mut idx1 = self.stackup.get_layer_index(layer1);
        let mut idx2 = self.stackup.get_layer_index(layer2);
        if idx1 < 0 || idx2 < 0 {
            return Err(ValueError(format!(
                "Could not find layers {layer1} and/or {layer2}"
            )));
        }
        let (mut layer1, mut layer2) = (layer1, layer2);
        if idx1 > idx2 {
            std::mem::swap(&mut idx1, &mut idx2);
            std::mem::swap(&mut layer1, &mut layer2);
        }
        let between = &self.stackup.layers[idx1 as usize + 1..idx2 as usize];
        let gap: f64 = between
            .iter()
            .filter(|l| l.is_dielectric())
            .map(|l| l.thickness_mm)
            .sum();
        if gap <= 0.0 {
            return Err(ValueError(format!(
                "No dielectric found between {layer1} and {layer2}"
            )));
        }
        let ers: Vec<f64> = between
            .iter()
            .filter(|l| l.is_dielectric() && l.epsilon_r > 0.0)
            .map(|l| l.epsilon_r)
            .collect();
        let er = if ers.is_empty() {
            4.5
        } else {
            super::py_sum(ers.iter().copied()) / ers.len() as f64
        };
        let h1 = self.stackup.get_dielectric_height(layer1)?;
        let h2 = self.stackup.get_dielectric_height(layer2)?;
        let t1 = self.stackup.get_copper_thickness(layer1);
        let t2 = self.stackup.get_copper_thickness(layer2);
        Ok(Self::broadside_coupled_calc(
            width_mm,
            gap,
            h1,
            h2,
            er,
            (t1 + t2) / 2.0,
        ))
    }

    pub fn broadside_coupled_calc(
        w: f64,
        gap: f64,
        h1: f64,
        h2: f64,
        er: f64,
        t: f64,
    ) -> DifferentialPairResult {
        let b = h1 + gap + h2 + 2.0 * t;
        let gap_over_w = if w > 0.0 { gap / w } else { 1.0 };
        let w_eff = if t > 0.0 && h1 > 0.0 {
            w + (t / PI) * (1.0 + (2.0 * h1 / t).ln())
        } else {
            w
        };
        let denom = 0.67 * PI * (0.8 * w_eff + t);
        let z0_single = if denom > 0.0 && b > 0.0 {
            (60.0 / er.sqrt()) * (4.0 * b / denom).ln()
        } else {
            50.0
        };
        let k = (-PI * gap_over_w).exp();
        let (z0_even, z0_odd) = if k < 0.999 {
            (
                z0_single * ((1.0 + k) / (1.0 - k)).sqrt(),
                z0_single * ((1.0 - k) / (1.0 + k)).sqrt(),
            )
        } else {
            (z0_single * 2.0, z0_single * 0.5)
        };
        let z0_even = clamp(z0_even, 20.0, 300.0);
        let z0_odd = clamp(z0_odd, 10.0, 200.0);
        DifferentialPairResult::from_modes(z0_even, z0_odd, er, er)
    }

    fn calc(
        &self,
        microstrip: bool,
        w: f64,
        g: f64,
        layer: &str,
    ) -> PhysResult<DifferentialPairResult> {
        if microstrip {
            self.edge_coupled_microstrip(w, g, layer)
        } else {
            self.edge_coupled_stripline(w, g, layer)
        }
    }

    /// Trace width at a fixed pair gap (upstream defaults `tolerance = 0.001`,
    /// `max_iterations = 60`).
    pub fn width_for_differential_impedance(
        &self,
        zdiff_target: f64,
        gap_mm: f64,
        layer: &str,
    ) -> PhysResult<f64> {
        self.width_for_differential_impedance_with(zdiff_target, gap_mm, layer, 0.001, 60)
    }

    pub fn width_for_differential_impedance_with(
        &self,
        zdiff_target: f64,
        gap_mm: f64,
        layer: &str,
        tolerance: f64,
        max_iterations: usize,
    ) -> PhysResult<f64> {
        if zdiff_target <= 0.0 || gap_mm <= 0.0 {
            return Err(ValueError::new(
                "Differential target and pair gap must be positive",
            ));
        }
        let ms = self.stackup.is_outer_layer(layer);
        let height = self.stackup.get_reference_plane_distance(layer)?;
        let (mut lo, mut hi) = (height * 0.001, height * 50.0);
        let in_bounds = self.calc(ms, hi, gap_mm, layer)?.zdiff <= zdiff_target
            && zdiff_target <= self.calc(ms, lo, gap_mm, layer)?.zdiff;
        if !in_bounds {
            return Err(ValueError::new(
                "Differential target is outside fixed-gap width bounds",
            ));
        }
        for _ in 0..max_iterations {
            let mid = (lo + hi) / 2.0;
            let actual = self.calc(ms, mid, gap_mm, layer)?.zdiff;
            if (actual - zdiff_target).abs() <= tolerance * zdiff_target {
                return Ok(mid);
            }
            if actual > zdiff_target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Err(ValueError::new(
            "Fixed-gap differential width solver did not converge",
        ))
    }

    /// Gap for a target differential impedance. `mode` is
    /// `edge_microstrip` (upstream default), `edge_stripline`, or `auto`;
    /// upstream defaults `tolerance = 0.02`, `max_iterations = 50`.
    pub fn gap_for_differential_impedance(
        &self,
        zdiff_target: f64,
        width_mm: f64,
        layer: &str,
        mode: &str,
    ) -> PhysResult<f64> {
        self.gap_for_differential_impedance_with(zdiff_target, width_mm, layer, mode, 0.02, 50)
    }

    pub fn gap_for_differential_impedance_with(
        &self,
        zdiff_target: f64,
        width_mm: f64,
        layer: &str,
        mode: &str,
        tolerance: f64,
        max_iterations: usize,
    ) -> PhysResult<f64> {
        if zdiff_target <= 0.0 {
            return Err(ValueError(format!(
                "Target Zdiff must be positive, got {}",
                py(zdiff_target)
            )));
        }
        if width_mm <= 0.0 {
            return Err(ValueError(format!(
                "Width must be positive, got {}",
                py(width_mm)
            )));
        }
        let ms = match mode {
            "auto" => self.stackup.is_outer_layer(layer),
            "edge_microstrip" => true,
            "edge_stripline" => false,
            _ => {
                return Err(ValueError(format!(
                    "Invalid mode: {mode}. Use 'auto', 'edge_microstrip', or 'edge_stripline'"
                )))
            }
        };
        let h = self.stackup.get_reference_plane_distance(layer)?;
        let mut gap_min = h * 0.05;
        let mut gap_max = h * 5.0;
        let mut z_min = self.calc(ms, width_mm, gap_min, layer)?.zdiff;
        let mut z_max = self.calc(ms, width_mm, gap_max, layer)?.zdiff;
        while z_min > zdiff_target && gap_min > h * 0.001 {
            gap_min /= 2.0;
            z_min = self.calc(ms, width_mm, gap_min, layer)?.zdiff;
        }
        while z_max < zdiff_target && gap_max < h * 50.0 {
            gap_max *= 2.0;
            z_max = self.calc(ms, width_mm, gap_max, layer)?.zdiff;
        }
        for _ in 0..max_iterations {
            let mid = (gap_min + gap_max) / 2.0;
            let z = self.calc(ms, width_mm, mid, layer)?.zdiff;
            if (z - zdiff_target).abs() / zdiff_target < tolerance {
                return Ok(mid);
            }
            if z < zdiff_target {
                gap_min = mid;
            } else {
                gap_max = mid;
            }
        }
        Ok((gap_min + gap_max) / 2.0)
    }
}
