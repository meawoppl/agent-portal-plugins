//! NEXT/FEXT crosstalk estimation between parallel traces (port of
//! `physics.crosstalk`).

use super::constants::SPEED_OF_LIGHT;
use super::coupled_lines::CoupledLines;
use super::stackup::Stackup;
use super::{py_float_repr as py, PhysResult, ValueError};

/// Result of crosstalk analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct CrosstalkResult {
    pub next_coefficient: f64,
    pub fext_coefficient: f64,
    pub next_db: f64,
    pub fext_db: f64,
    pub next_percent: f64,
    pub fext_percent: f64,
    pub coupled_length_mm: f64,
    pub saturation_length_mm: f64,
    /// `acceptable`, `marginal`, or `excessive`.
    pub severity: String,
    pub recommendation: Option<String>,
}

impl std::fmt::Display for CrosstalkResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CrosstalkResult(NEXT={:.1}%, FEXT={:.1}%, severity={})",
            self.next_percent, self.fext_percent, self.severity
        )
    }
}

/// Crosstalk estimator using coupled-line coupling coefficients.
#[derive(Debug, Clone)]
pub struct CrosstalkAnalyzer {
    pub stackup: Stackup,
    coupled_lines: CoupledLines,
}

fn pymax(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

fn pymin(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

impl CrosstalkAnalyzer {
    pub fn new(stackup: Stackup) -> Self {
        Self {
            coupled_lines: CoupledLines::new(stackup.clone()),
            stackup,
        }
    }

    /// Estimate crosstalk (upstream defaults `rise_time_ns = 1.0`,
    /// `aggressor_amplitude_v = 3.3`, which is informational only).
    pub fn analyze(
        &self,
        aggressor_width_mm: f64,
        victim_width_mm: f64,
        spacing_mm: f64,
        parallel_length_mm: f64,
        layer: &str,
        rise_time_ns: f64,
    ) -> PhysResult<CrosstalkResult> {
        let checks = [
            (aggressor_width_mm, "Aggressor width"),
            (victim_width_mm, "Victim width"),
            (spacing_mm, "Spacing"),
            (parallel_length_mm, "Parallel length"),
            (rise_time_ns, "Rise time"),
        ];
        for (v, what) in checks {
            if v <= 0.0 {
                return Err(ValueError(format!(
                    "{what} must be positive, got {}",
                    py(v)
                )));
            }
        }
        let avg_width = (aggressor_width_mm + victim_width_mm) / 2.0;
        let coupled = if self.stackup.is_outer_layer(layer) {
            self.coupled_lines
                .edge_coupled_microstrip(avg_width, spacing_mm, layer)?
        } else {
            self.coupled_lines
                .edge_coupled_stripline(avg_width, spacing_mm, layer)?
        };
        let k = coupled.coupling_coefficient;
        let eps_eff = (coupled.epsilon_eff_even + coupled.epsilon_eff_odd) / 2.0;
        let (next_coeff, fext_coeff, lsat) =
            Self::calculate_crosstalk(k, parallel_length_mm, rise_time_ns, eps_eff);
        let next_pct = next_coeff * 100.0;
        let fext_pct = fext_coeff * 100.0;
        let next_db = 20.0 * pymax(next_coeff, 1e-6).log10();
        let fext_db = 20.0 * pymax(fext_coeff, 1e-6).log10();
        let severity = Self::severity(next_pct, fext_pct);
        let recommendation = Self::recommendation(
            severity,
            next_pct,
            fext_pct,
            spacing_mm,
            parallel_length_mm,
            lsat,
        );
        Ok(CrosstalkResult {
            next_coefficient: next_coeff,
            fext_coefficient: fext_coeff,
            next_db,
            fext_db,
            next_percent: next_pct,
            fext_percent: fext_pct,
            coupled_length_mm: parallel_length_mm,
            saturation_length_mm: lsat,
            severity: severity.into(),
            recommendation,
        })
    }

    /// `(next_coefficient, fext_coefficient, saturation_length_mm)`.
    pub fn calculate_crosstalk(
        k: f64,
        length_mm: f64,
        rise_time_ns: f64,
        eps_eff: f64,
    ) -> (f64, f64, f64) {
        let v_p = if eps_eff > 0.0 {
            SPEED_OF_LIGHT / eps_eff.sqrt()
        } else {
            SPEED_OF_LIGHT
        };
        let rise_distance_mm = rise_time_ns * v_p * 1e-6;
        let lsat = rise_distance_mm / 2.0;
        let kb = k / 2.0;
        let next = if length_mm < lsat {
            kb * (length_mm / lsat)
        } else {
            kb
        };
        let kf = if rise_distance_mm > 0.0 {
            2.0 * k * (length_mm / rise_distance_mm)
        } else {
            0.0
        };
        (
            pymax(0.0, pymin(next, 1.0)),
            pymax(0.0, pymin(kf, 1.0)),
            lsat,
        )
    }

    pub fn severity(next_pct: f64, fext_pct: f64) -> &'static str {
        let m = pymax(next_pct, fext_pct);
        if m < 3.0 {
            "acceptable"
        } else if m < 10.0 {
            "marginal"
        } else {
            "excessive"
        }
    }

    fn recommendation(
        severity: &str,
        next_pct: f64,
        fext_pct: f64,
        spacing_mm: f64,
        parallel_length_mm: f64,
        lsat: f64,
    ) -> Option<String> {
        if severity == "acceptable" {
            return None;
        }
        let mut recs = Vec::new();
        if spacing_mm < 0.5 {
            recs.push(format!(
                "Increase spacing to {:.2}mm or more",
                spacing_mm * 2.0
            ));
        }
        if fext_pct > next_pct && parallel_length_mm > lsat {
            recs.push(format!(
                "Reduce parallel run to {:.1}mm",
                parallel_length_mm * 0.5
            ));
        }
        if severity == "excessive" {
            recs.push("Consider routing on different layers with ground between".into());
        }
        if recs.is_empty() {
            Some("Increase trace spacing or reduce parallel coupling length".into())
        } else {
            Some(recs.join("; "))
        }
    }

    /// Minimum spacing keeping NEXT and FEXT below a budget (upstream
    /// defaults `rise_time_ns = 1.0`, `tolerance = 0.1`, `max_iterations = 50`).
    pub fn spacing_for_crosstalk_budget(
        &self,
        max_crosstalk_percent: f64,
        width_mm: f64,
        parallel_length_mm: f64,
        layer: &str,
        rise_time_ns: f64,
    ) -> PhysResult<f64> {
        self.spacing_for_crosstalk_budget_with(
            max_crosstalk_percent,
            width_mm,
            parallel_length_mm,
            layer,
            rise_time_ns,
            0.1,
            50,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spacing_for_crosstalk_budget_with(
        &self,
        max_crosstalk_percent: f64,
        width_mm: f64,
        parallel_length_mm: f64,
        layer: &str,
        rise_time_ns: f64,
        tolerance: f64,
        max_iterations: usize,
    ) -> PhysResult<f64> {
        if max_crosstalk_percent <= 0.0 {
            return Err(ValueError(format!(
                "Max crosstalk must be positive, got {}",
                py(max_crosstalk_percent)
            )));
        }
        if width_mm <= 0.0 {
            return Err(ValueError(format!(
                "Width must be positive, got {}",
                py(width_mm)
            )));
        }
        if parallel_length_mm <= 0.0 {
            return Err(ValueError(format!(
                "Parallel length must be positive, got {}",
                py(parallel_length_mm)
            )));
        }
        let h = self.stackup.get_reference_plane_distance(layer)?;
        let mut spacing_min = h * 0.1;
        let mut spacing_max = h * 10.0;
        let xt = |s: f64| -> PhysResult<f64> {
            let r = self.analyze(
                width_mm,
                width_mm,
                s,
                parallel_length_mm,
                layer,
                rise_time_ns,
            )?;
            Ok(pymax(r.next_percent, r.fext_percent))
        };
        let mut xt_at_max = xt(spacing_max)?;
        while xt_at_max > max_crosstalk_percent && spacing_max < h * 100.0 {
            spacing_max *= 2.0;
            xt_at_max = xt(spacing_max)?;
        }
        if xt_at_max > max_crosstalk_percent {
            return Ok(spacing_max);
        }
        for _ in 0..max_iterations {
            let mid = (spacing_min + spacing_max) / 2.0;
            let x = xt(mid)?;
            if (x - max_crosstalk_percent).abs() / max_crosstalk_percent < tolerance {
                return Ok(mid);
            }
            if x > max_crosstalk_percent {
                spacing_min = mid;
            } else {
                spacing_max = mid;
            }
        }
        Ok((spacing_min + spacing_max) / 2.0)
    }
}
