//! Port of `kicad_tools.validate.rules.silkscreen` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `check_all_silkscreen` (stub).
pub fn check_all_silkscreen(
    _pcb: &Pcb,
    _design_rules: &DesignRules,
    _suppress_library: bool,
) -> DRCResults {
    DRCResults::new()
}
