//! Read-only inspection of project netclasses (port of
//! `kicad_tools.core.netclass_diagnostics`).
//!
//! Results describe independent memberships (KiCad 10.0.5 matching
//! contract), not assignment precedence or effective electrical constraints.
//! Inputs are never modified; original entries are cloned verbatim.

use std::collections::{BTreeSet, HashSet};

use serde_json::{json, Map, Value};

use crate::utils::pyrepr::py_str_repr;

fn is_literal(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | ' ' | '-' | ':')
}

fn is_quant(c: char) -> bool {
    c == '*' || c == '?'
}

/// Pattern within the verified literal / `*` / `?` subset, without
/// adjacent quantifiers.
pub fn pattern_supported(pattern: &str) -> bool {
    let chars: Vec<char> = pattern.chars().collect();
    chars.iter().all(|&c| is_literal(c) || is_quant(c))
        && !chars.windows(2).any(|w| is_quant(w[0]) && is_quant(w[1]))
}

/// Union of anchored wildcard and restricted anchored regex matching,
/// without backtracking. The unnamed net never matches.
pub fn pattern_matches(pattern: &str, net: &str) -> bool {
    if net.is_empty() {
        return false;
    }
    let pat: Vec<char> = pattern.chars().collect();
    let net: Vec<char> = net.chars().collect();
    let n = net.len();
    let mut ends: HashSet<usize> = HashSet::from([n]);
    if net.last() == Some(&'\n') {
        ends.insert(n - 1);
    }
    let mut positions: BTreeSet<usize> = BTreeSet::from([0]);
    for &c in &pat {
        if c == '*' {
            positions = match positions.first() {
                Some(&lo) => (lo..=n).collect(),
                None => BTreeSet::new(),
            };
        } else {
            positions = positions
                .iter()
                .filter(|&&i| i < n && (c == '?' || net[i] == c))
                .map(|i| i + 1)
                .collect();
        }
    }
    if positions.iter().any(|p| ends.contains(p)) {
        return true;
    }
    if pat.first().is_some_and(|&c| is_quant(c)) {
        return false;
    }
    let mut positions: BTreeSet<usize> = BTreeSet::from([0]);
    let mut index = 0;
    while index < pat.len() {
        let c = pat[index];
        let quantifier = pat.get(index + 1).copied().filter(|&q| is_quant(q));
        if let Some(q) = quantifier {
            let mut next = positions.clone();
            for &start in &positions {
                let mut end = start;
                while end < n && net[end] == c {
                    end += 1;
                    next.insert(end);
                    if q == '?' {
                        break;
                    }
                }
            }
            positions = next;
            index += 2;
        } else {
            positions = positions
                .iter()
                .filter(|&&i| i < n && net[i] == c)
                .map(|i| i + 1)
                .collect();
            index += 1;
        }
    }
    positions.iter().any(|p| ends.contains(p))
}

struct Report {
    diagnostics: Vec<Value>,
}

impl Report {
    fn diagnostic(&mut self, code: &str, path: &str, message: &str) {
        self.diagnostics
            .push(json!({"code": code, "path": path, "message": message}));
    }
}

fn nonempty_str(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Diagnose loaded `.kicad_pro` JSON. `net_names`: `None` means "not
/// evaluated"; otherwise it must be a JSON array of strings (anything else
/// is diagnosed `invalid_inventory` and not partially evaluated).
pub fn diagnose_netclasses(data: &Value, net_names: Option<&Value>) -> Value {
    let mut settings_status = "missing";
    let mut default_status = "absent";
    let mut inventory_status = "not_evaluated";
    let mut classes: Vec<Value> = Vec::new();
    let mut patterns: Vec<Value> = Vec::new();
    let mut assignments: Vec<Value> = Vec::new();
    let mut r = Report {
        diagnostics: Vec::new(),
    };

    let mut inventory: Option<Vec<String>> = None;
    if let Some(nets) = net_names {
        match nets.as_array().and_then(|items| {
            items
                .iter()
                .map(|n| n.as_str().map(str::to_string))
                .collect::<Option<BTreeSet<String>>>()
        }) {
            Some(set) => {
                inventory = Some(set.into_iter().collect());
                inventory_status = "evaluated";
            }
            None => {
                inventory_status = "invalid";
                r.diagnostic(
                    "invalid_inventory",
                    "net_names",
                    "Expected a collection of net-name strings.",
                );
            }
        }
    }

    let build = |settings_status: &str,
                 default_status: &str,
                 inventory_status: &str,
                 classes: Vec<Value>,
                 patterns: Vec<Value>,
                 assignments: Vec<Value>,
                 diagnostics: Vec<Value>| {
        let mut out = Map::new();
        out.insert("settings_status".into(), json!(settings_status));
        out.insert("default_status".into(), json!(default_status));
        out.insert("inventory_status".into(), json!(inventory_status));
        out.insert("classes".into(), Value::Array(classes));
        out.insert("patterns".into(), Value::Array(patterns));
        out.insert("assignments".into(), Value::Array(assignments));
        out.insert("diagnostics".into(), Value::Array(diagnostics));
        Value::Object(out)
    };

    let Some(data) = data.as_object() else {
        r.diagnostic("invalid_project", "$", "Expected a project JSON object.");
        return build(
            "invalid",
            default_status,
            inventory_status,
            classes,
            patterns,
            assignments,
            r.diagnostics,
        );
    };
    let Some(settings) = data.get("net_settings") else {
        r.diagnostic(
            "missing_settings",
            "net_settings",
            "No net settings are declared.",
        );
        r.diagnostic(
            "missing_default",
            "net_settings.classes",
            "Default is not declared.",
        );
        return build(
            settings_status,
            default_status,
            inventory_status,
            classes,
            patterns,
            assignments,
            r.diagnostics,
        );
    };
    let Some(settings) = settings.as_object() else {
        r.diagnostic("invalid_settings", "net_settings", "Expected an object.");
        return build(
            "invalid",
            default_status,
            inventory_status,
            classes,
            patterns,
            assignments,
            r.diagnostics,
        );
    };
    settings_status = "present";

    let empty: Vec<Value> = Vec::new();
    let entries = |r: &mut Report, field: &str| -> Vec<Value> {
        match settings.get(field) {
            None => empty.clone(),
            Some(Value::Array(items)) => items.clone(),
            Some(_) => {
                r.diagnostic(
                    "invalid_container",
                    &format!("net_settings.{field}"),
                    "Expected an array.",
                );
                Vec::new()
            }
        }
    };

    let mut names: HashSet<String> = HashSet::new();
    for (index, entry) in entries(&mut r, "classes").into_iter().enumerate() {
        let path = format!("net_settings.classes[{index}]");
        let name = entry.as_object().and_then(|o| o.get("name")).cloned();
        classes.push(json!({
            "index": index,
            "name": name.clone().unwrap_or(Value::Null),
            "entry": entry.clone(),
        }));
        match nonempty_str(name.as_ref()) {
            None => r.diagnostic(
                "invalid_class",
                &path,
                "Expected an object with a nonempty class name.",
            ),
            Some(n) if names.contains(n) => r.diagnostic(
                "duplicate_class",
                &path,
                &format!("Class {} is declared more than once.", py_str_repr(n)),
            ),
            Some(n) => {
                names.insert(n.to_string());
            }
        }
        if entry.as_object().is_some_and(|o| o.contains_key("nets")) {
            r.diagnostic(
                "unsupported_assignment",
                &format!("{path}.nets"),
                "Legacy class-local assignments are not evaluated.",
            );
        }
    }
    if names.contains("Default") {
        default_status = "declared";
    } else {
        r.diagnostic(
            "missing_default",
            "net_settings.classes",
            "Default is not declared.",
        );
    }

    let target_defined = |r: &mut Report, target: Option<&Value>, path: &str| -> Value {
        match nonempty_str(target) {
            None => {
                r.diagnostic("invalid_target", path, "Expected a nonempty class name.");
                Value::Null
            }
            Some(t) if !names.contains(t) => {
                r.diagnostic(
                    "undefined_class",
                    path,
                    &format!("Class {} is not declared.", py_str_repr(t)),
                );
                Value::Bool(false)
            }
            Some(_) => Value::Bool(true),
        }
    };

    for (index, entry) in entries(&mut r, "netclass_patterns").into_iter().enumerate() {
        let path = format!("net_settings.netclass_patterns[{index}]");
        let obj = entry.as_object();
        let pattern = obj.and_then(|o| o.get("pattern")).cloned();
        let target = obj.and_then(|o| o.get("netclass")).cloned();
        let defined = target_defined(&mut r, target.as_ref(), &format!("{path}.netclass"));
        let mut status = "invalid";
        let mut matches = Value::Null;
        match pattern.as_ref().and_then(Value::as_str) {
            None => r.diagnostic(
                "invalid_pattern",
                &format!("{path}.pattern"),
                "Expected a pattern string.",
            ),
            Some(p) if !pattern_supported(p) => {
                status = "unsupported";
                r.diagnostic(
                    "unsupported_pattern",
                    &format!("{path}.pattern"),
                    "Outside the verified literal/*/? subset.",
                );
            }
            Some(p) => {
                status = "supported";
                if let Some(inv) = &inventory {
                    let found: Vec<Value> = inv
                        .iter()
                        .filter(|net| pattern_matches(p, net))
                        .map(|net| Value::String(net.clone()))
                        .collect();
                    if found.is_empty() {
                        r.diagnostic(
                            "no_matches",
                            &format!("{path}.pattern"),
                            "No supplied board nets match.",
                        );
                    }
                    matches = Value::Array(found);
                }
            }
        }
        let mut row = Map::new();
        row.insert("index".into(), json!(index));
        row.insert("entry".into(), entry.clone());
        row.insert("pattern".into(), pattern.unwrap_or(Value::Null));
        row.insert("target".into(), target.unwrap_or(Value::Null));
        row.insert("target_defined".into(), defined);
        row.insert("status".into(), json!(status));
        row.insert("matches".into(), matches);
        patterns.push(Value::Object(row));
    }

    let net_present = |net: &str| -> Value {
        match &inventory {
            Some(inv) => Value::Bool(inv.iter().any(|n| n == net)),
            None => Value::Null,
        }
    };
    match settings.get("netclass_assignments") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (index, (net, targets)) in map.iter().enumerate() {
                let path = format!("net_settings.netclass_assignments[{index}]");
                let Some(list) = targets.as_array() else {
                    assignments.push(json!({
                        "index": index,
                        "net": net,
                        "entry": targets.clone(),
                        "status": "unsupported",
                    }));
                    r.diagnostic(
                        "unsupported_assignment",
                        &path,
                        "Expected a string net name and an array of class names; legacy strings \
                         are not migrated.",
                    );
                    continue;
                };
                if list.is_empty() {
                    assignments.push(json!({
                        "index": index,
                        "net": net,
                        "entry": [],
                        "status": "supported",
                        "net_present": net_present(net),
                    }));
                }
                for (target_index, target) in list.iter().enumerate() {
                    let defined =
                        target_defined(&mut r, Some(target), &format!("{path}[{target_index}]"));
                    let status = if defined.is_null() {
                        "invalid"
                    } else {
                        "supported"
                    };
                    assignments.push(json!({
                        "index": index,
                        "target_index": target_index,
                        "net": net,
                        "target": target.clone(),
                        "target_defined": defined,
                        "status": status,
                        "net_present": net_present(net),
                    }));
                }
            }
        }
        Some(_) => r.diagnostic(
            "unsupported_assignment",
            "net_settings.netclass_assignments",
            "Expected a net-name object of class-name arrays.",
        ),
    }

    build(
        settings_status,
        default_status,
        inventory_status,
        classes,
        patterns,
        assignments,
        r.diagnostics,
    )
}

/// Convenience over [`diagnose_netclasses`] for a string inventory.
pub fn diagnose_netclasses_for_nets<S: AsRef<str>>(data: &Value, net_names: Option<&[S]>) -> Value {
    let nets = net_names.map(|n| {
        Value::Array(
            n.iter()
                .map(|s| Value::String(s.as_ref().to_string()))
                .collect(),
        )
    });
    diagnose_netclasses(data, nets.as_ref())
}
