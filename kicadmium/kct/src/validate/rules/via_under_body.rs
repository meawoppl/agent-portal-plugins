//! Port of `kicad_tools.validate.rules.via_under_body` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `ViaUnderBodyRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct ViaUnderBodyRule;

impl ViaUnderBodyRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
