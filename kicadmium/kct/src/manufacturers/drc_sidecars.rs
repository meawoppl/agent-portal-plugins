//! DRC-constraint sidecar emission for `kct check --emit-dru` -- STUB.

use std::path::{Path, PathBuf};

use super::DesignRules;

pub fn write_drc_sidecars(
    _pcb_path: &Path,
    _rules: &DesignRules,
    _manufacturer_id: &str,
    _layers: i64,
    _copper_oz: f64,
    _emit_both: bool,
) -> Result<Vec<PathBuf>, String> {
    Err("DRC-constraint sidecar generation is not ported yet".into())
}
