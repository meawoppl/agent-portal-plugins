//! Schematic/PCB sync helpers (minimal port of the parts of
//! `kicad_tools.sync` that `kct check` consumes; the `sync` command itself is
//! Wave B).

pub mod discover;
pub mod drift;
