//! Port of `kicad_tools.validate.rules.diffpair_clearance_intra` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `DiffPairClearanceIntraRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct DiffPairClearanceIntraRule;

impl DiffPairClearanceIntraRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
