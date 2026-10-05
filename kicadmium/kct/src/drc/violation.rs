//! DRC violation data structures (port of `kicad_tools.drc.violation`).

use std::collections::BTreeSet;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

use super::severity::Severity;
use super::suggestions::FixSuggestion;
use crate::pyjson::Json;

/// Root-cause category for DRC violations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViolationCategory {
    Placement,
    Routing,
    Manufacturing,
    Connectivity,
    Cosmetic,
}

impl ViolationCategory {
    pub const ALL: [ViolationCategory; 5] = [
        ViolationCategory::Placement,
        ViolationCategory::Routing,
        ViolationCategory::Manufacturing,
        ViolationCategory::Connectivity,
        ViolationCategory::Cosmetic,
    ];

    pub fn value(self) -> &'static str {
        match self {
            ViolationCategory::Placement => "placement",
            ViolationCategory::Routing => "routing",
            ViolationCategory::Manufacturing => "manufacturing",
            ViolationCategory::Connectivity => "connectivity",
            ViolationCategory::Cosmetic => "cosmetic",
        }
    }

    pub fn from_value(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.value() == s)
    }
}

macro_rules! violation_types {
    ($($variant:ident => $value:literal),* $(,)?) => {
        /// Known DRC violation types.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[allow(non_camel_case_types)]
        pub enum ViolationType { $($variant),* }

        impl ViolationType {
            /// Every member in upstream definition order.
            pub const ALL: &'static [ViolationType] = &[$(ViolationType::$variant),*];

            pub fn value(self) -> &'static str {
                match self { $(ViolationType::$variant => $value),* }
            }
        }
    };
}

violation_types! {
    CLEARANCE => "clearance",
    CLEARANCE_SEGMENT_VIA => "clearance_segment_via",
    CLEARANCE_PAD_SEGMENT => "clearance_pad_segment",
    CLEARANCE_PAD_VIA => "clearance_pad_via",
    CLEARANCE_PAD_PAD => "clearance_pad_pad",
    CLEARANCE_SEGMENT_SEGMENT => "clearance_segment_segment",
    CLEARANCE_SEGMENT_ZONE => "clearance_segment_zone",
    CLEARANCE_VIA_VIA => "clearance_via_via",
    CLEARANCE_NET0_BRIDGE => "clearance_net0_bridge",
    DIFFPAIR_CLEARANCE_INTRA => "diffpair_clearance_intra",
    DIFFPAIR_LENGTH_SKEW => "diffpair_length_skew",
    MATCH_GROUP_LENGTH_SKEW => "match_group_length_skew",
    DIFFPAIR_ROUTING_CONTINUITY => "diffpair_routing_continuity",
    COPPER_EDGE_CLEARANCE => "copper_edge_clearance",
    COPPER_SLIVER => "copper_sliver",
    PHYSICAL_COPPER_GAP => "physical_copper_gap",
    PHYSICAL_COPPER_GAP_INCOMPLETE => "physical_copper_gap_incomplete",
    EDGE_CLEARANCE_TRACE => "edge_clearance_trace",
    EDGE_CLEARANCE_PAD => "edge_clearance_pad",
    EDGE_CLEARANCE_PAD_HOLE => "edge_clearance_pad_hole",
    EDGE_CLEARANCE_VIA => "edge_clearance_via",
    EDGE_CLEARANCE_ZONE => "edge_clearance_zone",
    COURTYARD_OVERLAP => "courtyard_overlap",
    UNCONNECTED_ITEMS => "unconnected_items",
    SHORTING_ITEMS => "shorting_items",
    VIA_HOLE_LARGER_THAN_PAD => "via_hole_larger_than_pad",
    VIA_ANNULAR_WIDTH => "via_annular_width",
    MICRO_VIA_HOLE_TOO_SMALL => "micro_via_hole_too_small",
    VIA_IN_PAD => "via_in_pad",
    TRACK_WIDTH => "track_width",
    TRACK_ANGLE => "track_angle",
    DIMENSION_TRACE_WIDTH => "dimension_trace_width",
    DIMENSION_VIA_DRILL => "dimension_via_drill",
    DIMENSION_VIA_DIAMETER => "dimension_via_diameter",
    DIMENSION_ANNULAR_RING => "dimension_annular_ring",
    HOLE_TO_HOLE_CLEARANCE => "hole_to_hole_clearance",
    DRILL_HOLE_TOO_SMALL => "drill_hole_too_small",
    DRILL_CLEARANCE => "drill_clearance",
    NPTH_HOLE_TOO_SMALL => "npth_hole_too_small",
    HOLE_NEAR_HOLE => "hole_near_hole",
    SILK_OVER_COPPER => "silk_over_copper",
    SILK_EDGE_CLEARANCE => "silk_edge_clearance",
    SILK_OVERLAP => "silk_overlap",
    SILKSCREEN_LINE_WIDTH => "silkscreen_line_width",
    SILKSCREEN_TEXT_HEIGHT => "silkscreen_text_height",
    SILKSCREEN_OVER_PAD => "silkscreen_over_pad",
    SILK_GEOMETRY_UNMODELED => "silk_geometry_unmodeled",
    SOLDER_MASK_BRIDGE => "solder_mask_bridge",
    SOLDER_MASK_CLEARANCE => "solder_mask_clearance",
    MASK_TO_COPPER => "mask_to_copper",
    MIN_PAD_SIZE => "min_pad_size",
    PTH_ANNULAR_RING => "pth_annular_ring",
    IMPEDANCE => "impedance",
    FOOTPRINT_OUTSIDE_BOARD => "footprint_outside_board",
    NET_UNDECLARED => "net_undeclared",
    SINGLE_PAD_NET => "single_pad_net",
    CONNECTIVITY => "connectivity",
    FOOTPRINT => "footprint",
    MALFORMED_OUTLINE => "malformed_outline",
    DUPLICATE_FOOTPRINT => "duplicate_footprint",
    EXTRA_FOOTPRINT => "extra_footprint",
    MISSING_FOOTPRINT => "missing_footprint",
    ZONE_UNFILLED => "zone_unfilled",
    ZONE_FILL_DISABLED => "zone_fill_disabled",
    ZONE_NO_NET => "zone_no_net",
    UNKNOWN => "unknown",
}

/// Explicit alias table checked after the direct enum-value match (see
/// upstream `from_string` for why each entry must stay).
const ALIASES: &[(&str, ViolationType)] = {
    use ViolationType::*;
    &[
        ("clearance_pad_pad", CLEARANCE_PAD_PAD),
        ("clearance_pad_segment", CLEARANCE_PAD_SEGMENT),
        ("clearance_pad_via", CLEARANCE_PAD_VIA),
        ("clearance_segment_segment", CLEARANCE_SEGMENT_SEGMENT),
        ("clearance_segment_via", CLEARANCE_SEGMENT_VIA),
        ("clearance_via_via", CLEARANCE_VIA_VIA),
        ("clearance_net0_bridge", CLEARANCE_NET0_BRIDGE),
        ("clearance_pad_trace", CLEARANCE_PAD_SEGMENT),
        ("clearance_trace_trace", CLEARANCE_SEGMENT_SEGMENT),
        ("clearance_trace_via", CLEARANCE_SEGMENT_VIA),
        ("diffpair_clearance_intra", DIFFPAIR_CLEARANCE_INTRA),
        ("diffpair_length_skew", DIFFPAIR_LENGTH_SKEW),
        ("match_group_length_skew", MATCH_GROUP_LENGTH_SKEW),
        ("diffpair_routing_continuity", DIFFPAIR_ROUTING_CONTINUITY),
        ("edge_clearance_trace", EDGE_CLEARANCE_TRACE),
        ("edge_clearance_pad", EDGE_CLEARANCE_PAD),
        ("edge_clearance_pad_hole", EDGE_CLEARANCE_PAD_HOLE),
        ("edge_clearance_via", EDGE_CLEARANCE_VIA),
        ("edge_clearance_zone", EDGE_CLEARANCE_ZONE),
        ("dimension_trace_width", DIMENSION_TRACE_WIDTH),
        ("dimension_via_drill", DIMENSION_VIA_DRILL),
        ("dimension_via_diameter", DIMENSION_VIA_DIAMETER),
        ("dimension_annular_ring", DIMENSION_ANNULAR_RING),
        ("hole_to_hole_clearance", HOLE_TO_HOLE_CLEARANCE),
        ("dimension_drill_clearance", HOLE_TO_HOLE_CLEARANCE),
        ("silkscreen_line_width", SILKSCREEN_LINE_WIDTH),
        ("silkscreen_text_height", SILKSCREEN_TEXT_HEIGHT),
        ("silkscreen_over_pad", SILKSCREEN_OVER_PAD),
        ("solder_mask_clearance", SOLDER_MASK_CLEARANCE),
        ("min_pad_size", MIN_PAD_SIZE),
        ("pth_annular_ring", PTH_ANNULAR_RING),
        ("impedance", IMPEDANCE),
        ("footprint_outside_board", FOOTPRINT_OUTSIDE_BOARD),
        ("net_undeclared", NET_UNDECLARED),
        ("single_pad_net", SINGLE_PAD_NET),
        ("connectivity", CONNECTIVITY),
        ("zone_unfilled", ZONE_UNFILLED),
        ("zone_fill_disabled", ZONE_FILL_DISABLED),
        ("zone_no_net", ZONE_NO_NET),
        ("via_in_pad", VIA_IN_PAD),
        ("copper_sliver", COPPER_SLIVER),
        ("physical_copper_gap", PHYSICAL_COPPER_GAP),
        (
            "physical_copper_gap_incomplete",
            PHYSICAL_COPPER_GAP_INCOMPLETE,
        ),
    ]
};

impl ViolationType {
    /// Parse a violation type from a kicad-cli type, a validate `rule_id`, or
    /// a free-form description (upstream `ViolationType.from_string`).
    pub fn from_string(s: &str) -> Self {
        use ViolationType::*;
        let s = s.trim().to_lowercase();
        if let Some(t) = Self::ALL.iter().find(|t| t.value() == s) {
            return *t;
        }
        if let Some((_, t)) = ALIASES.iter().find(|(k, _)| *k == s) {
            return *t;
        }
        let has = |needle: &str| s.contains(needle);
        if has("drill") && has("clearance") {
            return DRILL_CLEARANCE;
        }
        if has("clearance") {
            if has("edge") {
                return COPPER_EDGE_CLEARANCE;
            }
            if has("segment") && has("via") {
                return CLEARANCE_SEGMENT_VIA;
            }
            if has("pad") && has("segment") {
                return CLEARANCE_PAD_SEGMENT;
            }
            if has("pad") && has("via") {
                return CLEARANCE_PAD_VIA;
            }
            return CLEARANCE;
        }
        if has("unconnected") {
            return UNCONNECTED_ITEMS;
        }
        if has("short") {
            return SHORTING_ITEMS;
        }
        if has("courtyard") {
            return COURTYARD_OVERLAP;
        }
        if (has("track") || has("trace")) && has("width") {
            return TRACK_WIDTH;
        }
        if has("via") {
            if has("annular") {
                return VIA_ANNULAR_WIDTH;
            }
            if has("hole") && has("larger") {
                return VIA_HOLE_LARGER_THAN_PAD;
            }
            if has("micro") {
                return MICRO_VIA_HOLE_TOO_SMALL;
            }
        }
        if has("drill") {
            return DRILL_HOLE_TOO_SMALL;
        }
        if has("silk") {
            if has("copper") {
                return SILK_OVER_COPPER;
            }
            if has("line") && has("width") {
                return SILKSCREEN_LINE_WIDTH;
            }
            if has("text") && has("height") {
                return SILKSCREEN_TEXT_HEIGHT;
            }
            if has("over") && has("pad") {
                return SILKSCREEN_OVER_PAD;
            }
            return SILK_OVERLAP;
        }
        if has("solder") && has("mask") {
            if has("clearance") {
                return SOLDER_MASK_CLEARANCE;
            }
            return SOLDER_MASK_BRIDGE;
        }
        if has("impedance") {
            return IMPEDANCE;
        }
        if has("footprint") {
            if has("outside") {
                return FOOTPRINT_OUTSIDE_BOARD;
            }
            if has("duplicate") {
                return DUPLICATE_FOOTPRINT;
            }
            if has("extra") {
                return EXTRA_FOOTPRINT;
            }
            if has("missing") {
                return MISSING_FOOTPRINT;
            }
            return FOOTPRINT;
        }
        if has("outline") {
            return MALFORMED_OUTLINE;
        }
        UNKNOWN
    }

    /// Default root-cause category (`_TYPE_CATEGORY_MAP`); `None` for types
    /// the upstream map leaves out (callers fall back to ROUTING).
    pub fn default_category(self) -> Option<ViolationCategory> {
        use ViolationCategory as C;
        use ViolationType::*;
        Some(match self {
            CLEARANCE
            | CLEARANCE_SEGMENT_VIA
            | CLEARANCE_PAD_SEGMENT
            | CLEARANCE_PAD_VIA
            | CLEARANCE_SEGMENT_SEGMENT
            | CLEARANCE_SEGMENT_ZONE
            | CLEARANCE_VIA_VIA
            | DIFFPAIR_CLEARANCE_INTRA
            | DIFFPAIR_ROUTING_CONTINUITY
            | DIFFPAIR_LENGTH_SKEW
            | MATCH_GROUP_LENGTH_SKEW
            | TRACK_WIDTH
            | TRACK_ANGLE
            | DIMENSION_TRACE_WIDTH
            | HOLE_TO_HOLE_CLEARANCE => C::Routing,
            CLEARANCE_PAD_PAD | COURTYARD_OVERLAP | SOLDER_MASK_BRIDGE => C::Placement,
            COPPER_EDGE_CLEARANCE
            | COPPER_SLIVER
            | PHYSICAL_COPPER_GAP
            | PHYSICAL_COPPER_GAP_INCOMPLETE
            | EDGE_CLEARANCE_TRACE
            | EDGE_CLEARANCE_PAD
            | EDGE_CLEARANCE_PAD_HOLE
            | EDGE_CLEARANCE_VIA
            | EDGE_CLEARANCE_ZONE
            | VIA_HOLE_LARGER_THAN_PAD
            | VIA_ANNULAR_WIDTH
            | MICRO_VIA_HOLE_TOO_SMALL
            | VIA_IN_PAD
            | DRILL_HOLE_TOO_SMALL
            | DRILL_CLEARANCE
            | NPTH_HOLE_TOO_SMALL
            | HOLE_NEAR_HOLE
            | DIMENSION_VIA_DRILL
            | DIMENSION_VIA_DIAMETER
            | DIMENSION_ANNULAR_RING
            | SOLDER_MASK_CLEARANCE
            | MIN_PAD_SIZE
            | PTH_ANNULAR_RING
            | IMPEDANCE => C::Manufacturing,
            UNCONNECTED_ITEMS | SHORTING_ITEMS | NET_UNDECLARED | SINGLE_PAD_NET | CONNECTIVITY
            | ZONE_UNFILLED | ZONE_FILL_DISABLED | ZONE_NO_NET => C::Connectivity,
            SILK_OVER_COPPER
            | SILK_OVERLAP
            | SILKSCREEN_LINE_WIDTH
            | SILKSCREEN_TEXT_HEIGHT
            | SILKSCREEN_OVER_PAD => C::Cosmetic,
            FOOTPRINT | MALFORMED_OUTLINE | DUPLICATE_FOOTPRINT | EXTRA_FOOTPRINT
            | MISSING_FOOTPRINT => C::Placement,
            _ => return None,
        })
    }
}

impl fmt::Display for ViolationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.value())
    }
}

/// Position on the PCB.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Location {
    pub x_mm: f64,
    pub y_mm: f64,
    pub layer: String,
    /// Upstream stores these as Python ints when the source JSON did (or
    /// when defaulted to `0`); kept so `to_dict` renders `0` not `0.0`.
    pub(crate) x_is_int: bool,
    pub(crate) y_is_int: bool,
}

static LOC_AT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"@\s*\(\s*([\d.]+)\s*mm\s*,\s*([\d.]+)\s*mm\s*\)").expect("regex")
});
static LOC_JSON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""x"\s*:\s*([\d.]+).*?"y"\s*:\s*([\d.]+)"#).expect("regex"));

impl Location {
    pub fn new(x_mm: f64, y_mm: f64, layer: impl Into<String>) -> Self {
        Location {
            x_mm,
            y_mm,
            layer: layer.into(),
            x_is_int: false,
            y_is_int: false,
        }
    }

    pub(crate) fn from_json(x: Option<&Json>, y: Option<&Json>, layer: &str) -> Self {
        let num = |v: Option<&Json>| match v {
            None => (0.0, true),
            Some(j) => (
                j.as_f64().unwrap_or(0.0),
                j.is_int() || matches!(j, Json::Bool(_)),
            ),
        };
        let (x_mm, x_is_int) = num(x);
        let (y_mm, y_is_int) = num(y);
        Location {
            x_mm,
            y_mm,
            layer: layer.to_string(),
            x_is_int,
            y_is_int,
        }
    }

    /// Parse `@(162.4500 mm, 100.3250 mm)` or `"x": .., "y": ..` text.
    pub fn from_string(s: &str) -> Option<Self> {
        let caps = LOC_AT.captures(s).or_else(|| LOC_JSON.captures(s))?;
        Some(Location::new(
            caps[1].parse().ok()?,
            caps[2].parse().ok()?,
            "",
        ))
    }

    pub fn to_json(&self) -> Json {
        crate::jobj! {
            "x_mm" => Json::num(self.x_mm, self.x_is_int),
            "y_mm" => Json::num(self.y_mm, self.y_is_int),
            "layer" => self.layer.as_str(),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({:.2}, {:.2}) mm", self.x_mm, self.y_mm)?;
        if !self.layer.is_empty() {
            write!(f, " on {}", self.layer)?;
        }
        Ok(())
    }
}

/// Entry in `DRCViolation.suggestions`: upstream holds either the plain
/// strings from `feedback.generate_drc_suggestions` or, after
/// `kct drc --suggest`, a [`FixSuggestion`].
#[derive(Debug, Clone, PartialEq)]
pub enum Suggestion {
    Text(String),
    Fix(FixSuggestion),
}

impl Suggestion {
    /// Python `str(suggestion)`.
    pub fn text(&self) -> &str {
        match self {
            Suggestion::Text(s) => s,
            Suggestion::Fix(f) => &f.description,
        }
    }

    pub fn to_json(&self) -> Json {
        match self {
            Suggestion::Text(s) => Json::Str(s.clone()),
            Suggestion::Fix(f) => f.to_json(),
        }
    }
}

impl fmt::Display for Suggestion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

/// A single DRC violation.
#[derive(Debug, Clone, PartialEq)]
pub struct DRCViolation {
    pub vtype: ViolationType,
    /// Original type string from the report.
    pub type_str: String,
    pub severity: Severity,
    pub message: String,
    pub rule: String,
    pub locations: Vec<Location>,
    pub items: Vec<String>,
    pub nets: Vec<String>,
    pub required_value_mm: Option<f64>,
    pub actual_value_mm: Option<f64>,
    pub suggestions: Vec<Suggestion>,
    pub waived: bool,
    pub waiver_reason: Option<String>,
    pub waiver_issue: Option<String>,
}

impl DRCViolation {
    pub fn new(
        vtype: ViolationType,
        type_str: impl Into<String>,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        DRCViolation {
            vtype,
            type_str: type_str.into(),
            severity,
            message: message.into(),
            rule: String::new(),
            locations: Vec::new(),
            items: Vec::new(),
            nets: Vec::new(),
            required_value_mm: None,
            actual_value_mm: None,
            suggestions: Vec::new(),
            waived: false,
            waiver_reason: None,
            waiver_issue: None,
        }
    }

    /// Root-cause category, special-casing solder-mask bridges by how many
    /// components the items reference.
    pub fn category(&self) -> ViolationCategory {
        if self.vtype == ViolationType::SOLDER_MASK_BRIDGE {
            let refs = extract_component_refs(&self.items);
            if refs.len() == 1 {
                return ViolationCategory::Placement;
            } else if refs.len() >= 2 {
                return ViolationCategory::Routing;
            }
        }
        self.vtype
            .default_category()
            .unwrap_or(ViolationCategory::Routing)
    }

    /// Pad-pad clearance violation between pads of the same component.
    pub fn is_same_component_pad_clearance(&self) -> bool {
        self.vtype == ViolationType::CLEARANCE_PAD_PAD
            && extract_component_refs(&self.items).len() == 1
    }

    /// Solder-mask bridge inherent to a fine-pitch IC footprint.
    pub fn is_fine_pitch_inherent(&self, min_solder_mask_dam_mm: f64) -> bool {
        if self.vtype != ViolationType::SOLDER_MASK_BRIDGE {
            return false;
        }
        if extract_component_refs(&self.items).len() != 1 {
            return false;
        }
        self.actual_value_mm
            .is_some_and(|a| a < min_solder_mask_dam_mm)
    }

    /// Blocking error: severity error and not waived.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error && !self.waived
    }

    pub fn is_waived(&self) -> bool {
        self.waived
    }

    pub fn is_clearance(&self) -> bool {
        use ViolationType::*;
        matches!(
            self.vtype,
            CLEARANCE
                | CLEARANCE_SEGMENT_VIA
                | CLEARANCE_PAD_SEGMENT
                | CLEARANCE_PAD_VIA
                | CLEARANCE_PAD_PAD
                | CLEARANCE_SEGMENT_SEGMENT
                | CLEARANCE_VIA_VIA
                | COPPER_EDGE_CLEARANCE
                | EDGE_CLEARANCE_TRACE
                | EDGE_CLEARANCE_PAD
                | EDGE_CLEARANCE_PAD_HOLE
                | EDGE_CLEARANCE_VIA
                | EDGE_CLEARANCE_ZONE
        )
    }

    pub fn is_connection(&self) -> bool {
        matches!(
            self.vtype,
            ViolationType::UNCONNECTED_ITEMS | ViolationType::SHORTING_ITEMS
        )
    }

    pub fn primary_location(&self) -> Option<&Location> {
        self.locations.first()
    }

    pub fn location_str(&self) -> String {
        self.primary_location()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    pub fn delta_mm(&self) -> Option<f64> {
        Some(self.required_value_mm? - self.actual_value_mm?)
    }

    /// Upstream `to_dict` (same key order).
    pub fn to_json(&self) -> Json {
        let mut d = crate::jobj! {
            "type" => self.vtype.value(),
            "type_str" => self.type_str.as_str(),
            "severity" => self.severity.value(),
            "category" => self.category().value(),
            "message" => self.message.as_str(),
            "rule" => self.rule.as_str(),
            "locations" => Json::Arr(self.locations.iter().map(Location::to_json).collect()),
            "items" => &self.items,
            "nets" => &self.nets,
            "required_value_mm" => self.required_value_mm,
            "actual_value_mm" => self.actual_value_mm,
            "delta_mm" => self.delta_mm(),
        };
        if self.waived {
            d.set("status", "waived");
            d.set("waived", true);
            d.set("waiver_reason", self.waiver_reason.clone());
            d.set("waiver_issue", self.waiver_issue.clone());
        }
        if !self.suggestions.is_empty() {
            d.set(
                "suggestions",
                Json::Arr(self.suggestions.iter().map(Suggestion::to_json).collect()),
            );
        }
        d
    }
}

impl fmt::Display for DRCViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]: {}", self.type_str, self.message)?;
        if let Some(loc) = self.primary_location() {
            write!(f, " at {loc}")?;
        }
        Ok(())
    }
}

static REF_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bof\s+([A-Z]+\d+)\b").expect("regex"));
static FOOTPRINT_REF_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bFootprint\s+([A-Z]+\d+)\b").expect("regex"));

/// Unique component refs from "Pad 1 of U3"-style item strings.
pub fn extract_component_refs(items: &[String]) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    for item in items {
        for c in REF_PATTERN.captures_iter(item) {
            refs.insert(c[1].to_uppercase());
        }
    }
    refs
}

/// Normalize kicad-cli item descriptions (`"Footprint C52"`, `"Pad 6 of U3"`)
/// to a set of component refs (waiver matching, Issue #4691).
pub fn extract_item_refs(items: &[String]) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    for item in items {
        for pattern in [&*REF_PATTERN, &*FOOTPRINT_REF_PATTERN] {
            for c in pattern.captures_iter(item) {
                refs.insert(c[1].to_uppercase());
            }
        }
    }
    refs
}
