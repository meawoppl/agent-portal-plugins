//! Common PCB design mistake detection with educational explanations (port
//! of `kicad_tools.explain.mistakes`).

use std::fmt;

use crate::jobj;
use crate::pyjson::Json;
use crate::schema::pcb::{Pcb, Segment};

/// Categories of PCB design mistakes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MistakeCategory {
    BypassCap,
    Crystal,
    DifferentialPair,
    PowerTrace,
    Thermal,
    Emi,
    Decoupling,
    Grounding,
    Via,
    Manufacturability,
    Connectivity,
    BomHealth,
}

impl MistakeCategory {
    /// Members in definition order (upstream enum iteration order).
    pub const ALL: [MistakeCategory; 12] = [
        MistakeCategory::BypassCap,
        MistakeCategory::Crystal,
        MistakeCategory::DifferentialPair,
        MistakeCategory::PowerTrace,
        MistakeCategory::Thermal,
        MistakeCategory::Emi,
        MistakeCategory::Decoupling,
        MistakeCategory::Grounding,
        MistakeCategory::Via,
        MistakeCategory::Manufacturability,
        MistakeCategory::Connectivity,
        MistakeCategory::BomHealth,
    ];

    pub fn value(self) -> &'static str {
        match self {
            MistakeCategory::BypassCap => "bypass_capacitor",
            MistakeCategory::Crystal => "crystal_oscillator",
            MistakeCategory::DifferentialPair => "differential_pair",
            MistakeCategory::PowerTrace => "power_trace",
            MistakeCategory::Thermal => "thermal_management",
            MistakeCategory::Emi => "emi_shielding",
            MistakeCategory::Decoupling => "decoupling",
            MistakeCategory::Grounding => "grounding",
            MistakeCategory::Via => "via_placement",
            MistakeCategory::Manufacturability => "manufacturability",
            MistakeCategory::Connectivity => "connectivity",
            MistakeCategory::BomHealth => "bom_health",
        }
    }

    /// Upstream `MistakeCategory(value)`.
    pub fn from_value(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.value() == s)
    }
}

/// A detected PCB design mistake.
#[derive(Debug, Clone, PartialEq)]
pub struct Mistake {
    pub category: MistakeCategory,
    /// "error", "warning", "info".
    pub severity: String,
    pub title: String,
    pub components: Vec<String>,
    pub explanation: String,
    pub fix_suggestion: String,
    pub location: Option<(f64, f64)>,
    pub learn_more_url: Option<String>,
}

impl Mistake {
    pub fn to_dict(&self) -> Json {
        jobj! {
            "category" => self.category.value(),
            "severity" => self.severity.as_str(),
            "title" => self.title.as_str(),
            "components" => &self.components,
            "location" => self.location.map_or(Json::Null, |(x, y)| Json::Arr(vec![x.into(), y.into()])),
            "explanation" => self.explanation.as_str(),
            "fix_suggestion" => self.fix_suggestion.as_str(),
            "learn_more_url" => self.learn_more_url.clone(),
        }
    }

    pub fn format_tree(&self) -> String {
        let mut lines = vec![format!("[{}] {}", self.severity.to_uppercase(), self.title)];
        lines.push(format!("├─ Components: {}", self.components.join(", ")));
        if let Some((x, y)) = self.location {
            lines.push(format!("├─ Location: ({x:.2}, {y:.2}) mm"));
        }
        lines.push(format!("├─ Problem: {}", self.explanation));
        lines.push(format!("├─ Fix: {}", self.fix_suggestion));
        if let Some(url) = self.learn_more_url.as_deref().filter(|u| !u.is_empty()) {
            lines.push(format!("└─ Learn more: {url}"));
        }
        lines.join("\n")
    }
}

/// Raised by a check that could not reach a verdict (missing inputs).
#[derive(Debug, Clone, PartialEq)]
pub struct CheckIncomplete {
    pub reason: String,
}

impl CheckIncomplete {
    pub fn new(reason: impl Into<String>) -> Self {
        CheckIncomplete {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for CheckIncomplete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for CheckIncomplete {}

/// Coverage outcome for a single check run.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckCoverage {
    pub check_name: String,
    pub category: MistakeCategory,
    /// "ran" or "incomplete".
    pub status: String,
    pub reason: Option<String>,
}

impl CheckCoverage {
    pub fn to_dict(&self) -> Json {
        jobj! {
            "check_name" => self.check_name.as_str(),
            "category" => self.category.value(),
            "status" => self.status.as_str(),
            "reason" => self.reason.clone(),
        }
    }
}

/// A mistake detection check (upstream `MistakeCheck` protocol).
pub trait MistakeCheck {
    /// Upstream `type(check).__name__`.
    fn name(&self) -> &'static str;
    fn category(&self) -> MistakeCategory;
    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete>;
}

/// Runs a set of checks against a PCB.
pub struct MistakeDetector {
    checks: Vec<Box<dyn MistakeCheck>>,
}

impl Default for MistakeDetector {
    fn default() -> Self {
        Self::new(None)
    }
}

impl MistakeDetector {
    pub fn new(checks: Option<Vec<Box<dyn MistakeCheck>>>) -> Self {
        MistakeDetector {
            checks: checks.unwrap_or_else(get_default_checks),
        }
    }

    pub fn checks(&self) -> &[Box<dyn MistakeCheck>] {
        &self.checks
    }

    pub fn detect(&self, pcb: &Pcb) -> Vec<Mistake> {
        self.detect_with_coverage(pcb).0
    }

    pub fn detect_with_coverage(&self, pcb: &Pcb) -> (Vec<Mistake>, Vec<CheckCoverage>) {
        Self::run(self.checks.iter().map(|c| c.as_ref()), pcb)
    }

    pub fn detect_by_category(&self, pcb: &Pcb, category: MistakeCategory) -> Vec<Mistake> {
        self.detect_by_category_with_coverage(pcb, category).0
    }

    pub fn detect_by_category_with_coverage(
        &self,
        pcb: &Pcb,
        category: MistakeCategory,
    ) -> (Vec<Mistake>, Vec<CheckCoverage>) {
        Self::run(
            self.checks
                .iter()
                .map(|c| c.as_ref())
                .filter(|c| c.category() == category),
            pcb,
        )
    }

    fn run<'a>(
        checks: impl Iterator<Item = &'a dyn MistakeCheck>,
        pcb: &Pcb,
    ) -> (Vec<Mistake>, Vec<CheckCoverage>) {
        let mut mistakes = Vec::new();
        let mut coverage = Vec::new();
        for check in checks {
            match check.check(pcb) {
                Ok(found) => {
                    mistakes.extend(found);
                    coverage.push(CheckCoverage {
                        check_name: check.name().into(),
                        category: check.category(),
                        status: "ran".into(),
                        reason: None,
                    });
                }
                Err(exc) => coverage.push(CheckCoverage {
                    check_name: check.name().into(),
                    category: check.category(),
                    status: "incomplete".into(),
                    reason: Some(exc.reason),
                }),
            }
        }
        mistakes.sort_by_key(|m| severity_rank(&m.severity));
        (mistakes, coverage)
    }
}

/// `{"error": 0, "warning": 1, "info": 2}.get(severity, 99)`.
pub fn severity_rank(severity: &str) -> u8 {
    match severity {
        "error" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 99,
    }
}

/// Detect all mistakes with the default checks.
pub fn detect_mistakes(pcb: &Pcb) -> Vec<Mistake> {
    MistakeDetector::default().detect(pcb)
}

/// Detect mistakes and report per-check coverage.
pub fn detect_mistakes_with_coverage(pcb: &Pcb) -> (Vec<Mistake>, Vec<CheckCoverage>) {
    MistakeDetector::default().detect_with_coverage(pcb)
}

/// The default set of checks, in upstream order.
pub fn get_default_checks() -> Vec<Box<dyn MistakeCheck>> {
    use super::checks::*;
    vec![
        Box::new(BypassCapDistanceCheck),
        Box::new(CrystalTraceLengthCheck),
        Box::new(CrystalNoiseProximityCheck),
        Box::new(DifferentialPairSkewCheck),
        Box::new(PowerTraceWidthCheck),
        Box::new(ThermalPadConnectionCheck),
        Box::new(ViaInPadCheck),
        Box::new(AcidTrapCheck),
        Box::new(TombstoningRiskCheck),
        Box::new(MissingDecouplingCapCheck),
        Box::new(PullUpResistorCheck),
        Box::new(LedSeriesResistorCheck),
        Box::new(BomFieldHealthCheck),
    ]
}

// ------------------------------------------------------------- utilities

/// Euclidean distance (`math.sqrt(dx ** 2 + dy ** 2)`).
pub fn distance(p1: (f64, f64), p2: (f64, f64)) -> f64 {
    let dx = p2.0 - p1.0;
    let dy = p2.1 - p1.1;
    (dx * dx + dy * dy).sqrt()
}

/// Total length of trace segments.
pub fn trace_length<'a>(segments: impl IntoIterator<Item = &'a Segment>) -> f64 {
    let mut total = 0.0;
    for seg in segments {
        total += distance(seg.start, seg.end);
    }
    total
}

pub fn is_power_net(net_name: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "VCC", "VDD", "VIN", "VOUT", "3V3", "3.3V", "5V", "12V", "VBAT", "VSYS", "+", "PWR",
        "POWER",
    ];
    let up = net_name.to_uppercase();
    PATTERNS.iter().any(|p| up.contains(p))
}

pub fn is_ground_net(net_name: &str) -> bool {
    const PATTERNS: &[&str] = &["GND", "GROUND", "VSS", "AGND", "DGND", "PGND", "SGND"];
    let up = net_name.to_uppercase();
    PATTERNS.iter().any(|p| up.contains(p))
}

pub fn is_bypass_cap(reference: &str, value: &str) -> bool {
    if !reference.to_uppercase().starts_with('C') {
        return false;
    }
    const VALUES: &[&str] = &["100n", "0.1u", "10n", "1u", "4.7u", "10u"];
    let lower = value.to_lowercase().replace([' ', 'f'], "");
    VALUES.iter().any(|v| lower.contains(v))
}

pub fn is_crystal(reference: &str, footprint: &str) -> bool {
    let r = reference.to_uppercase();
    let fp = footprint.to_lowercase();
    r.starts_with('Y')
        || r.starts_with('X')
        || fp.contains("crystal")
        || fp.contains("oscillator")
        || fp.contains("xtal")
}

/// `(is_diff_pair, pair_base_name)`.
pub fn is_differential_pair_net(net_name: &str) -> (bool, Option<String>) {
    let up = net_name.to_uppercase();
    const PATTERNS: &[(&str, &str, &str)] = &[
        ("USB_D+", "USB_D-", "USB_D"),
        ("USB_DP", "USB_DM", "USB_D"),
        ("D+", "D-", "USB"),
        ("DP", "DM", "USB"),
        ("TX+", "TX-", "TX"),
        ("TXP", "TXN", "TX"),
        ("RX+", "RX-", "RX"),
        ("RXP", "RXN", "RX"),
        ("LVDS+", "LVDS-", "LVDS"),
    ];
    for (pos, neg, base) in PATTERNS {
        if up.contains(pos) || up.contains(neg) {
            return (true, Some((*base).to_string()));
        }
    }
    if up.ends_with("_P") || up.ends_with("_N") {
        // `upper_name[:-2]` (both suffix chars are ASCII).
        return (true, Some(up[..up.len() - 2].to_string()));
    }
    (false, None)
}
