//! Port of `kicad_tools.validate.rules.connectivity` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `ConnectivityRule` (stub).
#[derive(Debug, Clone)]
pub struct ConnectivityRule {
    pub strict: bool,
}

impl ConnectivityRule {
    pub fn new(strict: bool) -> Self {
        ConnectivityRule { strict }
    }

    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
