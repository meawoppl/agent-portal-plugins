//! Manufacturer design-rule checking for DRC reports (port of
//! `kicad_tools.drc.checker`).

use anyhow::{Context, Result};

use super::report::DRCReport;
use super::violation::{DRCViolation, ViolationType};
use crate::jobj;
use crate::manufacturers::{self, DesignRules};
use crate::pyjson::Json;

/// Result of a manufacturer rule check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckResult {
    Pass,
    Fail,
    Warning,
    Unknown,
}

impl CheckResult {
    pub fn value(self) -> &'static str {
        match self {
            CheckResult::Pass => "pass",
            CheckResult::Fail => "fail",
            CheckResult::Warning => "warning",
            CheckResult::Unknown => "unknown",
        }
    }
}

/// Result of checking one violation against manufacturer rules.
#[derive(Debug, Clone, PartialEq)]
pub struct ManufacturerCheck {
    pub violation: DRCViolation,
    pub result: CheckResult,
    pub message: String,
    pub manufacturer_id: String,
    pub rule_name: String,
    pub manufacturer_limit: Option<f64>,
    pub actual_value: Option<f64>,
}

impl ManufacturerCheck {
    pub fn is_compatible(&self) -> bool {
        matches!(self.result, CheckResult::Pass | CheckResult::Warning)
    }
}

impl std::fmt::Display for ManufacturerCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = self.result.value().to_uppercase();
        match (self.manufacturer_limit, self.actual_value) {
            (Some(l), Some(a)) => write!(
                f,
                "{tag}: {} (limit: {l:.4}mm, actual: {a:.4}mm)",
                self.message
            ),
            _ => write!(f, "{tag}: {}", self.message),
        }
    }
}

/// `ManufacturerProfile.get_design_rules(layers, copper_oz)`: exact key,
/// then `<layers>layer_1oz`, then `2layer_1oz`, then the first entry.
pub fn get_design_rules(manufacturer_id: &str, layers: i64, copper_oz: f64) -> Result<DesignRules> {
    let all = manufacturers::design_rules(manufacturer_id)?;
    let exact = format!("{layers}layer_{copper_oz:.0}oz");
    let one_oz = format!("{layers}layer_1oz");
    all.get(&exact)
        .or_else(|| all.get(&one_oz))
        .or_else(|| all.get("2layer_1oz"))
        .or_else(|| all.values().next())
        .cloned()
        .context("manufacturer profile has no design rules")
}

/// Check every violation in `report` against a manufacturer's limits.
/// Violations the checker cannot judge are omitted.
pub fn check_manufacturer_rules(
    report: &DRCReport,
    manufacturer_id: &str,
    layers: i64,
    copper_oz: f64,
) -> Vec<ManufacturerCheck> {
    let Ok(profile) = manufacturers::profile(manufacturer_id) else {
        return report
            .violations
            .iter()
            .map(|v| ManufacturerCheck {
                violation: v.clone(),
                result: CheckResult::Unknown,
                message: format!("Unknown manufacturer: {manufacturer_id}"),
                manufacturer_id: manufacturer_id.to_string(),
                rule_name: "unknown".into(),
                manufacturer_limit: None,
                actual_value: None,
            })
            .collect();
    };
    let Ok(rules) = get_design_rules(profile.id, layers, copper_oz) else {
        return Vec::new();
    };
    report
        .violations
        .iter()
        .filter_map(|v| check_violation(v, profile.id, profile.name, &rules))
        .collect()
}

fn check(
    v: &DRCViolation,
    mfr_id: &str,
    result: CheckResult,
    message: String,
    rule_name: &str,
    limit: Option<f64>,
    actual: Option<f64>,
) -> Option<ManufacturerCheck> {
    Some(ManufacturerCheck {
        violation: v.clone(),
        result,
        message,
        manufacturer_id: mfr_id.to_string(),
        rule_name: rule_name.to_string(),
        manufacturer_limit: limit,
        actual_value: actual,
    })
}

fn pass_fail(actual: f64, limit: f64) -> CheckResult {
    if actual >= limit {
        CheckResult::Pass
    } else {
        CheckResult::Fail
    }
}

fn check_violation(
    v: &DRCViolation,
    mfr_id: &str,
    name: &str,
    rules: &DesignRules,
) -> Option<ManufacturerCheck> {
    use ViolationType::*;
    let actual = v.actual_value_mm;
    match v.vtype {
        CLEARANCE => {
            let lim = rules.min_clearance_mm;
            return match actual {
                Some(a) => check(
                    v,
                    mfr_id,
                    pass_fail(a, lim),
                    format!("Clearance {a:.4}mm vs {name} min {lim:.4}mm"),
                    "min_clearance",
                    Some(lim),
                    Some(a),
                ),
                None => check(
                    v,
                    mfr_id,
                    CheckResult::Warning,
                    format!("Clearance violation - {name} requires {lim:.4}mm minimum"),
                    "min_clearance",
                    Some(lim),
                    None,
                ),
            };
        }
        TRACK_WIDTH => {
            let lim = rules.min_trace_width_mm;
            return match actual {
                Some(a) => check(
                    v,
                    mfr_id,
                    pass_fail(a, lim),
                    format!("Track width {a:.4}mm vs {name} min {lim:.4}mm"),
                    "min_trace_width",
                    Some(lim),
                    Some(a),
                ),
                None => check(
                    v,
                    mfr_id,
                    CheckResult::Warning,
                    format!("Track width violation - {name} requires {lim:.4}mm minimum"),
                    "min_trace_width",
                    Some(lim),
                    None,
                ),
            };
        }
        COPPER_EDGE_CLEARANCE => {
            let lim = rules.min_copper_to_edge_mm;
            return match actual {
                Some(a) => check(
                    v,
                    mfr_id,
                    pass_fail(a, lim),
                    format!("Edge clearance {a:.4}mm vs {name} min {lim:.3}mm"),
                    "min_copper_to_edge",
                    Some(lim),
                    Some(a),
                ),
                None => check(
                    v,
                    mfr_id,
                    CheckResult::Warning,
                    format!("Edge clearance violation - {name} requires {lim:.3}mm minimum"),
                    "min_copper_to_edge",
                    Some(lim),
                    None,
                ),
            };
        }
        VIA_ANNULAR_WIDTH => {
            let lim = rules.min_annular_ring_mm;
            return match actual {
                Some(a) => check(
                    v,
                    mfr_id,
                    pass_fail(a, lim),
                    format!("Annular ring {a:.4}mm vs {name} min {lim:.3}mm"),
                    "min_annular_ring",
                    Some(lim),
                    Some(a),
                ),
                None => check(
                    v,
                    mfr_id,
                    CheckResult::Warning,
                    format!("Annular ring violation - {name} requires {lim:.3}mm minimum"),
                    "min_annular_ring",
                    Some(lim),
                    None,
                ),
            };
        }
        DRILL_HOLE_TOO_SMALL => {
            let lim = rules.min_via_drill_mm;
            return match actual {
                Some(a) => check(
                    v,
                    mfr_id,
                    pass_fail(a, lim),
                    format!("Drill {a:.3}mm vs {name} min {lim:.2}mm"),
                    "min_via_drill",
                    Some(lim),
                    Some(a),
                ),
                None => check(
                    v,
                    mfr_id,
                    CheckResult::Warning,
                    format!("Drill size violation - {name} requires {lim:.2}mm minimum"),
                    "min_via_drill",
                    Some(lim),
                    None,
                ),
            };
        }
        _ => {}
    }
    if v.vtype == CLEARANCE_PAD_PAD && v.is_same_component_pad_clearance() {
        return check(
            v,
            mfr_id,
            CheckResult::Pass,
            "Same-component pad-pad clearance - inherent to IC footprint geometry (adjacent pins within a single package)".into(),
            "pad_pad_clearance",
            None,
            None,
        );
    }
    if v.vtype == SOLDER_MASK_BRIDGE {
        let lim = rules.min_solder_mask_dam_mm;
        if v.is_fine_pitch_inherent(lim) {
            let a = actual.unwrap_or_default();
            return check(
                v,
                mfr_id,
                CheckResult::Pass,
                format!(
                    "Fine-pitch IC solder mask bridge - inherent to IC footprint (dam {a:.4}mm < {name} min {lim:.3}mm, acceptable for fine-pitch ICs)"
                ),
                "solder_mask_dam",
                Some(lim),
                actual,
            );
        }
        return match actual {
            Some(a) => check(
                v,
                mfr_id,
                pass_fail(a, lim),
                format!("Solder mask dam {a:.4}mm vs {name} min {lim:.3}mm"),
                "solder_mask_dam",
                Some(lim),
                Some(a),
            ),
            None => check(
                v,
                mfr_id,
                CheckResult::Warning,
                format!("Solder mask bridge - {name} requires {lim:.3}mm minimum dam"),
                "solder_mask_dam",
                Some(lim),
                None,
            ),
        };
    }
    if matches!(v.vtype, UNCONNECTED_ITEMS | SHORTING_ITEMS) {
        return check(
            v,
            mfr_id,
            CheckResult::Fail,
            "Critical connection issue - must fix before manufacturing".into(),
            "connection",
            None,
            None,
        );
    }
    None
}

/// Summarize check results (`summarize_checks`).
pub fn summarize_checks(checks: &[ManufacturerCheck]) -> Json {
    let count = |r: CheckResult| checks.iter().filter(|c| c.result == r).count();
    let (pass, fail, warning, unknown) = (
        count(CheckResult::Pass),
        count(CheckResult::Fail),
        count(CheckResult::Warning),
        count(CheckResult::Unknown),
    );
    let mut by_rule = Json::obj();
    for c in checks {
        if !by_rule.contains_key(&c.rule_name) {
            by_rule.set(
                &c.rule_name,
                jobj! {"pass" => 0, "fail" => 0, "warning" => 0},
            );
        }
        let entry = by_rule.get_mut(&c.rule_name).expect("entry");
        let n = entry
            .get(c.result.value())
            .and_then(Json::as_i64)
            .unwrap_or(0);
        entry.set(c.result.value(), n + 1);
    }
    jobj! {
        "total" => checks.len(),
        "pass" => pass,
        "fail" => fail,
        "warning" => warning,
        "unknown" => unknown,
        "compatible" => pass + warning,
        "by_rule" => by_rule,
    }
}
