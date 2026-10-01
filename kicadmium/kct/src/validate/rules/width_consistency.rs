//! Port of `kicad_tools.validate.rules.width_consistency` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `WidthConsistencyRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct WidthConsistencyRule;

impl WidthConsistencyRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
