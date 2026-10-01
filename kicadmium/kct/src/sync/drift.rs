//! Schematic/PCB drift banner and netlist-sync gate (port of
//! `kicad_tools.sync.drift` as used by `kct check`) -- STUB.

use std::path::Path;

pub fn emit_drift_banner(_pcb_path: &Path, _schematic: Option<&str>) {}

pub fn run_netlist_sync_gate(
    _pcb_path: &Path,
    _schematic: Option<&str>,
    _strict: bool,
) -> anyhow::Result<i32> {
    Ok(0)
}
