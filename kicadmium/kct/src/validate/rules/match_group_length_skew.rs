//! Port of `kicad_tools.validate.rules.match_group_length_skew` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `MatchGroupLengthSkewRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct MatchGroupLengthSkewRule;

impl MatchGroupLengthSkewRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
