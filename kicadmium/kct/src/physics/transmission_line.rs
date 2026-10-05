//! Transmission line impedance (port of `physics.transmission_line`):
//! Hammerstad-Jensen microstrip, Ghione-Naldi CPWG, and the quasi-static
//! boundary-element stripline from [`super::stripline`].

use std::f64::consts::{E, PI};

use super::constants::{COPPER_CONDUCTIVITY, SPEED_OF_LIGHT};
use super::stackup::Stackup;
use super::stripline::stripline_impedance;
use super::{py_float_repr as py, PhysResult, ValueError};

/// Complete elliptic integral of the first kind `K(k)` via the AGM.
pub fn elliptic_k(k: f64) -> f64 {
    elliptic_k_tol(k, 1e-12)
}

pub fn elliptic_k_tol(k: f64, tolerance: f64) -> f64 {
    let k = k.abs();
    if k >= 1.0 {
        return f64::INFINITY;
    }
    if k == 0.0 {
        return PI / 2.0;
    }
    let mut a = 1.0f64;
    let mut b = (1.0 - k * k).sqrt();
    while (a - b).abs() > tolerance {
        let (na, nb) = ((a + b) / 2.0, (a * b).sqrt());
        a = na;
        b = nb;
    }
    PI / (2.0 * a)
}

/// Result of an impedance calculation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpedanceResult {
    /// Characteristic impedance, ohms.
    pub z0: f64,
    pub epsilon_eff: f64,
    /// Total loss at the reference frequency, dB/m.
    pub loss_db_per_m: f64,
    /// Phase velocity, m/s.
    pub phase_velocity: f64,
}

impl ImpedanceResult {
    pub fn propagation_delay_ps_per_mm(&self) -> f64 {
        if self.phase_velocity <= 0.0 {
            return 0.0;
        }
        1e12 * 0.001 / self.phase_velocity
    }

    pub fn propagation_delay_ns_per_inch(&self) -> f64 {
        self.propagation_delay_ps_per_mm() * 25.4 / 1000.0
    }
}

impl std::fmt::Display for ImpedanceResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ImpedanceResult(z0={:.2}Ω, εeff={:.3}, loss={:.3}dB/m)",
            self.z0, self.epsilon_eff, self.loss_db_per_m
        )
    }
}

/// Line geometry selector for solvers (upstream `mode` strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMode {
    Auto,
    Microstrip,
    Stripline,
}

impl LineMode {
    pub fn parse(mode: &str) -> PhysResult<Self> {
        match mode {
            "auto" => Ok(Self::Auto),
            "microstrip" => Ok(Self::Microstrip),
            "stripline" => Ok(Self::Stripline),
            _ => Err(ValueError(format!(
                "Invalid mode: {mode}. Use 'auto', 'microstrip', or 'stripline'"
            ))),
        }
    }
}

/// Analytical transmission line calculator over a stackup.
#[derive(Debug, Clone)]
pub struct TransmissionLine {
    pub stackup: Stackup,
}

fn positive_width(width_mm: f64) -> PhysResult<()> {
    if width_mm <= 0.0 {
        return Err(ValueError(format!(
            "Trace width must be positive, got {}",
            py(width_mm)
        )));
    }
    Ok(())
}

impl TransmissionLine {
    pub fn new(stackup: Stackup) -> Self {
        Self { stackup }
    }

    /// Microstrip impedance (upstream default `frequency_ghz = 1.0`).
    pub fn microstrip(
        &self,
        width_mm: f64,
        layer: &str,
        frequency_ghz: f64,
    ) -> PhysResult<ImpedanceResult> {
        positive_width(width_mm)?;
        let h = self.stackup.get_reference_plane_distance(layer)?;
        if h <= 0.0 {
            return Err(ValueError(format!(
                "Could not determine dielectric height for layer {layer}"
            )));
        }
        let er = self.stackup.get_dielectric_constant(layer);
        let t = self.stackup.get_copper_thickness(layer);
        let tan_d = self.stackup.get_loss_tangent(layer);
        Ok(Self::microstrip_calc(
            width_mm,
            h,
            er,
            t,
            tan_d,
            frequency_ghz,
        ))
    }

    /// Hammerstad-Jensen microstrip calculation.
    pub fn microstrip_calc(
        w: f64,
        h: f64,
        er: f64,
        t: f64,
        tan_d: f64,
        freq_ghz: f64,
    ) -> ImpedanceResult {
        let w_eff = if t > 0.0 && h > 0.0 {
            let d1 = (t / h).powf(2.0);
            let d2 = (t / (w * PI + 1.1 * t * PI)).powf(2.0);
            if d1 + d2 > 0.0 {
                w + (t / PI) * (4.0 * E / (d1 + d2).sqrt()).ln()
            } else {
                w
            }
        } else {
            w
        };
        let u = w_eff / h;
        let a = 1.0
            + (1.0 / 49.0) * ((u.powf(4.0) + (u / 52.0).powf(2.0)) / (u.powf(4.0) + 0.432)).ln()
            + (1.0 / 18.7) * (1.0 + (u / 18.1).powf(3.0)).ln();
        let b = 0.564 * ((er - 0.9) / (er + 3.0)).powf(0.053);
        let eps_eff = (er + 1.0) / 2.0 + ((er - 1.0) / 2.0) * (1.0 + 10.0 / u).powf(-a * b);
        let f_u = 6.0 + (2.0 * PI - 6.0) * (-((30.666 / u).powf(0.7528))).exp();
        let z0 = (60.0 / eps_eff.sqrt()) * (f_u / u + (1.0 + (2.0 / u).powf(2.0)).sqrt()).ln();
        let v_p = SPEED_OF_LIGHT / eps_eff.sqrt();
        let loss = Self::estimate_microstrip_loss(w, h, er, t, eps_eff, z0, tan_d, freq_ghz);
        ImpedanceResult {
            z0,
            epsilon_eff: eps_eff,
            loss_db_per_m: loss,
            phase_velocity: v_p,
        }
    }

    /// Stripline impedance (upstream default `frequency_ghz = 1.0`).
    pub fn stripline(
        &self,
        width_mm: f64,
        layer: &str,
        frequency_ghz: f64,
    ) -> PhysResult<ImpedanceResult> {
        positive_width(width_mm)?;
        let (h1, h2) = self.stackup.get_stripline_geometry(layer)?;
        if h1 <= 0.0 || h2 <= 0.0 {
            return Err(ValueError(format!(
                "Could not determine geometry for layer {layer}"
            )));
        }
        let er = self.stackup.get_dielectric_constant(layer);
        let t = self.stackup.get_copper_thickness(layer);
        let tan_d = self.stackup.get_loss_tangent(layer);
        Self::stripline_calc(width_mm, h1, h2, er, t, tan_d, frequency_ghz)
    }

    pub fn stripline_calc(
        w: f64,
        h1: f64,
        h2: f64,
        er: f64,
        t: f64,
        tan_d: f64,
        freq_ghz: f64,
    ) -> PhysResult<ImpedanceResult> {
        let b = h1 + h2 + t;
        let z0 = stripline_impedance(w, h1, h2, t, er)?;
        let v_p = SPEED_OF_LIGHT / er.sqrt();
        let loss = Self::estimate_stripline_loss(w, b, er, t, z0, tan_d, freq_ghz);
        Ok(ImpedanceResult {
            z0,
            epsilon_eff: er,
            loss_db_per_m: loss,
            phase_velocity: v_p,
        })
    }

    fn skin_resistance(freq_hz: f64) -> f64 {
        let mu0 = 4.0 * PI * 1e-7;
        (PI * freq_hz * mu0 / COPPER_CONDUCTIVITY).sqrt()
    }

    #[allow(clippy::too_many_arguments)]
    fn estimate_microstrip_loss(
        w: f64,
        _h: f64,
        er: f64,
        _t: f64,
        eps_eff: f64,
        z0: f64,
        tan_d: f64,
        freq_ghz: f64,
    ) -> f64 {
        let freq_hz = freq_ghz * 1e9;
        let rs = Self::skin_resistance(freq_hz);
        let w_m = w / 1000.0;
        let alpha_c_db = if w_m > 0.0 && z0 > 0.0 {
            rs / (z0 * w_m) * 8.686
        } else {
            0.0
        };
        let q = if er > 1.0 {
            (eps_eff - 1.0) / (er - 1.0)
        } else {
            0.5
        };
        let alpha_d = PI * freq_hz * eps_eff.sqrt() * er * q * tan_d / SPEED_OF_LIGHT;
        alpha_c_db + alpha_d * 8.686
    }

    fn estimate_stripline_loss(
        w: f64,
        _b: f64,
        er: f64,
        _t: f64,
        z0: f64,
        tan_d: f64,
        freq_ghz: f64,
    ) -> f64 {
        let freq_hz = freq_ghz * 1e9;
        let rs = Self::skin_resistance(freq_hz);
        let w_m = w / 1000.0;
        let alpha_c_db = if w_m > 0.0 && z0 > 0.0 {
            (2.7e-3 * rs * er.sqrt()) / (z0 * w_m) * 8.686
        } else {
            0.0
        };
        let alpha_d = PI * freq_hz * er.sqrt() * tan_d / SPEED_OF_LIGHT;
        alpha_c_db + alpha_d * 8.686
    }

    fn use_microstrip(&self, layer: &str, mode: LineMode) -> bool {
        match mode {
            LineMode::Auto => self.stackup.is_outer_layer(layer),
            LineMode::Microstrip => true,
            LineMode::Stripline => false,
        }
    }

    fn calc_z0(&self, microstrip: bool, w: f64, layer: &str) -> PhysResult<f64> {
        Ok(if microstrip {
            self.microstrip(w, layer, 1.0)?.z0
        } else {
            self.stripline(w, layer, 1.0)?.z0
        })
    }

    /// Trace width for a target impedance by bisection (upstream defaults:
    /// `mode = "auto"`, `tolerance = 0.01`, `max_iterations = 50`).
    pub fn width_for_impedance(&self, z0_target: f64, layer: &str, mode: &str) -> PhysResult<f64> {
        self.width_for_impedance_with(z0_target, layer, mode, 0.01, 50)
    }

    pub fn width_for_impedance_with(
        &self,
        z0_target: f64,
        layer: &str,
        mode: &str,
        tolerance: f64,
        max_iterations: usize,
    ) -> PhysResult<f64> {
        if z0_target <= 0.0 {
            return Err(ValueError(format!(
                "Target impedance must be positive, got {}",
                py(z0_target)
            )));
        }
        let ms = self.use_microstrip(layer, LineMode::parse(mode)?);
        let h = self.stackup.get_reference_plane_distance(layer)?;
        let mut w_min = h * 0.05;
        let mut w_max = h * 10.0;
        let mut z_at_min = self.calc_z0(ms, w_min, layer)?;
        let mut z_at_max = self.calc_z0(ms, w_max, layer)?;
        while z_at_min < z0_target && w_min > h * 0.001 {
            w_min /= 2.0;
            z_at_min = self.calc_z0(ms, w_min, layer)?;
        }
        while z_at_max > z0_target && w_max < h * 100.0 {
            w_max *= 2.0;
            z_at_max = self.calc_z0(ms, w_max, layer)?;
        }
        for _ in 0..max_iterations {
            let w_mid = (w_min + w_max) / 2.0;
            let z_mid = self.calc_z0(ms, w_mid, layer)?;
            if (z_mid - z0_target).abs() / z0_target < tolerance {
                return Ok(w_mid);
            }
            if z_mid > z0_target {
                w_min = w_mid;
            } else {
                w_max = w_mid;
            }
        }
        Ok((w_min + w_max) / 2.0)
    }

    /// Edge-coupled differential microstrip approximation:
    /// `(single-ended result, Zdiff)`.
    pub fn differential_microstrip(
        &self,
        width_mm: f64,
        spacing_mm: f64,
        layer: &str,
        frequency_ghz: f64,
    ) -> PhysResult<(ImpedanceResult, f64)> {
        let single = self.microstrip(width_mm, layer, frequency_ghz)?;
        let h = self.stackup.get_reference_plane_distance(layer)?;
        let s_over_h = if h > 0.0 { spacing_mm / h } else { 1.0 };
        let k = if s_over_h > 0.0 {
            (-2.0 * s_over_h).exp()
        } else {
            0.5
        };
        let z_diff = 2.0 * single.z0 * (1.0 - 0.347 * k);
        Ok((single, z_diff))
    }

    /// Coplanar waveguide with ground (upstream default `frequency_ghz = 1.0`).
    pub fn cpwg(
        &self,
        width_mm: f64,
        gap_mm: f64,
        layer: &str,
        frequency_ghz: f64,
    ) -> PhysResult<ImpedanceResult> {
        positive_width(width_mm)?;
        if gap_mm <= 0.0 {
            return Err(ValueError(format!(
                "Gap must be positive, got {}",
                py(gap_mm)
            )));
        }
        let h = self.stackup.get_reference_plane_distance(layer)?;
        if h <= 0.0 {
            return Err(ValueError(format!(
                "Could not determine dielectric height for layer {layer}"
            )));
        }
        let er = self.stackup.get_dielectric_constant(layer);
        let t = self.stackup.get_copper_thickness(layer);
        let tan_d = self.stackup.get_loss_tangent(layer);
        Ok(Self::cpwg_calc(
            width_mm,
            gap_mm,
            h,
            er,
            t,
            tan_d,
            frequency_ghz,
        ))
    }

    /// Ghione-Naldi CPWG calculation.
    pub fn cpwg_calc(
        w: f64,
        g: f64,
        h: f64,
        er: f64,
        t: f64,
        tan_d: f64,
        freq_ghz: f64,
    ) -> ImpedanceResult {
        let (w_eff, g_eff) = if t > 0.0 {
            let delta_w = (1.25 * t / PI) * (1.0 + (4.0 * PI * w / t).ln());
            (w + delta_w, (g - delta_w / 2.0).max(g * 0.5))
        } else {
            (w, g)
        };
        let a = w_eff / 2.0;
        let b = w_eff / 2.0 + g_eff;
        let k0 = a / b;
        let k0_prime = (1.0 - k0 * k0).sqrt();
        let sinh_a = (PI * a / (2.0 * h)).sinh();
        let sinh_b = (PI * b / (2.0 * h)).sinh();
        // Python raises OverflowError for huge arguments -> k1 = k0.
        let k1 = if !sinh_a.is_finite() || !sinh_b.is_finite() {
            k0
        } else if sinh_b != 0.0 {
            sinh_a / sinh_b
        } else {
            k0
        };
        let k1_prime = (1.0 - k1 * k1).sqrt();
        let kk0 = elliptic_k(k0);
        let kk0p = elliptic_k(k0_prime);
        let kk1 = elliptic_k(k1);
        let kk1p = elliptic_k(k1_prime);
        let q = if kk1p > 0.0 && kk0 > 0.0 {
            (kk1 * kk0p) / (kk1p * kk0)
        } else {
            0.5
        };
        let eps_eff = 1.0 + (er - 1.0) * q / 2.0;
        let mut z0 = if kk0p > 0.0 && kk1p > 0.0 {
            let sum = kk0 / kk0p + kk1 / kk1p;
            if sum > 0.0 {
                (60.0 * PI / eps_eff.sqrt()) / sum
            } else {
                50.0
            }
        } else {
            50.0
        };
        z0 = z0.clamp(10.0, 200.0);
        let v_p = SPEED_OF_LIGHT / eps_eff.sqrt();
        let loss = Self::estimate_cpwg_loss(w, g, er, eps_eff, z0, tan_d, freq_ghz);
        ImpedanceResult {
            z0,
            epsilon_eff: eps_eff,
            loss_db_per_m: loss,
            phase_velocity: v_p,
        }
    }

    fn estimate_cpwg_loss(
        w: f64,
        g: f64,
        er: f64,
        eps_eff: f64,
        z0: f64,
        tan_d: f64,
        freq_ghz: f64,
    ) -> f64 {
        let freq_hz = freq_ghz * 1e9;
        let rs = Self::skin_resistance(freq_hz);
        let w_eff_m = (w + 2.0 * g) / 1000.0;
        let alpha_c_db = if w_eff_m > 0.0 && z0 > 0.0 {
            1.5 * rs / (z0 * w_eff_m) * 8.686
        } else {
            0.0
        };
        let q = if er > 1.0 {
            (eps_eff - 1.0) / (er - 1.0)
        } else {
            0.5
        };
        let alpha_d = PI * freq_hz * eps_eff.sqrt() * er * q * tan_d / SPEED_OF_LIGHT;
        alpha_c_db + alpha_d * 8.686
    }

    /// CPWG `(width, gap)` for a target impedance. Give at most one of
    /// `width_mm` / `gap_mm`; neither solves a balanced geometry. Upstream
    /// defaults: `tolerance = 0.01`, `max_iterations = 50`.
    pub fn cpwg_geometry_for_impedance(
        &self,
        z0_target: f64,
        layer: &str,
        width_mm: Option<f64>,
        gap_mm: Option<f64>,
    ) -> PhysResult<(f64, f64)> {
        self.cpwg_geometry_for_impedance_with(z0_target, layer, width_mm, gap_mm, 0.01, 50)
    }

    pub fn cpwg_geometry_for_impedance_with(
        &self,
        z0_target: f64,
        layer: &str,
        width_mm: Option<f64>,
        gap_mm: Option<f64>,
        tolerance: f64,
        max_iterations: usize,
    ) -> PhysResult<(f64, f64)> {
        if z0_target <= 0.0 {
            return Err(ValueError(format!(
                "Target impedance must be positive, got {}",
                py(z0_target)
            )));
        }
        if width_mm.is_some() && gap_mm.is_some() {
            return Err(ValueError::new(
                "Specify either width_mm or gap_mm, not both",
            ));
        }
        let h = self.stackup.get_reference_plane_distance(layer)?;
        if let Some(w) = width_mm {
            if w <= 0.0 {
                return Err(ValueError(format!("Width must be positive, got {}", py(w))));
            }
            self.solve_cpwg_gap(z0_target, w, layer, h, tolerance, max_iterations)
        } else if let Some(g) = gap_mm {
            if g <= 0.0 {
                return Err(ValueError(format!("Gap must be positive, got {}", py(g))));
            }
            self.solve_cpwg_width(z0_target, g, layer, h, tolerance, max_iterations)
        } else {
            self.solve_cpwg_balanced(z0_target, layer, h, tolerance, max_iterations)
        }
    }

    fn cz(&self, w: f64, g: f64, layer: &str) -> PhysResult<f64> {
        Ok(self.cpwg(w, g, layer, 1.0)?.z0)
    }

    fn solve_cpwg_gap(
        &self,
        z0_target: f64,
        width_mm: f64,
        layer: &str,
        h: f64,
        tolerance: f64,
        max_iterations: usize,
    ) -> PhysResult<(f64, f64)> {
        let mut g_min = width_mm * 0.1;
        let mut g_max = width_mm * 5.0;
        let mut z_at_min = self.cz(width_mm, g_min, layer)?;
        let mut z_at_max = self.cz(width_mm, g_max, layer)?;
        while z_at_min > z0_target && g_min > h * 0.01 {
            g_min /= 2.0;
            z_at_min = self.cz(width_mm, g_min, layer)?;
        }
        while z_at_max < z0_target && g_max < h * 10.0 {
            g_max *= 2.0;
            z_at_max = self.cz(width_mm, g_max, layer)?;
        }
        for _ in 0..max_iterations {
            let g_mid = (g_min + g_max) / 2.0;
            let z_mid = self.cz(width_mm, g_mid, layer)?;
            if (z_mid - z0_target).abs() / z0_target < tolerance {
                return Ok((width_mm, g_mid));
            }
            if z_mid < z0_target {
                g_min = g_mid;
            } else {
                g_max = g_mid;
            }
        }
        Ok((width_mm, (g_min + g_max) / 2.0))
    }

    fn solve_cpwg_width(
        &self,
        z0_target: f64,
        gap_mm: f64,
        layer: &str,
        h: f64,
        tolerance: f64,
        max_iterations: usize,
    ) -> PhysResult<(f64, f64)> {
        let mut w_min = gap_mm * 0.2;
        let mut w_max = gap_mm * 10.0;
        let mut z_at_min = self.cz(w_min, gap_mm, layer)?;
        let mut z_at_max = self.cz(w_max, gap_mm, layer)?;
        while z_at_min < z0_target && w_min > h * 0.01 {
            w_min /= 2.0;
            z_at_min = self.cz(w_min, gap_mm, layer)?;
        }
        while z_at_max > z0_target && w_max < h * 10.0 {
            w_max *= 2.0;
            z_at_max = self.cz(w_max, gap_mm, layer)?;
        }
        for _ in 0..max_iterations {
            let w_mid = (w_min + w_max) / 2.0;
            let z_mid = self.cz(w_mid, gap_mm, layer)?;
            if (z_mid - z0_target).abs() / z0_target < tolerance {
                return Ok((w_mid, gap_mm));
            }
            if z_mid > z0_target {
                w_min = w_mid;
            } else {
                w_max = w_mid;
            }
        }
        Ok(((w_min + w_max) / 2.0, gap_mm))
    }

    fn solve_cpwg_balanced(
        &self,
        z0_target: f64,
        layer: &str,
        h: f64,
        tolerance: f64,
        max_iterations: usize,
    ) -> PhysResult<(f64, f64)> {
        let mut w = h * 0.5;
        for _ in 0..max_iterations {
            let (_, g) =
                self.solve_cpwg_gap(z0_target, w, layer, h, tolerance, max_iterations / 2)?;
            if (w - g).abs() / w.max(g) < tolerance {
                return Ok((w, g));
            }
            w = (w + g) / 2.0;
        }
        let (_, g) = self.solve_cpwg_gap(z0_target, w, layer, h, tolerance, max_iterations / 2)?;
        Ok((w, g))
    }
}
