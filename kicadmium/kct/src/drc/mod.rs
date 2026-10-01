//! DRC report parsing, explanation, manufacturer checks, and the native
//! geometric helpers of `kicad_tools.drc`: `geometric` (kicad-cli
//! reconciliation), `incremental` (cached placement DRC with a spatial
//! index, replacing the C++ backend), `predictive` (move warnings) and
//! `different_net_short` (grid-independent short detection).
//!
//! Not ported here: the repair modules, `fixer`, and `local_rerouter`.

pub mod checker;
pub mod compat;
pub mod different_net_short;
pub mod geometric;
pub mod incremental;
pub mod local_rerouter;
pub mod net_compat;
pub mod predictive;
pub mod repair_clearance;
pub mod repair_drill_clearance;
pub mod repair_silkscreen;
pub mod report;
pub mod severity;
pub mod suggestions;
pub mod violation;
pub mod waivers;

pub use checker::{check_manufacturer_rules, summarize_checks, CheckResult, ManufacturerCheck};
pub use net_compat::resolve_net_atom;
pub use report::{parse_json_report, parse_text_report, DRCReport, MaskCopperAssessment};
pub use severity::Severity;
pub use suggestions::{generate_fix_suggestions, FixAction, FixSuggestion};
pub use violation::{DRCViolation, Location, Suggestion, ViolationCategory, ViolationType};
