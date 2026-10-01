//! Port of `kicad_tools.validate.rules.placement` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `FootprintOutsideBoardRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct FootprintOutsideBoardRule;

impl FootprintOutsideBoardRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
