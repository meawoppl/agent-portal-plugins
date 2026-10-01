//! Port of `kicad_tools.validate.rules.diffpair_length_skew` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `DiffPairLengthSkewRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct DiffPairLengthSkewRule;

impl DiffPairLengthSkewRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
