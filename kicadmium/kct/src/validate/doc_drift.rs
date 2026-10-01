//! LLM-free doc-drift lint for `kct check` (port of
//! `kicad_tools.validate.doc_drift`, issue #4540).
//!
//! Only opt-in `<!-- kct:doc-pin RESOLVER KEY = VALUE -->` markers in the
//! board README(s) are evaluated; every finding is INFO severity.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::utils::pyrepr::{py_repr, py_str_repr};
use crate::validate::violations::{DRCResults, DRCViolation};

pub const RULE_STALE_PIN: &str = "doc_drift_stale_pin";
pub const RULE_UNRESOLVABLE_PIN: &str = "doc_drift_unresolvable_pin";

/// Repo-relative drc-tolerance ground-truth file.
pub const TOLERANCE_FILE: &str = ".github/routed-drc-tolerance.yml";

static DOC_PIN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"<!--\s*kct:doc-pin\s+(?P<resolver>[A-Za-z0-9_-]+)\s+(?P<key>\S+)\s*=\s*(?P<value>\S+)\s*-->",
    )
    .unwrap()
});

/// One parsed `kct:doc-pin` marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocPin {
    pub resolver: String,
    pub key: String,
    pub claimed: String,
    pub doc_path: PathBuf,
    pub line: usize,
}

/// Outcome of resolving a doc-pin's ground truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Resolved { value: i64, source: String },
    Unresolvable { reason: String },
    Skip,
}

/// Python `str.splitlines()` boundaries (the common subset).
fn split_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let brk = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !brk {
            continue;
        }
        out.push(&text[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r' {
            if let Some((j, '\n')) = chars.peek().copied() {
                chars.next();
                end = j + 1;
            }
        }
        start = end;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Extract every well-formed marker; a missing/unreadable doc yields none.
pub fn parse_doc_pins(doc_path: &Path) -> Vec<DocPin> {
    let Ok(text) = std::fs::read_to_string(doc_path) else {
        return vec![];
    };
    let mut pins = Vec::new();
    for (i, line) in split_lines(&text).into_iter().enumerate() {
        for m in DOC_PIN_RE.captures_iter(line) {
            pins.push(DocPin {
                resolver: m["resolver"].to_string(),
                key: m["key"].to_string(),
                claimed: m["value"].to_string(),
                doc_path: doc_path.to_path_buf(),
                line: i + 1,
            });
        }
    }
    pins
}

/// `Path.resolve()` (non-strict).
fn resolve(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    // Resolve the longest existing prefix, then append the remainder.
    let mut existing = abs.clone();
    let mut tail = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(n), Some(parent)) => {
                tail.push(n.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => return abs,
        }
    }
    let mut out = std::fs::canonicalize(&existing).unwrap_or(existing);
    for n in tail.into_iter().rev() {
        out.push(n);
    }
    out
}

/// Walk up from `start` to a directory holding `.git` or the tolerance file.
pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let current = resolve(start);
    current
        .ancestors()
        .find(|c| c.join(".git").exists() || c.join(TOLERANCE_FILE).is_file())
        .map(Path::to_path_buf)
}

/// Python `int(raw)` for a YAML scalar.
fn py_int(raw: &Value) -> Option<i64> {
    match raw {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.trunc() as i64)
        }),
        Value::String(s) => {
            let t = s.trim();
            let (neg, digits) = match t.strip_prefix('-') {
                Some(r) => (true, r),
                None => (false, t.strip_prefix('+').unwrap_or(t)),
            };
            if digits.is_empty()
                || digits.starts_with('_')
                || digits.ends_with('_')
                || digits.contains("__")
                || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
            {
                return None;
            }
            let v: i64 = digits.replace('_', "").parse().ok()?;
            Some(if neg { -v } else { v })
        }
        _ => None,
    }
}

fn resolve_drc_tolerance(repo_root: &Path, key: &str) -> Resolution {
    let tolerance_path = repo_root.join(TOLERANCE_FILE);
    if !tolerance_path.is_file() {
        return Resolution::Skip;
    }
    if !repo_root.join(key).is_file() {
        return Resolution::Unresolvable {
            reason: format!(
                "referenced routed-PCB path {} does not exist in the repo (root {})",
                py_str_repr(key),
                repo_root.display()
            ),
        };
    }
    let data: Value = match std::fs::read_to_string(&tolerance_path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_yaml::from_str::<serde_yaml::Value>(&t).map_err(|e| e.to_string()))
        .and_then(|y| serde_json::to_value(y).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            return Resolution::Unresolvable {
                reason: format!("could not read {TOLERANCE_FILE}: {e}"),
            }
        }
    };
    let tolerances = match &data {
        Value::Object(m) => m
            .get("tolerances")
            .cloned()
            .unwrap_or(Value::Object(Default::default())),
        _ => Value::Object(Default::default()),
    };
    let Value::Object(map) = tolerances else {
        return Resolution::Unresolvable {
            reason: format!("{TOLERANCE_FILE} 'tolerances' field is not a mapping"),
        };
    };
    match map.get(key) {
        None | Some(Value::Null) => Resolution::Resolved {
            value: 0,
            source: format!("{TOLERANCE_FILE} (no entry => strict 0)"),
        },
        Some(raw) => match py_int(raw) {
            Some(value) => Resolution::Resolved {
                value,
                source: TOLERANCE_FILE.to_string(),
            },
            None => Resolution::Unresolvable {
                reason: format!(
                    "{TOLERANCE_FILE} entry for {} is not an integer: {}",
                    py_str_repr(key),
                    py_repr(raw)
                ),
            },
        },
    }
}

/// Registered resolver names (sorted).
pub const RESOLVERS: &[&str] = &["drc-tolerance"];

fn run_resolver(name: &str, repo_root: &Path, key: &str) -> Option<Resolution> {
    match name {
        "drc-tolerance" => Some(resolve_drc_tolerance(repo_root, key)),
        _ => None,
    }
}

fn display_path(doc_path: &Path, repo_root: &Path) -> String {
    let r = resolve(doc_path);
    match r.strip_prefix(repo_root) {
        Ok(rel) => {
            let s = rel.to_string_lossy().to_string();
            if s.is_empty() {
                ".".into()
            } else {
                s
            }
        }
        Err(_) => doc_path.display().to_string(),
    }
}

fn readme_candidates(pcb_path: &Path) -> Vec<PathBuf> {
    let resolved = resolve(pcb_path);
    let parent = resolved.parent().unwrap_or(Path::new("/")).to_path_buf();
    let grand = parent.parent().unwrap_or(&parent).to_path_buf();
    let mut out: Vec<PathBuf> = Vec::new();
    for c in [parent.join("README.md"), grand.join("README.md")] {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Run the doc-drift lint for the board owning `pcb_path`.
pub fn check_doc_drift(pcb_path: &Path) -> DRCResults {
    let mut results = DRCResults::with_rules_checked(2);
    results.set_rule(RULE_STALE_PIN, 1);
    results.set_rule(RULE_UNRESOLVABLE_PIN, 1);

    let pins: Vec<DocPin> = readme_candidates(pcb_path)
        .iter()
        .flat_map(|d| parse_doc_pins(d))
        .collect();
    if pins.is_empty() {
        return results;
    }
    let resolved_pcb = resolve(pcb_path);
    let Some(repo_root) = find_repo_root(resolved_pcb.parent().unwrap_or(Path::new("/"))) else {
        return results;
    };

    let mut findings: Vec<((String, usize, &str), DRCViolation)> = Vec::new();
    for pin in &pins {
        let doc_rel = display_path(&pin.doc_path, &repo_root);
        let Some(resolution) = run_resolver(&pin.resolver, &repo_root, &pin.key) else {
            let message = format!(
                "{doc_rel}:{}: doc-pin names unknown resolver {} (known: {})",
                pin.line,
                py_str_repr(&pin.resolver),
                RESOLVERS.join(", ")
            );
            findings.push((
                (doc_rel, pin.line, RULE_UNRESOLVABLE_PIN),
                DRCViolation::new(RULE_UNRESOLVABLE_PIN, "info", message),
            ));
            continue;
        };
        match resolution {
            Resolution::Skip => {}
            Resolution::Unresolvable { reason } => {
                let message = format!(
                    "{doc_rel}:{}: doc-pin key {} could not be resolved: {reason}",
                    pin.line,
                    py_str_repr(&pin.key)
                );
                findings.push((
                    (doc_rel, pin.line, RULE_UNRESOLVABLE_PIN),
                    DRCViolation::new(RULE_UNRESOLVABLE_PIN, "info", message),
                ));
            }
            Resolution::Resolved { value, source } => {
                let claimed = py_int(&Value::String(pin.claimed.clone()));
                if claimed == Some(value) {
                    continue;
                }
                let message = format!(
                    "{doc_rel}:{}: doc-pin claims {} {} = {}, but {source} pins {value}",
                    pin.line, pin.resolver, pin.key, pin.claimed
                );
                findings.push((
                    (doc_rel, pin.line, RULE_STALE_PIN),
                    DRCViolation::new(RULE_STALE_PIN, "info", message)
                        .actual(value as f64)
                        .required_opt(claimed.map(|c| c as f64)),
                ));
            }
        }
    }
    findings.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, v) in findings {
        results.add(v);
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_regex_and_int() {
        let line = "x <!-- kct:doc-pin drc-tolerance boards/a.kicad_pcb = 8 --> y";
        let c = DOC_PIN_RE.captures(line).unwrap();
        assert_eq!(&c["resolver"], "drc-tolerance");
        assert_eq!(&c["key"], "boards/a.kicad_pcb");
        assert_eq!(&c["value"], "8");
        assert_eq!(py_int(&Value::String(" 1_0 ".into())), Some(10));
        assert_eq!(py_int(&Value::String("8.0".into())), None);
        assert_eq!(split_lines("a\r\nb\rc\n"), vec!["a", "b", "c"]);
    }
}
