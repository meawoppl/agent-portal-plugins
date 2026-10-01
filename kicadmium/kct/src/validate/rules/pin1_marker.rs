//! Port of `kicad_tools.validate.rules.pin1_marker` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `Pin1MarkerRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct Pin1MarkerRule;

impl Pin1MarkerRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
