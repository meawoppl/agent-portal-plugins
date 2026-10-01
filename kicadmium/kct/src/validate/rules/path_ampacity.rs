//! Port of `kicad_tools.validate.rules.path_ampacity` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

use crate::router::current_paths::CurrentPathSpec;

/// Upstream `PathAmpacityRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct PathAmpacityRule {
    pub specs: Vec<CurrentPathSpec>,
}

impl PathAmpacityRule {
    pub fn new(specs: Vec<CurrentPathSpec>) -> Self {
        PathAmpacityRule { specs }
    }

    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
