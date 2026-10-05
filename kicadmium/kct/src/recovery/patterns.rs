//! Port of `kicad_tools.recovery.patterns`: match failures to known patterns
//! for targeted suggestions.

use super::types::{BlockingElement, FailureAnalysis, FailureCause};
use crate::pyjson::Json;

/// A matched failure pattern with suggestions.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchedPattern {
    pub pattern: String,
    pub suggestion: String,
    pub example: String,
    pub confidence: f64,
}

impl MatchedPattern {
    pub fn to_dict(&self) -> Json {
        let mut d = Json::obj();
        d.set("pattern", self.pattern.as_str());
        d.set("suggestion", self.suggestion.as_str());
        d.set("example", self.example.as_str());
        d.set("confidence", self.confidence);
        d
    }
}

/// Definition of a failure pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatternDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub suggestion: &'static str,
    pub example: &'static str,
}

/// Known failure patterns.
pub const PATTERNS: &[PatternDefinition] = &[
    PatternDefinition {
        name: "bypass_cap_blocking",
        description: "Bypass capacitor blocking routing path",
        suggestion: "Use radial bypass cap placement around IC",
        example: "Place C1-C4 in a ring 2-3mm from U1 VDD pins",
    },
    PatternDefinition {
        name: "connector_bottleneck",
        description: "Routing congestion near connector",
        suggestion: "Fan out traces immediately after connector",
        example: "Route USB signals away from connector before turning",
    },
    PatternDefinition {
        name: "power_plane_crossing",
        description: "Signal crossing split power plane",
        suggestion: "Route signal on single reference plane or add stitching vias",
        example: "Keep CLK trace on layer 2 above continuous GND plane",
    },
    PatternDefinition {
        name: "pin_escape_congestion",
        description: "Too many pins escaping in same direction",
        suggestion: "Use alternating escape directions or add routing layers",
        example: "Route odd pins left, even pins right from BGA",
    },
    PatternDefinition {
        name: "via_farm_blocking",
        description: "Dense via array blocking routing",
        suggestion: "Stagger vias or route through via farm gaps",
        example: "Offset vias by 0.5mm to create routing channels",
    },
    PatternDefinition {
        name: "differential_pair_obstacle",
        description: "Obstacle splitting differential pair",
        suggestion: "Route both traces on same side of obstacle",
        example: "Keep USB_DP and USB_DM together around C5",
    },
    PatternDefinition {
        name: "thermal_via_interference",
        description: "Thermal vias interfering with routing",
        suggestion: "Reduce thermal via density or route around thermal pad",
        example: "Use 4 thermal vias instead of 9 under QFN thermal pad",
    },
    PatternDefinition {
        name: "crystal_isolation",
        description: "Crystal circuit too close to noisy signals",
        suggestion: "Isolate crystal with ground ring and keep signals away",
        example: "Add ground traces around Y1, route digital signals 2mm away",
    },
];

fn ref_upper(b: &BlockingElement) -> Option<String> {
    b.reference
        .as_ref()
        .filter(|r| !r.is_empty())
        .map(|r| r.to_uppercase())
}

/// Matches failures to known patterns.
#[derive(Debug, Clone, Copy, Default)]
pub struct PatternMatcher;

impl PatternMatcher {
    pub fn new() -> Self {
        PatternMatcher
    }

    /// Matching patterns, sorted by confidence (descending, stable).
    pub fn match_patterns(&self, f: &FailureAnalysis) -> Vec<MatchedPattern> {
        let checks: [(&str, bool); 8] = [
            ("bypass_cap_blocking", self.matches_bypass_cap_blocking(f)),
            ("connector_bottleneck", self.matches_connector_bottleneck(f)),
            ("power_plane_crossing", self.matches_power_plane_crossing(f)),
            (
                "pin_escape_congestion",
                self.matches_pin_escape_congestion(f),
            ),
            ("via_farm_blocking", self.matches_via_farm_blocking(f)),
            (
                "differential_pair_obstacle",
                f.root_cause == FailureCause::DifferentialPair,
            ),
            (
                "thermal_via_interference",
                self.matches_thermal_via_interference(f),
            ),
            ("crystal_isolation", self.matches_crystal_isolation(f)),
        ];
        let mut out: Vec<MatchedPattern> = checks
            .iter()
            .filter(|(_, hit)| *hit)
            .map(|(name, _)| self.create_match(name, 1.0))
            .collect();
        out.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
        out
    }

    pub fn get_all_patterns(&self) -> Vec<PatternDefinition> {
        PATTERNS.to_vec()
    }

    pub fn create_match(&self, name: &str, confidence: f64) -> MatchedPattern {
        match PATTERNS.iter().find(|p| p.name == name) {
            Some(p) => MatchedPattern {
                pattern: p.name.into(),
                suggestion: p.suggestion.into(),
                example: p.example.into(),
                confidence,
            },
            None => MatchedPattern {
                pattern: name.into(),
                suggestion: "Unknown pattern".into(),
                example: String::new(),
                confidence: 0.0,
            },
        }
    }

    fn matches_bypass_cap_blocking(&self, f: &FailureAnalysis) -> bool {
        f.root_cause == FailureCause::BlockedPath
            && f.blocking_elements.iter().any(Self::is_bypass_cap)
    }

    fn matches_connector_bottleneck(&self, f: &FailureAnalysis) -> bool {
        f.root_cause == FailureCause::Congestion && f.near_connector()
    }

    fn matches_power_plane_crossing(&self, f: &FailureAnalysis) -> bool {
        f.root_cause == FailureCause::LayerConflict
            && f.blocking_elements.iter().any(|b| b.kind == "zone")
    }

    fn matches_pin_escape_congestion(&self, f: &FailureAnalysis) -> bool {
        f.root_cause == FailureCause::Congestion
            && f.congestion_score >= 0.8
            && f.blocking_elements
                .iter()
                .any(|b| ref_upper(b).is_some_and(|r| r.starts_with('U')))
    }

    fn matches_via_farm_blocking(&self, f: &FailureAnalysis) -> bool {
        f.root_cause == FailureCause::BlockedPath
            && f.blocking_elements
                .iter()
                .filter(|b| b.kind == "via")
                .count()
                >= 3
    }

    fn matches_thermal_via_interference(&self, f: &FailureAnalysis) -> bool {
        if !matches!(
            f.root_cause,
            FailureCause::BlockedPath | FailureCause::Congestion
        ) {
            return false;
        }
        let vias = f.blocking_elements.iter().any(|b| b.kind == "via");
        let thermal = f
            .blocking_elements
            .iter()
            .any(|b| ref_upper(b).is_some_and(|r| r.starts_with('U') || r.starts_with("IC")));
        vias && thermal
    }

    fn matches_crystal_isolation(&self, f: &FailureAnalysis) -> bool {
        f.root_cause == FailureCause::Clearance
            && f.blocking_elements
                .iter()
                .any(|b| ref_upper(b).is_some_and(|r| r.starts_with('Y')))
    }

    fn is_bypass_cap(b: &BlockingElement) -> bool {
        b.kind == "component"
            && b.reference
                .as_ref()
                .is_some_and(|r| r.to_uppercase().starts_with('C'))
    }
}
