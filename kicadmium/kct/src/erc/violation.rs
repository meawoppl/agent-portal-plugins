//! ERC violation data structures (port of `kicad_tools.erc.violation`).

use std::fmt;

pub use crate::drc::severity::ERCSeverity;
use crate::jobj;
use crate::pyjson::{py_title, Json};

/// ERC severity (upstream re-exports `ERCSeverity as Severity`).
pub type Severity = ERCSeverity;

macro_rules! erc_types {
    ($($variant:ident => $value:literal),* $(,)?) => {
        /// Known ERC violation types from KiCad.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[allow(non_camel_case_types)]
        pub enum ERCViolationType { $($variant),* }

        impl ERCViolationType {
            pub const ALL: &'static [ERCViolationType] = &[$(ERCViolationType::$variant),*];

            pub fn value(self) -> &'static str {
                match self { $(ERCViolationType::$variant => $value),* }
            }
        }
    };
}

erc_types! {
    PIN_NOT_CONNECTED => "pin_not_connected",
    PIN_NOT_DRIVEN => "pin_not_driven",
    POWER_PIN_NOT_DRIVEN => "power_pin_not_driven",
    NO_CONNECT_CONNECTED => "no_connect_connected",
    NO_CONNECT_DANGLING => "no_connect_dangling",
    CONFLICTING_NETCLASS => "conflicting_netclass",
    DIFFERENT_UNIT_FOOTPRINT => "different_unit_footprint",
    DIFFERENT_UNIT_NET => "different_unit_net",
    DUPLICATE_PIN_ERROR => "duplicate_pin_error",
    DUPLICATE_REFERENCE => "duplicate_reference",
    PIN_TO_PIN => "pin_to_pin",
    ENDPOINT_OFF_GRID => "endpoint_off_grid",
    EXTRA_UNITS => "extra_units",
    GLOBAL_LABEL_DANGLING => "global_label_dangling",
    HIER_LABEL_MISMATCH => "hier_label_mismatch",
    ISOLATED_PIN_LABEL => "isolated_pin_label",
    LABEL_DANGLING => "label_dangling",
    LIB_SYMBOL_ISSUES => "lib_symbol_issues",
    LIB_SYMBOL_MISMATCH => "lib_symbol_mismatch",
    FOOTPRINT_LINK_ISSUES => "footprint_link_issues",
    MISSING_BIDI_PIN => "missing_bidi_pin",
    MISSING_INPUT_PIN => "missing_input_pin",
    MISSING_POWER_PIN => "missing_power_pin",
    MISSING_UNIT => "missing_unit",
    MULTIPLE_NET_NAMES => "multiple_net_names",
    BUS_ENTRY_NEEDED => "bus_entry_needed",
    BUS_TO_BUS_CONFLICT => "bus_to_bus_conflict",
    BUS_TO_NET_CONFLICT => "bus_to_net_conflict",
    FOUR_WAY_JUNCTION => "four_way_junction",
    NET_NOT_BUS_MEMBER => "net_not_bus_member",
    SIMILAR_LABELS => "similar_labels",
    SINGLE_GLOBAL_LABEL => "single_global_label",
    SIMULATION_MODEL => "simulation_model",
    UNRESOLVED_VARIABLE => "unresolved_variable",
    UNANNOTATED => "unannotated",
    UNSPECIFIED => "unspecified",
    WIRE_DANGLING => "wire_dangling",
    UNCONNECTED_WIRE_ENDPOINT => "unconnected_wire_endpoint",
    UNKNOWN => "unknown",
}

impl ERCViolationType {
    /// Exact (case-insensitive, trimmed) value match, else UNKNOWN.
    pub fn from_string(s: &str) -> Self {
        let s = s.trim().to_lowercase();
        Self::ALL
            .iter()
            .copied()
            .find(|t| t.value() == s)
            .unwrap_or(ERCViolationType::UNKNOWN)
    }

    /// Blocks manufacturing readiness (`ERC_BLOCKING_TYPES`).
    pub fn is_blocking(self) -> bool {
        ERC_BLOCKING_TYPES.contains(&self)
    }

    /// Demoted to a warning in the audit verdict (`ERC_NON_BLOCKING_TYPES`).
    pub fn is_non_blocking(self) -> bool {
        ERC_NON_BLOCKING_TYPES.contains(&self)
    }
}

impl fmt::Display for ERCViolationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.value())
    }
}

/// Human-readable description for each type (`ERC_TYPE_DESCRIPTIONS`).
pub const ERC_TYPE_DESCRIPTIONS: &[(&str, &str)] = &[
    ("pin_not_connected", "Unconnected pin"),
    ("pin_not_driven", "Input pin not driven"),
    ("power_pin_not_driven", "Power input not driven"),
    ("no_connect_connected", "No-connect pin is connected"),
    (
        "no_connect_dangling",
        "No-connect flag not connected to pin",
    ),
    ("conflicting_netclass", "Conflicting netclass assignments"),
    (
        "different_unit_footprint",
        "Different footprint across symbol units",
    ),
    (
        "different_unit_net",
        "Different nets on same pin across units",
    ),
    ("duplicate_pin_error", "Duplicate pin in symbol"),
    ("duplicate_reference", "Duplicate reference designator"),
    (
        "pin_to_pin",
        "Conflicting pin types (output-to-output or similar)",
    ),
    ("endpoint_off_grid", "Wire endpoint off grid"),
    ("extra_units", "Extra units in multi-unit symbol"),
    ("global_label_dangling", "Global label not connected"),
    ("hier_label_mismatch", "Hierarchical label mismatch"),
    ("isolated_pin_label", "Label connected to only one pin"),
    ("label_dangling", "Label not connected"),
    ("lib_symbol_issues", "Library symbol issues"),
    (
        "lib_symbol_mismatch",
        "Symbol does not match library definition",
    ),
    (
        "footprint_link_issues",
        "Assigned footprint does not match footprint filters",
    ),
    ("missing_bidi_pin", "Missing bidirectional pin"),
    ("missing_input_pin", "Missing input pin"),
    ("missing_power_pin", "Missing power pin"),
    ("missing_unit", "Missing unit in multi-unit symbol"),
    ("multiple_net_names", "Wire has multiple net names"),
    ("bus_entry_needed", "Bus entry needed"),
    ("bus_to_bus_conflict", "Bus to bus conflict"),
    ("bus_to_net_conflict", "Bus to net conflict"),
    ("four_way_junction", "Four-way wire junction"),
    ("net_not_bus_member", "Net label on bus wire"),
    ("similar_labels", "Similar labels (possible typo)"),
    (
        "single_global_label",
        "Global label appears only once in design",
    ),
    ("simulation_model", "Simulation model issue"),
    ("unresolved_variable", "Unresolved text variable"),
    ("unannotated", "Symbol not annotated"),
    ("unspecified", "Unspecified error"),
    ("wire_dangling", "Wire not connected at both ends"),
    ("unconnected_wire_endpoint", "Unconnected wire endpoint"),
    ("unknown", "Unknown violation type"),
];

/// `ERC_TYPE_DESCRIPTIONS.get(type_str)`.
pub fn erc_type_description(type_str: &str) -> Option<&'static str> {
    ERC_TYPE_DESCRIPTIONS
        .iter()
        .find(|(k, _)| *k == type_str)
        .map(|(_, v)| *v)
}

/// Category groupings for display (`ERC_CATEGORIES`).
pub const ERC_CATEGORIES: &[(&str, &[&str])] = &[
    (
        "Connection",
        &[
            "pin_not_connected",
            "pin_not_driven",
            "power_pin_not_driven",
            "no_connect_connected",
            "no_connect_dangling",
        ],
    ),
    (
        "Pin Conflicts",
        &[
            "conflicting_netclass",
            "different_unit_footprint",
            "different_unit_net",
            "duplicate_pin_error",
            "duplicate_reference",
            "pin_to_pin",
        ],
    ),
    (
        "Labels",
        &[
            "global_label_dangling",
            "hier_label_mismatch",
            "isolated_pin_label",
            "label_dangling",
            "multiple_net_names",
            "similar_labels",
            "single_global_label",
        ],
    ),
    (
        "Structure",
        &[
            "bus_entry_needed",
            "bus_to_bus_conflict",
            "bus_to_net_conflict",
            "endpoint_off_grid",
            "four_way_junction",
            "net_not_bus_member",
            "unconnected_wire_endpoint",
            "wire_dangling",
        ],
    ),
    (
        "Symbols",
        &[
            "extra_units",
            "footprint_link_issues",
            "lib_symbol_issues",
            "lib_symbol_mismatch",
            "missing_bidi_pin",
            "missing_input_pin",
            "missing_power_pin",
            "missing_unit",
            "simulation_model",
            "unannotated",
        ],
    ),
    (
        "Other",
        &[
            "footprint_link_issues",
            "isolated_pin_label",
            "pin_to_pin",
            "single_global_label",
            "unresolved_variable",
            "unspecified",
            "unknown",
        ],
    ),
];

/// Genuine electrical problems that block manufacturing readiness.
pub const ERC_BLOCKING_TYPES: &[ERCViolationType] = &[
    ERCViolationType::POWER_PIN_NOT_DRIVEN,
    ERCViolationType::PIN_NOT_CONNECTED,
    ERCViolationType::PIN_NOT_DRIVEN,
    ERCViolationType::DIFFERENT_UNIT_NET,
    ERCViolationType::DUPLICATE_REFERENCE,
    ERCViolationType::MISSING_POWER_PIN,
    ERCViolationType::HIER_LABEL_MISMATCH,
    ERCViolationType::BUS_TO_NET_CONFLICT,
    ERCViolationType::BUS_TO_BUS_CONFLICT,
];

/// Non-electrical types demoted to warnings in the audit verdict.
pub const ERC_NON_BLOCKING_TYPES: &[ERCViolationType] = &[
    ERCViolationType::LIB_SYMBOL_MISMATCH,
    ERCViolationType::FOOTPRINT_LINK_ISSUES,
    ERCViolationType::SINGLE_GLOBAL_LABEL,
    ERCViolationType::ISOLATED_PIN_LABEL,
    ERCViolationType::PIN_TO_PIN,
    ERCViolationType::ENDPOINT_OFF_GRID,
    ERCViolationType::SIMILAR_LABELS,
];

/// A single ERC violation.
#[derive(Debug, Clone, PartialEq)]
pub struct ERCViolation {
    pub vtype: ERCViolationType,
    pub type_str: String,
    pub severity: ERCSeverity,
    pub description: String,
    pub sheet: String,
    pub pos_x: f64,
    pub pos_y: f64,
    pub items: Vec<String>,
    pub excluded: bool,
    pub suggestions: Vec<String>,
    /// Whether `pos_x`/`pos_y` were Python ints upstream (JSON ints or the
    /// `0` default), so JSON output renders `0` rather than `0.0`.
    pub(crate) pos_is_int: (bool, bool),
}

impl ERCViolation {
    pub fn new(
        vtype: ERCViolationType,
        type_str: impl Into<String>,
        severity: ERCSeverity,
        description: impl Into<String>,
    ) -> Self {
        ERCViolation {
            vtype,
            type_str: type_str.into(),
            severity,
            description: description.into(),
            sheet: String::new(),
            pos_x: 0.0,
            pos_y: 0.0,
            items: Vec::new(),
            excluded: false,
            suggestions: Vec::new(),
            pos_is_int: (true, true),
        }
    }

    /// Set a float position (as parsed from a text report or JSON floats).
    pub fn with_pos(mut self, x: f64, y: f64) -> Self {
        self.set_pos(x, false, y, false);
        self
    }

    pub(crate) fn set_pos(&mut self, x: f64, x_int: bool, y: f64, y_int: bool) {
        self.pos_x = x;
        self.pos_y = y;
        self.pos_is_int = (x_int, y_int);
    }

    pub fn is_error(&self) -> bool {
        self.severity == ERCSeverity::Error
    }

    pub fn is_connection_issue(&self) -> bool {
        use ERCViolationType::*;
        matches!(
            self.vtype,
            PIN_NOT_CONNECTED
                | PIN_NOT_DRIVEN
                | POWER_PIN_NOT_DRIVEN
                | NO_CONNECT_CONNECTED
                | NO_CONNECT_DANGLING
        )
    }

    pub fn is_label_issue(&self) -> bool {
        use ERCViolationType::*;
        matches!(
            self.vtype,
            GLOBAL_LABEL_DANGLING
                | HIER_LABEL_MISMATCH
                | LABEL_DANGLING
                | MULTIPLE_NET_NAMES
                | SIMILAR_LABELS
        )
    }

    /// Human-readable type description (falls back to a title-cased type).
    pub fn type_description(&self) -> String {
        erc_type_description(&self.type_str)
            .map(str::to_string)
            .unwrap_or_else(|| py_title(&self.type_str.replace('_', " ")))
    }

    pub fn location_str(&self) -> String {
        if !self.sheet.is_empty() {
            format!("{} at ({:.1}, {:.1})", self.sheet, self.pos_x, self.pos_y)
        } else if self.pos_x != 0.0 || self.pos_y != 0.0 {
            format!("({:.1}, {:.1})", self.pos_x, self.pos_y)
        } else {
            String::new()
        }
    }

    pub(crate) fn pos_json(&self) -> (Json, Json) {
        (
            Json::num(self.pos_x, self.pos_is_int.0),
            Json::num(self.pos_y, self.pos_is_int.1),
        )
    }

    pub fn to_json(&self) -> Json {
        let (x, y) = self.pos_json();
        jobj! {
            "type" => self.vtype.value(),
            "type_str" => self.type_str.as_str(),
            "type_description" => self.type_description(),
            "severity" => self.severity.value(),
            "description" => self.description.as_str(),
            "sheet" => self.sheet.as_str(),
            "position" => jobj!{"x" => x, "y" => y},
            "items" => &self.items,
            "excluded" => self.excluded,
            "suggestions" => &self.suggestions,
        }
    }
}

impl fmt::Display for ERCViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]: {}", self.type_str, self.description)?;
        let loc = self.location_str();
        if !loc.is_empty() {
            write!(f, " at {loc}")?;
        }
        Ok(())
    }
}
