//! Schematic field-geometry lint (port of
//! `kicad_tools.validate.rules.schematic_fields`) -- STUB.

use std::path::Path;

use crate::validate::violations::DRCResults;

pub const DEFAULT_SCH_FIELD_THRESHOLD_MM: f64 = 15.0;

pub fn check_schematic_fields(_sch_path: &Path, _threshold_mm: f64) -> DRCResults {
    DRCResults::new()
}
