//! Port of `kicad_tools.validate.rules.zero_length_segment` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `ZeroLengthSegmentRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct ZeroLengthSegmentRule;

impl ZeroLengthSegmentRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
