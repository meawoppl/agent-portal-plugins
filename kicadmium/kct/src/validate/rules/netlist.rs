//! Port of `kicad_tools.validate.rules.netlist` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `NetlistRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct NetlistRule;

impl NetlistRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
