//! Propagation delay and timing analysis (port of `physics.timing`).

use super::constants::SPEED_OF_LIGHT;
use super::stackup::Stackup;
use super::transmission_line::{LineMode, TransmissionLine};
use super::{py_float_repr as py, PhysResult, ValueError};

/// Propagation characteristics for a trace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropagationResult {
    pub delay_ps_per_mm: f64,
    pub delay_ns_per_inch: f64,
    pub velocity_m_per_s: f64,
    pub velocity_percent_c: f64,
    /// Total delay for `trace_length_mm` (0 when not analyzed).
    pub total_delay_ns: f64,
    pub trace_length_mm: f64,
}

impl std::fmt::Display for PropagationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.total_delay_ns > 0.0 {
            write!(
                f,
                "PropagationResult(delay={:.2}ps/mm, total={:.3}ns, v={:.1}%c)",
                self.delay_ps_per_mm, self.total_delay_ns, self.velocity_percent_c
            )
        } else {
            write!(
                f,
                "PropagationResult(delay={:.2}ps/mm, v={:.1}%c)",
                self.delay_ps_per_mm, self.velocity_percent_c
            )
        }
    }
}

/// Timing analysis for one net of a length-matched group.
#[derive(Debug, Clone, PartialEq)]
pub struct TimingBudget {
    pub net_name: String,
    pub trace_length_mm: f64,
    pub propagation_delay_ns: f64,
    pub target_delay_ns: Option<f64>,
    pub skew_ns: Option<f64>,
    pub within_budget: bool,
}

impl TimingBudget {
    pub fn new(
        net_name: impl Into<String>,
        trace_length_mm: f64,
        propagation_delay_ns: f64,
    ) -> Self {
        Self {
            net_name: net_name.into(),
            trace_length_mm,
            propagation_delay_ns,
            target_delay_ns: None,
            skew_ns: None,
            within_budget: true,
        }
    }
}

impl std::fmt::Display for TimingBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.skew_ns {
            Some(skew) => write!(
                f,
                "TimingBudget({}: {:.3}ns, skew={:.1}ps [{}])",
                self.net_name,
                self.propagation_delay_ns,
                skew * 1000.0,
                if self.within_budget { "OK" } else { "FAIL" }
            ),
            None => write!(
                f,
                "TimingBudget({}: {:.3}ns)",
                self.net_name, self.propagation_delay_ns
            ),
        }
    }
}

/// Intra-pair skew analysis for a differential pair.
#[derive(Debug, Clone, PartialEq)]
pub struct DifferentialPairSkew {
    pub positive_net: String,
    pub negative_net: String,
    pub p_delay_ns: f64,
    pub n_delay_ns: f64,
    pub skew_ps: f64,
    pub max_skew_ps: f64,
    pub within_spec: bool,
}

impl DifferentialPairSkew {
    pub fn p_longer(&self) -> bool {
        self.p_delay_ns > self.n_delay_ns
    }

    pub fn recommendation(&self) -> Option<String> {
        if self.within_spec {
            return None;
        }
        let longer = if self.p_longer() { "P" } else { "N" };
        Some(format!(
            "Reduce {longer} net length by ~{:.1}mm to meet spec",
            self.skew_ps / 6.0
        ))
    }
}

impl std::fmt::Display for DifferentialPairSkew {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DifferentialPairSkew({}/{}: skew={:.1}ps [{}])",
            self.positive_net,
            self.negative_net,
            self.skew_ps,
            if self.within_spec { "OK" } else { "FAIL" }
        )
    }
}

/// Serpentine (meander) parameters for delay matching.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SerpentineParameters {
    pub extra_length_mm: f64,
    pub meander_amplitude_mm: f64,
    pub meander_pitch_mm: f64,
    pub num_meanders: f64,
}

/// Propagation delay and timing analyzer. All `mode` arguments accept
/// `auto` (upstream default), `microstrip`, or `stripline`.
#[derive(Debug, Clone)]
pub struct TimingAnalyzer {
    pub stackup: Stackup,
    pub tl: TransmissionLine,
}

impl TimingAnalyzer {
    pub fn new(stackup: Stackup) -> Self {
        Self {
            tl: TransmissionLine::new(stackup.clone()),
            stackup,
        }
    }

    pub fn propagation_delay(
        &self,
        width_mm: f64,
        layer: &str,
        mode: &str,
    ) -> PhysResult<PropagationResult> {
        if width_mm <= 0.0 {
            return Err(ValueError(format!(
                "Trace width must be positive, got {}",
                py(width_mm)
            )));
        }
        let ms = match LineMode::parse(mode)? {
            LineMode::Auto => self.stackup.is_outer_layer(layer),
            LineMode::Microstrip => true,
            LineMode::Stripline => false,
        };
        let r = if ms {
            self.tl.microstrip(width_mm, layer, 1.0)?
        } else {
            self.tl.stripline(width_mm, layer, 1.0)?
        };
        Ok(PropagationResult {
            delay_ps_per_mm: r.propagation_delay_ps_per_mm(),
            delay_ns_per_inch: r.propagation_delay_ns_per_inch(),
            velocity_m_per_s: r.phase_velocity,
            velocity_percent_c: (r.phase_velocity / SPEED_OF_LIGHT) * 100.0,
            total_delay_ns: 0.0,
            trace_length_mm: 0.0,
        })
    }

    pub fn analyze_trace(
        &self,
        trace_length_mm: f64,
        width_mm: f64,
        layer: &str,
        mode: &str,
    ) -> PhysResult<PropagationResult> {
        if trace_length_mm <= 0.0 {
            return Err(ValueError(format!(
                "Trace length must be positive, got {}",
                py(trace_length_mm)
            )));
        }
        let base = self.propagation_delay(width_mm, layer, mode)?;
        Ok(PropagationResult {
            total_delay_ns: base.delay_ps_per_mm * trace_length_mm / 1000.0,
            trace_length_mm,
            ..base
        })
    }

    pub fn length_for_delay(
        &self,
        target_delay_ns: f64,
        width_mm: f64,
        layer: &str,
        mode: &str,
    ) -> PhysResult<f64> {
        if target_delay_ns <= 0.0 {
            return Err(ValueError(format!(
                "Target delay must be positive, got {}",
                py(target_delay_ns)
            )));
        }
        let prop = self.propagation_delay(width_mm, layer, mode)?;
        Ok(target_delay_ns * 1000.0 / prop.delay_ps_per_mm)
    }

    /// Length matching for `(name, length_mm)` nets against their average
    /// delay (upstream default `max_skew_ns = 0.1`).
    pub fn analyze_length_matching(
        &self,
        nets: &[(&str, f64)],
        width_mm: f64,
        layer: &str,
        max_skew_ns: f64,
        mode: &str,
    ) -> PhysResult<Vec<TimingBudget>> {
        if nets.is_empty() {
            return Ok(Vec::new());
        }
        let prop = self.propagation_delay(width_mm, layer, mode)?;
        let mut budgets: Vec<TimingBudget> = nets
            .iter()
            .map(|(name, length)| {
                TimingBudget::new(*name, *length, prop.delay_ps_per_mm * length / 1000.0)
            })
            .collect();
        let target =
            super::py_sum(budgets.iter().map(|b| b.propagation_delay_ns)) / budgets.len() as f64;
        for b in &mut budgets {
            let skew = b.propagation_delay_ns - target;
            b.target_delay_ns = Some(target);
            b.skew_ns = Some(skew);
            b.within_budget = skew.abs() <= max_skew_ns;
        }
        Ok(budgets)
    }

    /// Intra-pair skew (upstream defaults: nets `D+`/`D-`,
    /// `max_skew_ps = 10.0`, `mode = "auto"`).
    #[allow(clippy::too_many_arguments)]
    pub fn analyze_differential_pair_skew(
        &self,
        positive_length_mm: f64,
        negative_length_mm: f64,
        width_mm: f64,
        layer: &str,
        positive_net: &str,
        negative_net: &str,
        max_skew_ps: f64,
        mode: &str,
    ) -> PhysResult<DifferentialPairSkew> {
        if positive_length_mm <= 0.0 || negative_length_mm <= 0.0 {
            return Err(ValueError::new("Trace lengths must be positive"));
        }
        let prop = self.propagation_delay(width_mm, layer, mode)?;
        let p = prop.delay_ps_per_mm * positive_length_mm / 1000.0;
        let n = prop.delay_ps_per_mm * negative_length_mm / 1000.0;
        let skew_ps = (p - n).abs() * 1000.0;
        Ok(DifferentialPairSkew {
            positive_net: positive_net.into(),
            negative_net: negative_net.into(),
            p_delay_ns: p,
            n_delay_ns: n,
            skew_ps,
            max_skew_ps,
            within_spec: skew_ps <= max_skew_ps,
        })
    }

    pub fn length_difference_for_skew(
        &self,
        max_skew_ps: f64,
        width_mm: f64,
        layer: &str,
        mode: &str,
    ) -> PhysResult<f64> {
        if max_skew_ps <= 0.0 {
            return Err(ValueError(format!(
                "Max skew must be positive, got {}",
                py(max_skew_ps)
            )));
        }
        let prop = self.propagation_delay(width_mm, layer, mode)?;
        Ok(max_skew_ps / prop.delay_ps_per_mm)
    }

    pub fn serpentine_parameters(
        &self,
        target_extra_delay_ns: f64,
        width_mm: f64,
        spacing_mm: f64,
        layer: &str,
        mode: &str,
    ) -> PhysResult<SerpentineParameters> {
        if target_extra_delay_ns <= 0.0 {
            return Err(ValueError(format!(
                "Target extra delay must be positive, got {}",
                py(target_extra_delay_ns)
            )));
        }
        if spacing_mm <= 0.0 {
            return Err(ValueError(format!(
                "Spacing must be positive, got {}",
                py(spacing_mm)
            )));
        }
        let extra = self.length_for_delay(target_extra_delay_ns, width_mm, layer, mode)?;
        let a = 3.0 * width_mm;
        let b = spacing_mm + width_mm;
        let min_amplitude = if b > a { b } else { a };
        let amplitude = min_amplitude * 1.5;
        let pitch = 2.0 * spacing_mm + width_mm;
        Ok(SerpentineParameters {
            extra_length_mm: extra,
            meander_amplitude_mm: amplitude,
            meander_pitch_mm: pitch,
            num_meanders: extra / (2.0 * amplitude),
        })
    }
}
