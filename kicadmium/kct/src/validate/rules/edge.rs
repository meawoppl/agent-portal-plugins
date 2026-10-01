//! Port of `kicad_tools.validate.rules.edge` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `EdgeClearanceRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct EdgeClearanceRule;

impl EdgeClearanceRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
