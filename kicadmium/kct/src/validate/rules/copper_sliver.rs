//! Port of `kicad_tools.validate.rules.copper_sliver` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `CopperSliverRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct CopperSliverRule;

impl CopperSliverRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
