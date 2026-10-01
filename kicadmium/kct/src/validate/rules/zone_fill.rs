//! Port of `kicad_tools.validate.rules.zone_fill` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `IsolatedCopperRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct IsolatedCopperRule;

impl IsolatedCopperRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}

/// Upstream `ZoneFillRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct ZoneFillRule;

impl ZoneFillRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
