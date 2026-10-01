//! Port of `kicad_tools.validate.rules.courtyard` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

use super::courtyard_waivers::CourtyardWaivers;

/// Upstream `CourtyardOverlapRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct CourtyardOverlapRule {
    pub waivers: Option<CourtyardWaivers>,
}

impl CourtyardOverlapRule {
    pub fn new(waivers: Option<CourtyardWaivers>) -> Self {
        CourtyardOverlapRule { waivers }
    }

    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
