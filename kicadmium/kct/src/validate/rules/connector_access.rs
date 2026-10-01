//! Port of `kicad_tools.validate.rules.connector_access` -- STUB, not yet ported.

use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::DRCResults;

/// Upstream `ConnectorEdgeAccessRule` (stub).
#[derive(Debug, Clone, Default)]
pub struct ConnectorEdgeAccessRule {
    pub emit_inventory: bool,
}

impl ConnectorEdgeAccessRule {
    pub fn new(emit_inventory: bool) -> Self {
        ConnectorEdgeAccessRule { emit_inventory }
    }

    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        DRCResults::new()
    }
}
