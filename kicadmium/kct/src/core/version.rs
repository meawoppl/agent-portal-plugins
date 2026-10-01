//! Port of `kicad_tools.core.version`: format stamps emitted by writers.

/// `(version N)` stamped on generated boards (loads across KiCad 10.0.x).
pub const KICAD_BOARD_FORMAT_VERSION: i64 = 20241229;
/// `(version N)` stamped on generated schematics.
pub const KICAD_SCH_FORMAT_VERSION: i64 = 20231120;
/// `(version N)` stamped on generated symbol libraries.
pub const KICAD_SYM_FORMAT_VERSION: i64 = 20231120;
/// `(generator_version "X")` shared by all writers.
pub const KICAD_GENERATOR_VERSION: &str = "10.0";
