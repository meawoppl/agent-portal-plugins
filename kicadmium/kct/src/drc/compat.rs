//! Adapter from pure-Rust checker results to [`DRCReport`] (port of
//! `kicad_tools.drc.compat`).
//!
//! Upstream converts `validate.violations.DRCResults`; that type is ported
//! separately, so this takes any slice of [`CheckerViolation`] implementors.
// TODO(foundation): implement CheckerViolation for validate::violations::DRCViolation once merged

use std::path::Path;

use super::report::DRCReport;
use super::severity::Severity;
use super::violation::{DRCViolation, Location, ViolationType};

/// Flat checker finding (`validate.violations.DRCViolation` shape).
pub trait CheckerViolation {
    fn rule_id(&self) -> &str;
    fn severity(&self) -> &str;
    fn message(&self) -> &str;
    fn location(&self) -> Option<(f64, f64)>;
    fn layer(&self) -> Option<&str>;
    fn items(&self) -> &[String];
    fn nets(&self) -> &[String];
    fn required_value(&self) -> Option<f64>;
    fn actual_value(&self) -> Option<f64>;
}

/// Convert checker findings into a `DRCReport` for the repair tools.
pub fn drc_results_to_report<V: CheckerViolation>(
    violations: &[V],
    pcb_path: Option<&Path>,
) -> DRCReport {
    let converted = violations
        .iter()
        .map(|v| {
            let mut out = DRCViolation::new(
                ViolationType::from_string(v.rule_id()),
                v.rule_id(),
                Severity::from_string(v.severity()),
                v.message(),
            );
            if let Some((x, y)) = v.location() {
                out.locations
                    .push(Location::new(x, y, v.layer().unwrap_or("")));
            }
            out.items = v.items().to_vec();
            out.nets = v.nets().to_vec();
            out.required_value_mm = v.required_value();
            out.actual_value_mm = v.actual_value();
            out
        })
        .collect();
    let (pcb_name, source_file) = match pcb_path {
        Some(p) => (
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            p.to_string_lossy().into_owned(),
        ),
        None => (String::new(), String::new()),
    };
    DRCReport::new(source_file, pcb_name, converted)
}
