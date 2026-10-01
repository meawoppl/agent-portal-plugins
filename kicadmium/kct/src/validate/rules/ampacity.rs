//! Port of `kicad_tools.validate.rules.ampacity` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `AmpacityRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct AmpacityRule {
    /// `{net_name: target_ampacity}` (insertion ordered).
    pub specs: Vec<(String, f64)>,
}

impl AmpacityRule {
    pub fn new(specs: Vec<(String, f64)>) -> Self {
        AmpacityRule { specs }
    }

    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::with_rules_checked(1)
    }
}
