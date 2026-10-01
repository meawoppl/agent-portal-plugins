//! Port of `kicad_tools.validate.rules.impedance` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `ImpedanceRule` (stub). `specs: None` means the default
/// (heuristic-suppressed) spec set.
#[derive(Debug, Clone, Default)]
pub struct ImpedanceRule {
    pub specs: Option<Vec<NetImpedanceSpec>>,
}

/// Upstream `NetImpedanceSpec` (stub).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetImpedanceSpec {
    pub net_pattern: String,
    pub target_impedance: f64,
}

impl ImpedanceRule {
    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
