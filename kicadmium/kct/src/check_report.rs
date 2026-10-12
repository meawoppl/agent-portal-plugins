//! Versioned CLI contract shared by native and heuristic check commands.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
pub const SCHEMA: &str = "kicadmium.check/v1";
pub const JSON_SCHEMA: &str = include_str!("../../docs/check-report.schema.json");
#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub schema: String,
    pub tool: String,
    pub ok: bool,
    pub summary: Value,
    pub findings: Vec<Value>,
    pub details: Value,
}
impl Envelope {
    pub fn new(tool: &str, ok: bool, mut report: Value) -> Self {
        // Accept historical native reports at this one boundary; never emit a
        // string-encoded report or require consumers to decode nested JSON.
        while let Value::String(text) = &report {
            report = serde_json::from_str(text).unwrap_or(Value::Null);
        }
        let mut findings = Vec::new();
        collect(&report, &mut findings);
        let mut counts = BTreeMap::<String, usize>::new();
        for finding in &findings {
            *counts
                .entry(finding["severity"].as_str().unwrap_or("info").into())
                .or_default() += 1;
        }
        let mut summary = report
            .get("summary")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        for (key, severity) in [
            ("errors", "error"),
            ("warnings", "warning"),
            ("infos", "info"),
        ] {
            if summary.get(key).is_none() {
                summary[key] = json!(counts.get(severity).copied().unwrap_or(0));
            }
        }
        summary["shown"] = json!(findings.len());
        let valid = report.is_object();
        let mut details = report;
        if let Some(object) = details.as_object_mut() {
            for key in [
                "findings",
                "violations",
                "unconnected_items",
                "schematic_parity",
                "sheets",
                "summary",
            ] {
                object.remove(key);
            }
        }
        Self {
            schema: SCHEMA.into(),
            tool: tool.into(),
            ok: ok && valid,
            summary,
            findings,
            details,
        }
    }
}
fn collect(report: &Value, out: &mut Vec<Value>) {
    for key in [
        "findings",
        "violations",
        "unconnected_items",
        "schematic_parity",
    ] {
        for raw in report[key].as_array().into_iter().flatten() {
            let mut finding = raw.clone();
            if !finding.is_object() {
                continue;
            }
            let rule = raw["rule"]
                .as_str()
                .or_else(|| raw["type"].as_str())
                .unwrap_or(key);
            let severity = raw["severity"].as_str().unwrap_or("warning");
            let message = raw["message"]
                .as_str()
                .or_else(|| raw["description"].as_str())
                .unwrap_or("");
            finding["rule"] = json!(rule);
            finding["severity"] = json!(severity);
            finding["message"] = json!(message);
            finding["nets"] = raw["nets"]
                .as_array()
                .map(|a| json!(a))
                .unwrap_or_else(|| json!([]));
            finding["refs"] = raw["refs"]
                .as_array()
                .or_else(|| raw["components"].as_array())
                .map(|a| json!(a))
                .unwrap_or_else(|| json!([]));
            let at = raw
                .get("at")
                .or_else(|| raw.pointer("/items/0/pos"))
                .or_else(|| raw.get("location"))
                .cloned()
                .unwrap_or(Value::Null);
            finding["at"] = if let Some(pair) = at.as_array().filter(|a| a.len() == 2) {
                json!({"x":pair[0],"y":pair[1]})
            } else {
                at
            };
            out.push(finding);
        }
    }
    for sheet in report["sheets"].as_array().into_iter().flatten() {
        collect(sheet, out);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_parser_handles_all_check_families() {
        let samples = [
            (
                "drc",
                json!({"violations":[{"type":"clearance","severity":"error","description":"short","items":[{"pos":{"x":1,"y":2}}]}],"unconnected_items":[{"type":"unconnected","severity":"error"}]}),
            ),
            (
                "erc",
                json!({"sheets":[{"violations":[{"type":"pin","severity":"warning","description":"unconnected"}]}]}),
            ),
            (
                "lint",
                json!({"findings":[{"rule":"trace.short","severity":"info","nets":["GND"],"at":{"x":1,"y":2},"message":"short"}]}),
            ),
            (
                "quality",
                json!({"summary":{"warnings":99},"violations":[{"type":"bypass","severity":"warning","description":"far"}]}),
            ),
        ];
        for (tool, report) in samples {
            let envelope = Envelope::new(tool, false, report);
            let encoded = serde_json::to_value(&envelope).unwrap();
            assert_eq!(encoded["schema"], SCHEMA);
            let decoded: Envelope = serde_json::from_value(encoded).unwrap();
            assert!(!decoded.findings.is_empty());
            for finding in decoded.findings {
                for key in ["rule", "severity", "nets", "refs", "at", "message"] {
                    assert!(finding.get(key).is_some());
                }
            }
            if tool == "quality" {
                assert_eq!(decoded.summary["warnings"], 99);
            }
        }
    }
    #[test]
    fn no_double_encoding_or_silent_success_on_missing_report() {
        assert!(!Envelope::new("drc", true, Value::Null).ok);
        let output = Envelope::new("erc", true, json!("{\"violations\":[]}"));
        assert!(output.ok);
        assert!(output.details.is_object());
        let schema: Value = serde_json::from_str(JSON_SCHEMA).unwrap();
        assert_eq!(schema["properties"]["schema"]["const"], SCHEMA);
    }
}
