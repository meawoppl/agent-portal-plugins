//! `fix-vias --relocate-in-pad` (port of `kicad_tools.cli.relocate_in_pad_vias`).
//!
//! Not ported yet: the connectivity-preserving off-pad slide needs the
//! shapely-based pad/escape geometry and the connectivity validator.

use std::path::Path;

use anyhow::Result;

/// Run the relocation pass; currently reports that it is unavailable.
#[allow(clippy::too_many_arguments)]
pub fn run_relocate_in_pad(
    _mfr: &str,
    _layers: i64,
    _copper: f64,
    _pcb_path: &Path,
    _output: Option<&str>,
    _nets: &[String],
    _dry_run: bool,
    _search_alternatives: bool,
    _quiet: bool,
    _format: &str,
) -> Result<i32> {
    eprintln!(
        "Error: --relocate-in-pad is not yet available in the native kct port \
         (relocate_in_pad_vias); run fix-vias without it to resize vias"
    );
    Ok(1)
}
