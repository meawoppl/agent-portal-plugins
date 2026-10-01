//! Port of `kicad_tools.silkscreen._silk_defaults`: shared numeric defaults
//! for silkscreen clearance/search geometry.

/// Default silk-to-obstacle clearance (mm) for `place_refs`.
pub const DEFAULT_CLEARANCE_MM: f64 = 0.15;
/// Candidate search radius (mm) for `place_refs`.
pub const DEFAULT_MAX_OFFSET_MM: f64 = 8.0;
/// Candidate ring step (mm) for `place_refs`.
pub const DEFAULT_STEP_MM: f64 = 0.25;
/// Minimum silk-to-board-edge clearance (mm) for `check_silk_edge_clearance`.
pub const SILK_EDGE_CLEARANCE_MM: f64 = crate::validate::rules::silkscreen::SILK_EDGE_CLEARANCE_MM;
