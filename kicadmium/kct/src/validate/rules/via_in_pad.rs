//! Port of `kicad_tools.validate.rules.via_in_pad` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `ViaInPadRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct ViaInPadRule;

impl ViaInPadRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
