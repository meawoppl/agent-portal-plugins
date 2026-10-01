//! Per-board fabrication-floor overrides (port of
//! `kicad_tools.manufacturers.fabrication_overrides`) -- STUB.

use std::path::Path;

use super::DesignRules;

pub fn resolve_pcb_fabrication_overrides(
    _pcb_path: &Path,
    rules: DesignRules,
    _manufacturer_id: &str,
) -> (DesignRules, Option<String>) {
    (rules, None)
}
