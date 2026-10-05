//! Shared DRC rule constants (port of `kicad_tools.validate.rules.base`).

/// Numerical guard band for DRC clearance comparisons (mm).
///
/// Exists only to suppress IEEE-754 rounding artifacts in distance math, not
/// to model manufacturing precision (issue #3913).
pub const DRC_TOLERANCE: f64 = 1e-4;
