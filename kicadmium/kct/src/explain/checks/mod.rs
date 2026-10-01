//! PCB design mistake checks (port of `kicad_tools.explain.checks`).

pub mod acid_trap;
pub mod bom_health;
pub mod bypass;
pub mod connectivity;
pub mod crystal;
pub mod decoupling;
pub mod differential;
pub mod power;
pub mod thermal;
pub mod tombstoning;
pub mod via;

pub use acid_trap::AcidTrapCheck;
pub use bom_health::BomFieldHealthCheck;
pub use bypass::BypassCapDistanceCheck;
pub use connectivity::{LedSeriesResistorCheck, PullUpResistorCheck};
pub use crystal::{CrystalNoiseProximityCheck, CrystalTraceLengthCheck};
pub use decoupling::MissingDecouplingCapCheck;
pub use differential::DifferentialPairSkewCheck;
pub use power::PowerTraceWidthCheck;
pub use thermal::ThermalPadConnectionCheck;
pub use tombstoning::TombstoningRiskCheck;
pub use via::ViaInPadCheck;

use super::mistakes::{Mistake, MistakeCategory};

/// Shorthand constructor used by the checks.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mistake(
    category: MistakeCategory,
    severity: &str,
    title: &str,
    components: Vec<String>,
    location: Option<(f64, f64)>,
    explanation: String,
    fix_suggestion: String,
    learn_more_url: &str,
) -> Mistake {
    Mistake {
        category,
        severity: severity.into(),
        title: title.into(),
        components,
        explanation,
        fix_suggestion,
        location,
        learn_more_url: Some(learn_more_url.into()),
    }
}
