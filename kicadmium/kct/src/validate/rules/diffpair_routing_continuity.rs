//! Port of `kicad_tools.validate.rules.diffpair_routing_continuity` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `DiffPairRoutingContinuityRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct DiffPairRoutingContinuityRule;

impl DiffPairRoutingContinuityRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
