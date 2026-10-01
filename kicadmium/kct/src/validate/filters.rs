//! DRC/ERC violation filtering and reclassification engine (port of
//! `kicad_tools.validate.filters`).
//!
//! Rules come from `[[drc.filters]]` / `[[erc.filters]]` tables in a TOML
//! config. Each rule's regex patterns (case-insensitive) must all match for
//! the rule to apply; the first matching rule wins.
//!
//! Patterns are compiled with the Rust `regex` crate, which lacks Python's
//! look-around and backreferences; such patterns are rejected as invalid.

use std::fmt;
use std::path::Path;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use crate::drc::severity::{ERCSeverity, Severity};
use crate::drc::violation::DRCViolation;
use crate::erc::violation::ERCViolation;
use crate::pyjson::{py_repr_str, Json};

/// Valid filter actions.
pub const VALID_ACTIONS: [&str; 3] = ["error", "ignore", "warning"];

/// Invalid filter configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterConfigError(pub String);

impl fmt::Display for FilterConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FilterConfigError {}

/// Errors from [`load_filters_from_toml`].
#[derive(Debug)]
pub enum FilterLoadError {
    NotFound(String),
    Config(FilterConfigError),
}

impl fmt::Display for FilterLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilterLoadError::NotFound(m) => f.write_str(m),
            FilterLoadError::Config(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for FilterLoadError {}

/// Violation types the filter engine can match and reclassify.
pub trait Filterable: Clone {
    fn filter_type_str(&self) -> &str;
    fn filter_message(&self) -> &str;
    fn filter_items(&self) -> &[String];
    fn filter_nets(&self) -> &[String] {
        &[]
    }
    fn filter_sheet(&self) -> &str {
        ""
    }
    /// Copy with severity set to `new_severity` (`"warning"`/`"error"`).
    fn reclassified(&self, new_severity: &str) -> Self;
}

impl Filterable for DRCViolation {
    fn filter_type_str(&self) -> &str {
        &self.type_str
    }
    fn filter_message(&self) -> &str {
        &self.message
    }
    fn filter_items(&self) -> &[String] {
        &self.items
    }
    fn filter_nets(&self) -> &[String] {
        &self.nets
    }
    fn reclassified(&self, new_severity: &str) -> Self {
        let mut v = self.clone();
        v.severity = Severity::from_string(new_severity);
        v
    }
}

impl Filterable for ERCViolation {
    fn filter_type_str(&self) -> &str {
        &self.type_str
    }
    fn filter_message(&self) -> &str {
        &self.description
    }
    fn filter_items(&self) -> &[String] {
        &self.items
    }
    fn filter_sheet(&self) -> &str {
        &self.sheet
    }
    fn reclassified(&self, new_severity: &str) -> Self {
        let mut v = self.clone();
        v.severity = ERCSeverity::from_string(new_severity);
        v
    }
}

/// A single filter rule.
#[derive(Debug, Clone)]
pub struct ViolationFilter {
    pub type_pattern: Option<String>,
    pub message_pattern: Option<String>,
    pub component_pattern: Option<String>,
    pub net_pattern: Option<String>,
    pub sheet_pattern: Option<String>,
    pub action: String,
    pub comment: String,
    compiled: [Option<Regex>; 5],
}

const PATTERN_ATTRS: [&str; 5] = [
    "type_pattern",
    "message_pattern",
    "component_pattern",
    "net_pattern",
    "sheet_pattern",
];

impl ViolationFilter {
    /// Validate `action` and compile every pattern (`re.IGNORECASE`).
    pub fn new(
        type_pattern: Option<&str>,
        message_pattern: Option<&str>,
        component_pattern: Option<&str>,
        net_pattern: Option<&str>,
        sheet_pattern: Option<&str>,
        action: &str,
        comment: &str,
    ) -> Result<Self, FilterConfigError> {
        if !VALID_ACTIONS.contains(&action) {
            return Err(FilterConfigError(format!(
                "Invalid filter action {}; must be one of {}",
                py_repr_str(action),
                VALID_ACTIONS.join(", ")
            )));
        }
        let patterns = [
            type_pattern,
            message_pattern,
            component_pattern,
            net_pattern,
            sheet_pattern,
        ];
        let mut compiled: [Option<Regex>; 5] = Default::default();
        for (i, p) in patterns.iter().enumerate() {
            if let Some(p) = p {
                compiled[i] = Some(
                    RegexBuilder::new(p)
                        .case_insensitive(true)
                        .build()
                        .map_err(|e| {
                            FilterConfigError(format!(
                                "Invalid regex in {}: {} -- {}",
                                PATTERN_ATTRS[i],
                                py_repr_str(p),
                                e.to_string().lines().last().unwrap_or_default().trim()
                            ))
                        })?,
                );
            }
        }
        let own = |p: Option<&str>| p.map(str::to_string);
        Ok(ViolationFilter {
            type_pattern: own(type_pattern),
            message_pattern: own(message_pattern),
            component_pattern: own(component_pattern),
            net_pattern: own(net_pattern),
            sheet_pattern: own(sheet_pattern),
            action: action.to_string(),
            comment: comment.to_string(),
            compiled,
        })
    }

    /// Filter with only a type pattern (common case).
    pub fn by_type(type_pattern: &str, action: &str) -> Result<Self, FilterConfigError> {
        Self::new(Some(type_pattern), None, None, None, None, action, "")
    }

    fn match_pattern(&self, idx: usize, values: &[&str]) -> bool {
        match &self.compiled[idx] {
            None => true,
            Some(re) => values.iter().any(|v| re.is_match(v)),
        }
    }

    /// Whether `violation` matches all specified patterns.
    pub fn matches<V: Filterable>(&self, v: &V) -> bool {
        if !self.match_pattern(0, &[v.filter_type_str()]) {
            return false;
        }
        if !self.match_pattern(1, &[v.filter_message()]) {
            return false;
        }
        if self.component_pattern.is_some() {
            let refs = extract_refs(v.filter_items());
            if refs.is_empty() {
                return false;
            }
            let refs: Vec<&str> = refs.iter().map(String::as_str).collect();
            if !self.match_pattern(2, &refs) {
                return false;
            }
        }
        if self.net_pattern.is_some() {
            let nets: Vec<&str> = v.filter_nets().iter().map(String::as_str).collect();
            if nets.is_empty() || !self.match_pattern(3, &nets) {
                return false;
            }
        }
        if self.sheet_pattern.is_some() {
            let sheet = v.filter_sheet();
            if sheet.is_empty() || !self.match_pattern(4, &[sheet]) {
                return false;
            }
        }
        true
    }
}

/// Result of applying filters.
#[derive(Debug, Clone)]
pub struct FilterResult<V> {
    pub kept: Vec<V>,
    pub ignored: Vec<V>,
    pub reclassified: Vec<V>,
    pub raw_count: usize,
}

impl<V> FilterResult<V> {
    pub fn ignored_count(&self) -> usize {
        self.ignored.len()
    }
    pub fn reclassified_count(&self) -> usize {
        self.reclassified.len()
    }
    pub fn kept_count(&self) -> usize {
        self.kept.len()
    }
}

/// Applies filter rules in order; first match wins.
#[derive(Debug, Clone, Default)]
pub struct FilterEngine {
    pub filters: Vec<ViolationFilter>,
}

impl FilterEngine {
    pub fn new(filters: Vec<ViolationFilter>) -> Self {
        FilterEngine { filters }
    }

    pub fn apply<V: Filterable>(&self, violations: &[V]) -> FilterResult<V> {
        let mut result = FilterResult {
            kept: Vec::new(),
            ignored: Vec::new(),
            reclassified: Vec::new(),
            raw_count: violations.len(),
        };
        for v in violations {
            match self.filters.iter().find(|f| f.matches(v)) {
                None => result.kept.push(v.clone()),
                Some(f) if f.action == "ignore" => result.ignored.push(v.clone()),
                Some(f) if f.action == "warning" || f.action == "error" => {
                    let r = v.reclassified(&f.action);
                    result.kept.push(r.clone());
                    result.reclassified.push(r);
                }
                Some(_) => result.kept.push(v.clone()),
            }
        }
        result
    }
}

/// Parse DRC and ERC filter lists from a parsed config (TOML as [`Json`]).
pub fn parse_filters_from_config(
    config: &Json,
) -> Result<(Vec<ViolationFilter>, Vec<ViolationFilter>), FilterConfigError> {
    let empty = Json::obj();
    let drc = parse_filter_list(config.get("drc").unwrap_or(&empty), "drc")?;
    let erc = parse_filter_list(config.get("erc").unwrap_or(&empty), "erc")?;
    Ok((drc, erc))
}

fn parse_filter_list(
    section: &Json,
    label: &str,
) -> Result<Vec<ViolationFilter>, FilterConfigError> {
    let raw = match section.get("filters") {
        None => return Ok(Vec::new()),
        Some(Json::Arr(list)) => list,
        Some(other) => {
            return Err(FilterConfigError(format!(
                "[{label}.filters] must be an array of tables, got {}",
                other.py_type_name()
            )))
        }
    };
    let mut out = Vec::new();
    for (i, entry) in raw.iter().enumerate() {
        if !matches!(entry, Json::Obj(_)) {
            return Err(FilterConfigError(format!(
                "[{label}.filters] entry {i} must be a table, got {}",
                entry.py_type_name()
            )));
        }
        let get = |k: &str| entry.get(k).and_then(Json::as_str);
        out.push(
            ViolationFilter::new(
                get("type_pattern"),
                get("message_pattern"),
                get("component_pattern"),
                get("net_pattern"),
                get("sheet_pattern"),
                get("action").unwrap_or("ignore"),
                get("comment").unwrap_or(""),
            )
            .map_err(|e| FilterConfigError(format!("[{label}.filters] entry {i}: {e}")))?,
        );
    }
    Ok(out)
}

/// Convert a parsed TOML value into [`Json`].
pub fn toml_to_json(v: &toml::Value) -> Json {
    match v {
        toml::Value::String(s) => Json::Str(s.clone()),
        toml::Value::Integer(i) => Json::Int(*i),
        toml::Value::Float(f) => Json::Float(*f),
        toml::Value::Boolean(b) => Json::Bool(*b),
        toml::Value::Datetime(d) => Json::Str(d.to_string()),
        toml::Value::Array(a) => Json::Arr(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Json::Obj(
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

/// Load `(drc_filters, erc_filters)` from a TOML file.
pub fn load_filters_from_toml(
    path: impl AsRef<Path>,
) -> Result<(Vec<ViolationFilter>, Vec<ViolationFilter>), FilterLoadError> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(FilterLoadError::NotFound(format!(
            "Filter config not found: {}",
            path.display()
        )));
    }
    let data = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|text| text.parse::<toml::Table>().map_err(|e| e.to_string()))
        .map_err(|e| {
            FilterLoadError::Config(FilterConfigError(format!(
                "Error reading {}: {}",
                path.display(),
                e.trim()
            )))
        })?;
    let json = toml_to_json(&toml::Value::Table(data));
    parse_filters_from_config(&json).map_err(FilterLoadError::Config)
}

static REF_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bof\s+([A-Z]+\d+)\b").expect("regex"));
static BARE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[A-Z]+\d+$").expect("regex"));

/// Component refs in item strings ("Pad 1 of U3", or bare "D1"), in order.
pub fn extract_refs(items: &[String]) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    for item in items {
        for c in REF_PATTERN.captures_iter(item) {
            let r = c[1].to_uppercase();
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
    }
    for item in items {
        let stripped = item.trim();
        if BARE_REF.is_match(stripped) {
            let r = stripped.to_uppercase();
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
    }
    refs
}
