//! Port of `kicad_tools.validate.rules.single_pad_net` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `SinglePadNetRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct SinglePadNetRule;

impl SinglePadNetRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
