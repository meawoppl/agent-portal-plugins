//! Port of `kicad_tools.validate.rules.clearance` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `ClearanceRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct ClearanceRule;

impl ClearanceRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}

/// Upstream `SegmentZoneClearanceRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct SegmentZoneClearanceRule;

impl SegmentZoneClearanceRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}

/// Upstream `ViaZoneClearanceRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct ViaZoneClearanceRule;

impl ViaZoneClearanceRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
