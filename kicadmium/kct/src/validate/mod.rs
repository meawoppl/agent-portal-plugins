//! Pure-Rust validation for KiCad designs (port of `kicad_tools.validate`).
//!
//! The DRC side ([`checker::DRCChecker`]) runs manufacturer design rules
//! against a loaded PCB without kicad-cli; `kicad_tools.drc` (here
//! [`crate::drc`]) parses kicad-cli reports instead.

pub mod ampacity_specs;
pub mod checker;
pub mod connectivity;
pub mod consistency;
pub mod diffpair_engagement;
pub mod diffpair_skew;
pub mod doc_drift;
pub mod filters;
pub mod impedance_specs;
pub mod manifest;
pub mod mask_copper;
pub mod match_group_skew;
pub mod rules;
pub mod spatial;
pub mod violations;

pub use checker::{DRCChecker, DRCCheckerOptions};
pub use violations::{DRCResults, DRCViolation};
