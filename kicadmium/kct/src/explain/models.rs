//! Data models for the explain system (port of `kicad_tools.explain.models`).

use crate::drc::violation::DRCViolation;
use crate::jobj;
use crate::pyjson::{py_float_repr, Json};

/// Reference to an external specification.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpecReference {
    pub name: String,
    pub section: String,
    pub url: String,
    pub version: String,
    /// Normalized manufacturer key (`jlcpcb`, `oshpark`), empty for
    /// interface specs.
    pub manufacturer: String,
}

impl SpecReference {
    pub fn new(name: impl Into<String>) -> Self {
        SpecReference {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn to_dict(&self) -> Json {
        jobj! {
            "name" => self.name.as_str(),
            "section" => self.section.as_str(),
            "url" => self.url.as_str(),
            "version" => self.version.as_str(),
            "manufacturer" => self.manufacturer.as_str(),
        }
    }
}

/// Explanation for a design rule.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleExplanation {
    pub rule_id: String,
    pub title: String,
    pub explanation: String,
    pub spec_references: Vec<SpecReference>,
    pub fix_templates: Vec<String>,
    pub related_rules: Vec<String>,
    pub learn_more: Option<String>,
    pub severity: String,
}

impl RuleExplanation {
    pub fn new(
        rule_id: impl Into<String>,
        title: impl Into<String>,
        explanation: impl Into<String>,
    ) -> Self {
        RuleExplanation {
            rule_id: rule_id.into(),
            title: title.into(),
            explanation: explanation.into(),
            spec_references: Vec::new(),
            fix_templates: Vec::new(),
            related_rules: Vec::new(),
            learn_more: None,
            severity: "error".into(),
        }
    }

    pub fn to_dict(&self) -> Json {
        jobj! {
            "rule_id" => self.rule_id.as_str(),
            "title" => self.title.as_str(),
            "explanation" => self.explanation.as_str(),
            "spec_references" => Json::Arr(self.spec_references.iter().map(SpecReference::to_dict).collect()),
            "fix_templates" => &self.fix_templates,
            "related_rules" => &self.related_rules,
            "learn_more" => self.learn_more.clone(),
            "severity" => self.severity.as_str(),
        }
    }
}

/// Result of explaining a rule or violation.
#[derive(Debug, Clone, PartialEq)]
pub struct ExplanationResult {
    pub rule: String,
    pub title: String,
    pub explanation: String,
    pub spec_reference: Option<SpecReference>,
    pub current_value: Option<f64>,
    pub required_value: Option<f64>,
    pub unit: String,
    pub severity: String,
    pub fix_suggestions: Vec<String>,
    pub related_rules: Vec<String>,
    /// Python `dict[str, Any]` context (insertion ordered).
    pub context: Json,
}

impl ExplanationResult {
    pub fn new(
        rule: impl Into<String>,
        title: impl Into<String>,
        explanation: impl Into<String>,
    ) -> Self {
        ExplanationResult {
            rule: rule.into(),
            title: title.into(),
            explanation: explanation.into(),
            spec_reference: None,
            current_value: None,
            required_value: None,
            unit: String::new(),
            severity: "error".into(),
            fix_suggestions: Vec::new(),
            related_rules: Vec::new(),
            context: Json::obj(),
        }
    }

    pub fn to_dict(&self) -> Json {
        jobj! {
            "rule" => self.rule.as_str(),
            "title" => self.title.as_str(),
            "explanation" => self.explanation.as_str(),
            "spec_reference" => self.spec_reference.as_ref().map_or(Json::Null, SpecReference::to_dict),
            "current_value" => self.current_value,
            "required_value" => self.required_value,
            "unit" => self.unit.as_str(),
            "severity" => self.severity.as_str(),
            "fix_suggestions" => &self.fix_suggestions,
            "related_rules" => &self.related_rules,
            "context" => self.context.clone(),
        }
    }

    /// Tree structure for terminal output.
    pub fn format_tree(&self) -> String {
        let mut lines = vec![self.title.clone()];
        if let Some(r) = &self.spec_reference {
            lines.push(format!("├─ Spec: {}", r.name));
            if !r.section.is_empty() {
                lines.push(format!("│  Section: {}", r.section));
            }
            if !r.version.is_empty() {
                lines.push(format!("│  Version: {}", r.version));
            }
        }
        lines.push(format!("├─ Rationale: {}", self.explanation));
        if let (Some(c), Some(r)) = (self.current_value, self.required_value) {
            lines.push(format!("├─ Current: {}{}", py_float_repr(c), self.unit));
            lines.push(format!("├─ Required: {}{}", py_float_repr(r), self.unit));
        }
        if let Some((first, rest)) = self.fix_suggestions.split_first() {
            lines.push(format!("├─ Fix: {first}"));
            for s in rest {
                lines.push(format!("│  Or: {s}"));
            }
        }
        if !self.related_rules.is_empty() {
            lines.push(format!("└─ Related: {}", self.related_rules.join(", ")));
        }
        lines.join("\n")
    }
}

/// A DRC violation with its explanation attached.
#[derive(Debug, Clone)]
pub struct ExplainedViolation {
    pub violation: DRCViolation,
    pub explanation: ExplanationResult,
}

impl ExplainedViolation {
    pub fn to_dict(&self) -> Json {
        jobj! {
            "violation" => self.violation.to_json(),
            "explanation" => self.explanation.to_dict(),
        }
    }
}

/// Specification for a communication interface.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceSpec {
    pub interface: String,
    pub spec_document: String,
    pub spec_url: String,
    /// `dict[str, dict[str, Any]]` parsed from YAML (ordered).
    pub constraints: Json,
}

impl InterfaceSpec {
    pub fn to_dict(&self) -> Json {
        jobj! {
            "interface" => self.interface.as_str(),
            "spec_document" => self.spec_document.as_str(),
            "spec_url" => self.spec_url.as_str(),
            "constraints" => self.constraints.clone(),
        }
    }
}
