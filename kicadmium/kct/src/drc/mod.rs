//! DRC report parsing, explanation, and manufacturer checks (port of the
//! report side of `kicad_tools.drc`).
//!
//! Not ported here (they need `schema.pcb` geometry): `geometric`,
//! `incremental`, `predictive`, `different_net_short`, the repair modules,
//! `fixer`, `local_rerouter`, and the C++ backend.

pub mod checker;
pub mod compat;
pub mod net_compat;
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
