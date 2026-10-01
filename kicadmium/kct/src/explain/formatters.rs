//! Output formatters for explanation results (port of
//! `kicad_tools.explain.formatters`).

use super::models::{ExplainedViolation, ExplanationResult};
use crate::pyjson::{dumps_indent, py_float_repr, Json};

/// Plain text.
pub fn format_text(result: &ExplanationResult) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("Rule: {}", result.rule));
    lines.push(format!("Title: {}", result.title));
    lines.push(format!("Severity: {}", result.severity));
    lines.push(String::new());

    lines.push("Explanation:".into());
    for line in result.explanation.split('\n') {
        lines.push(format!("  {}", line.trim()));
    }
    lines.push(String::new());

    if let Some(r) = &result.spec_reference {
        lines.push("Specification Reference:".into());
        lines.push(format!("  Name: {}", r.name));
        if !r.section.is_empty() {
            lines.push(format!("  Section: {}", r.section));
        }
        if !r.url.is_empty() {
            lines.push(format!("  URL: {}", r.url));
        }
        if !r.version.is_empty() {
            lines.push(format!("  Version: {}", r.version));
        }
        lines.push(String::new());
    }

    if result.current_value.is_some() || result.required_value.is_some() {
        lines.push("Values:".into());
        if let Some(c) = result.current_value {
            lines.push(format!("  Current: {}{}", py_float_repr(c), result.unit));
        }
        if let Some(r) = result.required_value {
            lines.push(format!("  Required: {}{}", py_float_repr(r), result.unit));
        }
        lines.push(String::new());
    }

    if !result.fix_suggestions.is_empty() {
        lines.push("Fix Suggestions:".into());
        for (i, s) in result.fix_suggestions.iter().enumerate() {
            lines.push(format!("  {}. {s}", i + 1));
        }
        lines.push(String::new());
    }

    if !result.related_rules.is_empty() {
        lines.push(format!("Related Rules: {}", result.related_rules.join(", ")));
    }
    lines.join("\n")
}

/// Compact tree.
pub fn format_tree(result: &ExplanationResult) -> String {
    result.format_tree()
}

/// JSON (`json.dumps(result.to_dict(), indent=2)`).
pub fn format_json(result: &ExplanationResult) -> String {
    dumps_indent(&result.to_dict(), 2)
}

/// Markdown.
pub fn format_markdown(result: &ExplanationResult) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("## {}", result.title));
    lines.push(String::new());
    lines.push(format!("**Rule ID:** `{}`  ", result.rule));
    lines.push(format!("**Severity:** {}", result.severity));
    lines.push(String::new());

    lines.push("### Explanation".into());
    lines.push(String::new());
    lines.push(result.explanation.clone());
    lines.push(String::new());

    if let Some(r) = &result.spec_reference {
        lines.push("### Specification Reference".into());
        lines.push(String::new());
        if !r.url.is_empty() {
            lines.push(format!("- **Source:** [{}]({})", r.name, r.url));
        } else {
            lines.push(format!("- **Source:** {}", r.name));
        }
        if !r.section.is_empty() {
            lines.push(format!("- **Section:** {}", r.section));
        }
        if !r.version.is_empty() {
            lines.push(format!("- **Version:** {}", r.version));
        }
        lines.push(String::new());
    }

    if result.current_value.is_some() || result.required_value.is_some() {
        lines.push("### Values".into());
        lines.push(String::new());
        lines.push("| Metric | Value |".into());
        lines.push("|--------|-------|".into());
        if let Some(c) = result.current_value {
            lines.push(format!("| Current | {}{} |", py_float_repr(c), result.unit));
        }
        if let Some(r) = result.required_value {
            lines.push(format!("| Required | {}{} |", py_float_repr(r), result.unit));
        }
        lines.push(String::new());
    }

    if !result.fix_suggestions.is_empty() {
        lines.push("### Fix Suggestions".into());
        lines.push(String::new());
        for s in &result.fix_suggestions {
            lines.push(format!("- {s}"));
        }
        lines.push(String::new());
    }

    if !result.related_rules.is_empty() {
        lines.push("### Related Rules".into());
        lines.push(String::new());
        for r in &result.related_rules {
            lines.push(format!("- `{r}`"));
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

/// Explained violations as plain text.
pub fn format_violations_text(violations: &[ExplainedViolation]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for (i, ev) in violations.iter().enumerate() {
        let v = &ev.violation;
        let exp = &ev.explanation;
        lines.push(format!("[{}] {}: {}", i + 1, v.type_str, v.message));
        lines.push(format!("    ├─ Rule: {}", exp.title));
        if let Some(r) = &exp.spec_reference {
            lines.push(format!("    ├─ Spec: {}", r.name));
        }
        if let Some(loc) = v.primary_location() {
            lines.push(format!(
                "    ├─ Location: ({:.2}, {:.2}) mm",
                loc.x_mm, loc.y_mm
            ));
        }
        match exp.fix_suggestions.first() {
            Some(f) => lines.push(format!("    └─ Fix: {f}")),
            None => lines.push("    └─ See rule explanation for guidance".into()),
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

/// Explained violations as JSON.
pub fn format_violations_json(violations: &[ExplainedViolation]) -> String {
    let data = Json::Arr(violations.iter().map(ExplainedViolation::to_dict).collect());
    dumps_indent(&data, 2)
}

/// Explained violations as markdown.
pub fn format_violations_markdown(violations: &[ExplainedViolation]) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("# DRC Violations with Explanations".into());
    lines.push(String::new());
    lines.push(format!("**Total violations:** {}", violations.len()));
    lines.push(String::new());

    for (i, ev) in violations.iter().enumerate() {
        let v = &ev.violation;
        let exp = &ev.explanation;
        lines.push(format!("## {}. {}", i + 1, v.type_str));
        lines.push(String::new());
        lines.push(format!("**Message:** {}", v.message));
        lines.push(String::new());
        if let Some(loc) = v.primary_location() {
            lines.push(format!(
                "**Location:** ({:.2}, {:.2}) mm",
                loc.x_mm, loc.y_mm
            ));
            lines.push(String::new());
        }
        lines.push(format!("### Explanation: {}", exp.title));
        lines.push(String::new());
        lines.push(exp.explanation.clone());
        lines.push(String::new());
        if let Some(r) = &exp.spec_reference {
            if !r.url.is_empty() {
                lines.push(format!("**Spec:** [{}]({})", r.name, r.url));
            } else {
                lines.push(format!("**Spec:** {}", r.name));
            }
            lines.push(String::new());
        }
        if !exp.fix_suggestions.is_empty() {
            lines.push("**Fix suggestions:**".into());
            for s in &exp.fix_suggestions {
                lines.push(format!("- {s}"));
            }
            lines.push(String::new());
        }
        lines.push("---".into());
        lines.push(String::new());
    }
    lines.join("\n")
}

/// Keys of upstream `FORMATTERS`, in order.
pub const FORMATTERS: &[&str] = &["text", "tree", "json", "markdown", "md"];
/// Keys of upstream `VIOLATION_FORMATTERS`, in order.
pub const VIOLATION_FORMATTERS: &[&str] = &["text", "json", "markdown", "md"];

/// Format a result; `Err` carries upstream's `ValueError` message.
pub fn format_result(result: &ExplanationResult, format_type: &str) -> Result<String, String> {
    Ok(match format_type {
        "text" => format_text(result),
        "tree" => format_tree(result),
        "json" => format_json(result),
        "markdown" | "md" => format_markdown(result),
        other => {
            return Err(format!(
                "Unknown format: {}. Available: {}",
                crate::pyjson::py_repr_str(other),
                FORMATTERS.join(", ")
            ))
        }
    })
}

/// Format explained violations; `Err` carries upstream's `ValueError` message.
pub fn format_violations(
    violations: &[ExplainedViolation],
    format_type: &str,
) -> Result<String, String> {
    Ok(match format_type {
        "text" => format_violations_text(violations),
        "json" => format_violations_json(violations),
        "markdown" | "md" => format_violations_markdown(violations),
        other => {
            return Err(format!(
                "Unknown format: {}. Available: {}",
                crate::pyjson::py_repr_str(other),
                VIOLATION_FORMATTERS.join(", ")
            ))
        }
    })
}
