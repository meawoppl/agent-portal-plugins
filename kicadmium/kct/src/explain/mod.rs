//! Queryable explanation system with spec references (port of
//! `kicad_tools.explain`).
//!
//! Explains DRC violations and design-rule constraints with references to
//! specifications and fix suggestions, and detects common PCB design
//! mistakes ([`mistakes`]).

pub mod checks;
pub mod decisions;
pub mod formatters;
pub mod mistakes;
pub mod models;
pub mod registry;

pub use formatters::{format_result, format_violations, FORMATTERS, VIOLATION_FORMATTERS};
pub use mistakes::{
    detect_mistakes, detect_mistakes_with_coverage, get_default_checks, CheckCoverage,
    CheckIncomplete, Mistake, MistakeCategory, MistakeCheck, MistakeDetector,
};
pub use models::{
    ExplainedViolation, ExplanationResult, InterfaceSpec, RuleExplanation, SpecReference,
};
pub use registry::{normalize_manufacturer, ExplanationRegistry, DEFAULT_MANUFACTURER};

use crate::drc::violation::DRCViolation;
use crate::pyjson::{py_float_repr, py_repr_str, py_title, Json};

/// Explain a rule (upstream `explain`). `Err` carries the `ValueError`
/// message for unknown rules.
pub fn explain(rule_id: &str, context: Option<Json>) -> Result<ExplanationResult, String> {
    let reg = ExplanationRegistry::global();
    explain_with(&reg, rule_id, context)
}

/// [`explain`] against an explicit registry.
pub fn explain_with(
    reg: &ExplanationRegistry,
    rule_id: &str,
    context: Option<Json>,
) -> Result<ExplanationResult, String> {
    let context = match context {
        Some(c @ Json::Obj(_)) => c,
        _ => Json::obj(),
    };
    let explanation = match reg.get(rule_id) {
        Some(e) => e,
        None => match reg.search(rule_id).first() {
            Some(e) => *e,
            None => {
                return Err(format!(
                    "Unknown rule: {}. Use list_rules() to see available rules.",
                    py_repr_str(rule_id)
                ))
            }
        },
    };

    // `context.get("value") or context.get("current_value")`
    let current = match context.get("value") {
        Some(v) if v.truthy() => Some(v),
        _ => context.get("current_value"),
    };
    let current_value = current.and_then(Json::as_f64);
    let required_value = context.get("required_value").and_then(Json::as_f64);
    let unit = match context.get("unit") {
        Some(Json::Str(s)) => s.clone(),
        Some(other) => crate::explain::registry::py_str(other),
        None => "mm".to_string(),
    };

    let fix_suggestions =
        generate_fix_suggestions(explanation, &context, current_value, required_value, &unit);
    let manufacturer = context
        .get("manufacturer")
        .filter(|m| m.truthy())
        .map(crate::explain::registry::py_str);
    let spec_reference =
        resolve_spec_reference(&explanation.spec_references, manufacturer.as_deref());

    Ok(ExplanationResult {
        rule: explanation.rule_id.clone(),
        title: explanation.title.clone(),
        explanation: explanation.explanation.clone(),
        spec_reference,
        current_value,
        required_value,
        unit,
        severity: explanation.severity.clone(),
        fix_suggestions,
        related_rules: explanation.related_rules.clone(),
        context,
    })
}

/// Explain a list of DRC violations (upstream `explain_violations`).
pub fn explain_violations(violations: &[DRCViolation]) -> Vec<ExplainedViolation> {
    let reg = ExplanationRegistry::global();
    violations
        .iter()
        .map(|v| {
            let rule_id = rule_id_from_violation(v);
            let context = build_context_from_violation(v);
            let explanation = explain_with(&reg, &rule_id, Some(context))
                .unwrap_or_else(|_| create_generic_explanation(v));
            ExplainedViolation {
                violation: v.clone(),
                explanation,
            }
        })
        .collect()
}

/// Explain the constraints applied to a net (upstream
/// `explain_net_constraints`).
pub fn explain_net_constraints(net_name: &str, interface_type: Option<&str>) -> ExplanationResult {
    let interface_type = match interface_type {
        Some(t) => Some(t.to_string()),
        None => infer_interface_type(net_name).map(str::to_string),
    };
    let Some(interface_type) = interface_type else {
        let mut r = ExplanationResult::new(
            "unknown_net",
            format!("Net: {net_name}"),
            format!(
                "No specific interface type detected for net '{net_name}'. Standard design rules apply."
            ),
        );
        r.severity = "info".into();
        return r;
    };

    let reg = ExplanationRegistry::global();
    let Some(spec) = reg.get_interface(&interface_type) else {
        let mut r = ExplanationResult::new(
            format!("{interface_type}_unknown"),
            format!("Interface: {interface_type}"),
            format!("Interface type '{interface_type}' is not documented in the registry."),
        );
        r.severity = "warning".into();
        return r;
    };

    let mut desc: Vec<String> = Vec::new();
    if let Json::Obj(items) = &spec.constraints {
        for (name, data) in items {
            if let Some(v) = data.get("value") {
                desc.push(format!("- {name}: {}", registry::py_str(v)));
            } else if let Some(v) = data.get("max_skew_ps") {
                desc.push(format!("- {name}: max {}ps skew", registry::py_str(v)));
            }
        }
    }

    let mut r = ExplanationResult::new(
        format!("{interface_type}_constraints"),
        format!("{} Constraints", spec.interface),
        format!(
            "Net '{net_name}' is part of a {} interface.\n\nDerived constraints:\n{}",
            spec.interface,
            desc.join("\n")
        ),
    );
    r.spec_reference = Some(SpecReference {
        name: spec.spec_document.clone(),
        url: spec.spec_url.clone(),
        ..Default::default()
    });
    r.severity = "info".into();
    r.context = crate::jobj! {
        "net_name" => net_name,
        "interface" => interface_type.as_str(),
    };
    r
}

/// Sorted rule IDs.
pub fn list_rules() -> Vec<String> {
    ExplanationRegistry::global().list_rules()
}

/// Rules matching a query (cloned, registration order).
pub fn search_rules(query: &str) -> Vec<RuleExplanation> {
    ExplanationRegistry::global()
        .search(query)
        .into_iter()
        .cloned()
        .collect()
}

// ---------------------------------------------------------------- helpers

fn generate_fix_suggestions(
    explanation: &RuleExplanation,
    context: &Json,
    current_value: Option<f64>,
    required_value: Option<f64>,
    unit: &str,
) -> Vec<String> {
    let mut suggestions = Vec::new();
    let delta = match (current_value, required_value) {
        (Some(c), Some(r)) => Some(r - c),
        _ => None,
    };
    let ctx_str = |key: &str, default: &str| -> String {
        context
            .get(key)
            .map_or_else(|| default.to_string(), registry::py_str)
    };

    for template in &explanation.fix_templates {
        let mut s = template.clone();
        if let Some(d) = delta.filter(|_| s.contains("{delta}")) {
            s = s.replace("{delta}", &format!("{d:.3}"));
        }
        if let Some(r) = required_value.filter(|_| s.contains("{required}")) {
            s = s.replace("{required}", &py_float_repr(r));
        }
        if let Some(c) = current_value.filter(|_| s.contains("{current}")) {
            s = s.replace("{current}", &py_float_repr(c));
        }
        if s.contains("{unit}") {
            s = s.replace("{unit}", unit);
        }
        if s.contains("{net1}") {
            s = s.replace("{net1}", &ctx_str("net1", "NET1"));
        }
        if s.contains("{net2}") {
            s = s.replace("{net2}", &ctx_str("net2", "NET2"));
        }
        suggestions.push(s);
    }

    if suggestions.is_empty() {
        if let Some(d) = delta {
            let rule = explanation.rule_id.to_lowercase();
            let req = required_value.map(py_float_repr).unwrap_or_default();
            let cur = current_value.map(py_float_repr).unwrap_or_default();
            suggestions.push(if rule.contains("clearance") {
                format!("Increase spacing by at least {d:.3}{unit} to meet minimum clearance")
            } else if rule.contains("width") || rule.contains("trace") {
                format!("Increase width to at least {req}{unit}")
            } else if rule.contains("via") {
                format!("Adjust via size to meet {req}{unit} minimum")
            } else {
                format!("Adjust value from {cur}{unit} to at least {req}{unit}")
            });
        }
    }
    suggestions
}

fn resolve_spec_reference(
    refs: &[SpecReference],
    manufacturer: Option<&str>,
) -> Option<SpecReference> {
    if refs.is_empty() {
        return None;
    }
    if let Some(m) = manufacturer.filter(|m| !m.is_empty()) {
        let key = normalize_manufacturer(m);
        if let Some(r) = refs.iter().find(|r| r.manufacturer == key) {
            return Some(r.clone());
        }
    }
    if let Some(r) = refs.iter().find(|r| r.manufacturer == DEFAULT_MANUFACTURER) {
        return Some(r.clone());
    }
    Some(refs[0].clone())
}

/// Upstream `_get_rule_id_from_violation`: `DRCViolation` always has a
/// `rule` attribute (possibly empty), so that is what upstream returns.
fn rule_id_from_violation(v: &DRCViolation) -> String {
    v.rule.clone()
}

fn build_context_from_violation(v: &DRCViolation) -> Json {
    let mut ctx = Json::obj();
    ctx.set("required_value", v.required_value_mm);
    ctx.set("value", v.actual_value_mm);
    if let Some(n) = v.nets.first() {
        ctx.set("net1", n.as_str());
    }
    if let Some(n) = v.nets.get(1) {
        ctx.set("net2", n.as_str());
    }
    if let Some(loc) = v.primary_location() {
        ctx.set(
            "location",
            Json::Arr(vec![loc.x_mm.into(), loc.y_mm.into()]),
        );
    }
    ctx
}

fn create_generic_explanation(v: &DRCViolation) -> ExplanationResult {
    let rule_id = rule_id_from_violation(v);
    let mut r = ExplanationResult::new(
        rule_id.as_str(),
        py_title(&rule_id.replace('_', " ")),
        format!(
            "This violation indicates a design rule check failure. Message: {}",
            v.message
        ),
    );
    r.severity = v.severity.value().to_string();
    r.fix_suggestions =
        vec!["Review the design at the indicated location and adjust accordingly.".into()];
    r
}

fn infer_interface_type(net_name: &str) -> Option<&'static str> {
    let up = net_name.to_uppercase();
    let any = |pats: &[&str]| pats.iter().any(|p| up.contains(p));
    if any(&["USB_D+", "USB_D-", "USB_DP", "USB_DM", "VBUS"]) {
        return Some("usb_20_high_speed");
    }
    if any(&["SDA", "SCL", "I2C"]) {
        return Some("i2c");
    }
    if any(&["MOSI", "MISO", "SCLK", "SCK", "SPI_"]) {
        return Some("spi");
    }
    if any(&["UART_TX", "UART_RX", "TXD", "RXD"]) {
        return Some("uart");
    }
    if any(&["TDI", "TDO", "TCK", "TMS", "JTAG"]) {
        return Some("jtag");
    }
    None
}
