//! Port of `kicad_tools.validate.rules.physical_gap` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `check_physical_copper_gap` (stub).
pub fn check_physical_copper_gap(_pcb: &Pcb, _design_rules: &DesignRules, _gap_mm: f64) -> DRCResults {
    DRCResults::new()
}
