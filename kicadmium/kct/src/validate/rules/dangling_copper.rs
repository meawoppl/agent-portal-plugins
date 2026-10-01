//! Port of `kicad_tools.validate.rules.dangling_copper` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `DanglingCopperRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct DanglingCopperRule;

impl DanglingCopperRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
