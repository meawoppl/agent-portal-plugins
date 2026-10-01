//! Port of `kicad_tools.validate.rules.solder_mask` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `SolderMaskPadRules` (stub).
#[derive(Debug, Clone, Default)]
pub struct SolderMaskPadRules;

impl SolderMaskPadRules {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
