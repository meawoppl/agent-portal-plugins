//! DRC violation models for the pure-Rust checker (port of
//! `kicad_tools.validate.violations`).

use crate::drc::compat::CheckerViolation;
use crate::drc::violation::ViolationType;
use crate::jobj;
use crate::pyjson::Json;
use crate::validate::mask_copper::MaskCopperAssessment;

/// A single DRC finding (upstream frozen dataclass `DRCViolation`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DRCViolation {
    pub rule_id: String,
    /// `"error"`, `"warning"` or `"info"`.
    pub severity: String,
    pub message: String,
    pub location: Option<(f64, f64)>,
    pub layer: Option<String>,
    pub actual_value: Option<f64>,
    pub required_value: Option<f64>,
    pub items: Vec<String>,
    pub nets: Vec<String>,
    pub waived: bool,
    pub waiver_reason: Option<String>,
    pub waiver_issue: Option<String>,
    pub closest_locations: Vec<(f64, f64)>,
}

impl DRCViolation {
    /// `DRCViolation(rule_id, severity, message)`; panics on a bad severity
    /// exactly where upstream raises `ValueError` in `__post_init__`.
    pub fn new(rule_id: impl Into<String>, severity: &str, message: impl Into<String>) -> Self {
        assert!(
            matches!(severity, "error" | "warning" | "info"),
            "severity must be 'error', 'warning', or 'info', got {severity:?}"
        );
        DRCViolation {
            rule_id: rule_id.into(),
            severity: severity.to_string(),
            message: message.into(),
            ..Default::default()
        }
    }

    pub fn at(mut self, x: f64, y: f64) -> Self {
        self.location = Some((x, y));
        self
    }

    pub fn location_opt(mut self, loc: Option<(f64, f64)>) -> Self {
        self.location = loc;
        self
    }

    pub fn layer(mut self, layer: impl Into<String>) -> Self {
        self.layer = Some(layer.into());
        self
    }

    pub fn layer_opt(mut self, layer: Option<String>) -> Self {
        self.layer = layer;
        self
    }

    pub fn actual(mut self, v: f64) -> Self {
        self.actual_value = Some(v);
        self
    }

    pub fn actual_opt(mut self, v: Option<f64>) -> Self {
        self.actual_value = v;
        self
    }

    pub fn required(mut self, v: f64) -> Self {
        self.required_value = Some(v);
        self
    }

    pub fn required_opt(mut self, v: Option<f64>) -> Self {
        self.required_value = v;
        self
    }

    pub fn items<I, S>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.items = items.into_iter().map(Into::into).collect();
        self
    }

    pub fn nets<I, S>(mut self, nets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.nets = nets.into_iter().map(Into::into).collect();
        self
    }

    /// Blocking error (not waived).
    pub fn is_error(&self) -> bool {
        self.severity == "error" && !self.waived
    }

    pub fn is_warning(&self) -> bool {
        self.severity == "warning" && !self.waived
    }

    pub fn is_info(&self) -> bool {
        self.severity == "info" && !self.waived
    }

    pub fn is_waived(&self) -> bool {
        self.waived
    }

    /// `to_dict()` (key order matches upstream).
    pub fn to_dict(&self) -> Json {
        let resolved = ViolationType::from_string(&self.rule_id);
        let loc = match self.location {
            // Python: `list(self.location) if self.location else None` -- a
            // 2-tuple is always truthy.
            Some((x, y)) => Json::Arr(vec![Json::Float(x), Json::Float(y)]),
            None => Json::Null,
        };
        let mut data = jobj! {
            "rule_id" => self.rule_id.as_str(),
            "type" => resolved.value(),
            "severity" => self.severity.as_str(),
            "message" => self.message.as_str(),
            "location" => loc,
            "layer" => self.layer.clone(),
            "actual_value" => self.actual_value,
            "required_value" => self.required_value,
            "items" => self.items.clone(),
            "nets" => self.nets.clone(),
            "status" => if self.waived { "waived" } else { self.severity.as_str() },
            "waived" => self.waived,
        };
        if !self.closest_locations.is_empty() {
            data.set(
                "closest_locations",
                Json::Arr(
                    self.closest_locations
                        .iter()
                        .map(|&(x, y)| Json::Arr(vec![Json::Float(x), Json::Float(y)]))
                        .collect(),
                ),
            );
        }
        if self.waived {
            data.set("waiver_reason", self.waiver_reason.clone());
            data.set("waiver_issue", self.waiver_issue.clone());
        }
        data
    }
}

impl CheckerViolation for DRCViolation {
    fn rule_id(&self) -> &str {
        &self.rule_id
    }
    fn severity(&self) -> &str {
        &self.severity
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn location(&self) -> Option<(f64, f64)> {
        self.location
    }
    fn layer(&self) -> Option<&str> {
        self.layer.as_deref()
    }
    fn items(&self) -> &[String] {
        &self.items
    }
    fn nets(&self) -> &[String] {
        &self.nets
    }
    fn required_value(&self) -> Option<f64> {
        self.required_value
    }
    fn actual_value(&self) -> Option<f64> {
        self.actual_value
    }
}

/// Aggregated results of a check run (upstream `DRCResults`).
#[derive(Debug, Clone, Default)]
pub struct DRCResults {
    pub violations: Vec<DRCViolation>,
    pub rules_checked: i64,
    /// Insertion-ordered `rule_id -> count` (upstream dict).
    pub rules_checked_by_rule: Vec<(String, i64)>,
    pub suppressed_count: i64,
    pub mask_copper_assessments: Vec<MaskCopperAssessment>,
}

impl DRCResults {
    pub fn new() -> Self {
        Self::default()
    }

    /// `DRCResults(rules_checked=n)`.
    pub fn with_rules_checked(n: i64) -> Self {
        DRCResults {
            rules_checked: n,
            ..Default::default()
        }
    }

    pub fn error_count(&self) -> usize {
        self.violations.iter().filter(|v| v.is_error()).count()
    }

    pub fn warning_count(&self) -> usize {
        self.violations.iter().filter(|v| v.is_warning()).count()
    }

    pub fn info_count(&self) -> usize {
        self.violations.iter().filter(|v| v.is_info()).count()
    }

    pub fn waived_count(&self) -> usize {
        self.violations.iter().filter(|v| v.is_waived()).count()
    }

    pub fn passed(&self) -> bool {
        self.error_count() == 0 && self.mask_copper_assessments.iter().all(|a| a.passed())
    }

    pub fn errors(&self) -> Vec<&DRCViolation> {
        self.violations.iter().filter(|v| v.is_error()).collect()
    }

    pub fn warnings(&self) -> Vec<&DRCViolation> {
        self.violations.iter().filter(|v| v.is_warning()).collect()
    }

    pub fn infos(&self) -> Vec<&DRCViolation> {
        self.violations.iter().filter(|v| v.is_info()).collect()
    }

    pub fn waived(&self) -> Vec<&DRCViolation> {
        self.violations.iter().filter(|v| v.is_waived()).collect()
    }

    pub fn len(&self) -> usize {
        self.violations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, DRCViolation> {
        self.violations.iter()
    }

    pub fn add(&mut self, v: DRCViolation) {
        self.violations.push(v);
    }

    /// Increment the per-rule check counter (`d[k] = d.get(k, 0) + n`).
    pub fn bump_rule(&mut self, rule_id: &str, n: i64) {
        match self.rules_checked_by_rule.iter_mut().find(|(k, _)| k == rule_id) {
            Some(e) => e.1 += n,
            None => self.rules_checked_by_rule.push((rule_id.to_string(), n)),
        }
    }

    /// Set the per-rule counter (`d[k] = n`).
    pub fn set_rule(&mut self, rule_id: &str, n: i64) {
        match self.rules_checked_by_rule.iter_mut().find(|(k, _)| k == rule_id) {
            Some(e) => e.1 = n,
            None => self.rules_checked_by_rule.push((rule_id.to_string(), n)),
        }
    }

    pub fn merge(&mut self, other: DRCResults) {
        self.violations.extend(other.violations);
        self.mask_copper_assessments
            .extend(other.mask_copper_assessments);
        self.rules_checked += other.rules_checked;
        for (rule_id, count) in &other.rules_checked_by_rule {
            self.bump_rule(rule_id, *count);
        }
        self.suppressed_count += other.suppressed_count;
    }

    pub fn filter_by_rule(&self, rule_id: &str) -> Vec<&DRCViolation> {
        self.violations
            .iter()
            .filter(|v| v.rule_id == rule_id)
            .collect()
    }

    pub fn filter_by_layer(&self, layer: &str) -> Vec<&DRCViolation> {
        self.violations
            .iter()
            .filter(|v| v.layer.as_deref() == Some(layer))
            .collect()
    }

    pub fn rules_checked_by_rule_json(&self) -> Json {
        Json::Obj(
            self.rules_checked_by_rule
                .iter()
                .map(|(k, v)| (k.clone(), Json::Int(*v)))
                .collect(),
        )
    }

    pub fn to_dict(&self) -> Json {
        jobj! {
            "passed" => self.passed(),
            "error_count" => self.error_count(),
            "warning_count" => self.warning_count(),
            "info_count" => self.info_count(),
            "waived_count" => self.waived_count(),
            "mask_copper_assessments" => Json::Arr(self.mask_copper_assessments.iter().map(|a| a.to_dict()).collect()),
            "rules_checked" => self.rules_checked,
            "rules_checked_by_rule" => self.rules_checked_by_rule_json(),
            "violations" => Json::Arr(self.violations.iter().map(|v| v.to_dict()).collect()),
        }
    }

    pub fn summary(&self) -> String {
        let status = if self.passed() { "PASSED" } else { "FAILED" };
        let info_part = if self.info_count() > 0 {
            format!(", {} infos", self.info_count())
        } else {
            String::new()
        };
        let waived_part = if self.waived_count() > 0 {
            format!(", {} waived", self.waived_count())
        } else {
            String::new()
        };
        format!(
            "DRC {status}: {} errors, {} warnings{info_part}{waived_part} ({} rules checked)",
            self.error_count(),
            self.warning_count(),
            self.rules_checked
        )
    }
}

impl<'a> IntoIterator for &'a DRCResults {
    type Item = &'a DRCViolation;
    type IntoIter = std::slice::Iter<'a, DRCViolation>;
    fn into_iter(self) -> Self::IntoIter {
        self.violations.iter()
    }
}
