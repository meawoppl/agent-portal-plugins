//! Manufacturing-bundle freshness check for `kct check` (port of
//! `check_cmd._manifest_subcheck`'s verification body).

use std::path::Path;

/// Verify `manifest.json` hashes and that the archived PCB matches `pcb`.
pub fn verify_bundle(_manifest_path: &Path, _pcb_path: &Path) -> Result<(), String> {
    Err("manifest verification not ported yet".into())
}
