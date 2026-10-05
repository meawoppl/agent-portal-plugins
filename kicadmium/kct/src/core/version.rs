//! Shared KiCad file-format version constants (port of `kicad_tools.core.version`).
//!
//! Board, schematic, and symbol-library files are independent format-version
//! streams. The date codes are the conservative floor that loads across the
//! whole KiCad 10.0.x line (KiCad rejects *newer* codes than it knows, e.g.
//! 10.0.3 rejects the 10.0.4 board code 20260206) -- do not bump them to the
//! newest code.

/// `.kicad_pcb` / `.kicad_mod` `(version ...)`.
pub const KICAD_BOARD_FORMAT_VERSION: i64 = 20241229;

/// `.kicad_sch` `(version ...)`.
pub const KICAD_SCH_FORMAT_VERSION: i64 = 20231120;

/// `.kicad_sym` `(version ...)`.
pub const KICAD_SYM_FORMAT_VERSION: i64 = 20231120;

/// `(generator_version ...)` emitted (quoted) by every writer.
pub const KICAD_GENERATOR_VERSION: &str = "10.0";
