//! Explanation registry for design rules and interface specs (port of
//! `kicad_tools.explain.registry`).
//!
//! Upstream keeps class-level dicts loaded lazily from `specs/*.yaml`. Here
//! the built-in YAML specs are embedded at compile time and loaded (in the
//! same sorted file order) into a process-wide registry on first use;
//! [`ExplanationRegistry::new`] builds independent instances for tests.

use std::sync::{LazyLock, RwLock, RwLockReadGuard};

use super::models::{InterfaceSpec, RuleExplanation, SpecReference};
use crate::pyjson::Json;

/// Default manufacturer used to resolve a rule's primary spec reference.
pub const DEFAULT_MANUFACTURER: &str = "jlcpcb";

/// Built-in spec files, in upstream's `sorted(specs_dir.glob("*.yaml"))` order.
pub const BUILTIN_SPECS: &[(&str, &str)] = &[
    ("i2c.yaml", include_str!("specs/i2c.yaml")),
    ("jlcpcb.yaml", include_str!("specs/jlcpcb.yaml")),
    ("oshpark.yaml", include_str!("specs/oshpark.yaml")),
    ("spi.yaml", include_str!("specs/spi.yaml")),
    ("usb.yaml", include_str!("specs/usb.yaml")),
];

/// Normalize a manufacturer name/id to a stable lookup key.
pub fn normalize_manufacturer(name: &str) -> String {
    name.to_lowercase().replace([' ', '-', '_'], "")
}

/// Registry of rule explanations and interface specs (insertion ordered,
/// like the upstream dicts).
#[derive(Debug, Clone, Default)]
pub struct ExplanationRegistry {
    explanations: Vec<(String, RuleExplanation)>,
    interfaces: Vec<(String, InterfaceSpec)>,
}

static GLOBAL: LazyLock<RwLock<ExplanationRegistry>> =
    LazyLock::new(|| RwLock::new(ExplanationRegistry::builtin()));

impl ExplanationRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry loaded from the built-in specs (upstream `_ensure_loaded`).
    pub fn builtin() -> Self {
        let mut reg = Self::new();
        for (_, text) in BUILTIN_SPECS {
            // Upstream logs and skips files that fail to load.
            let _ = reg.load_yaml_str(text);
        }
        reg
    }

    /// Process-wide registry.
    pub fn global() -> RwLockReadGuard<'static, ExplanationRegistry> {
        GLOBAL.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Mutate the process-wide registry (upstream classmethod `register`).
    pub fn with_global_mut<R>(f: impl FnOnce(&mut ExplanationRegistry) -> R) -> R {
        let mut guard = GLOBAL.write().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// Reset the process-wide registry to the built-in specs (`reload`).
    pub fn reload_global() {
        Self::with_global_mut(|r| *r = Self::builtin());
    }

    pub fn register(&mut self, rule_id: &str, explanation: RuleExplanation) {
        match self.explanations.iter_mut().find(|(k, _)| k == rule_id) {
            Some(slot) => slot.1 = explanation,
            None => self.explanations.push((rule_id.to_string(), explanation)),
        }
    }

    pub fn register_interface(&mut self, name: &str, spec: InterfaceSpec) {
        match self.interfaces.iter_mut().find(|(k, _)| k == name) {
            Some(slot) => slot.1 = spec,
            None => self.interfaces.push((name.to_string(), spec)),
        }
    }

    pub fn get(&self, rule_id: &str) -> Option<&RuleExplanation> {
        self.explanations
            .iter()
            .find(|(k, _)| k == rule_id)
            .map(|(_, v)| v)
    }

    fn get_mut(&mut self, rule_id: &str) -> Option<&mut RuleExplanation> {
        self.explanations
            .iter_mut()
            .find(|(k, _)| k == rule_id)
            .map(|(_, v)| v)
    }

    pub fn get_interface(&self, name: &str) -> Option<&InterfaceSpec> {
        self.interfaces
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
    }

    /// Sorted rule IDs.
    pub fn list_rules(&self) -> Vec<String> {
        let mut out: Vec<String> = self.explanations.iter().map(|(k, _)| k.clone()).collect();
        out.sort();
        out
    }

    /// Sorted interface names.
    pub fn list_interfaces(&self) -> Vec<String> {
        let mut out: Vec<String> = self.interfaces.iter().map(|(k, _)| k.clone()).collect();
        out.sort();
        out
    }

    /// All explanations in registration order.
    pub fn explanations(&self) -> impl Iterator<Item = &RuleExplanation> {
        self.explanations.iter().map(|(_, v)| v)
    }

    /// All interfaces in registration order.
    pub fn interfaces(&self) -> impl Iterator<Item = (&str, &InterfaceSpec)> {
        self.interfaces.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Rules whose ID or title contains `query` (case-insensitive), in
    /// registration order.
    pub fn search(&self, query: &str) -> Vec<&RuleExplanation> {
        let q = query.to_lowercase();
        self.explanations
            .iter()
            .filter(|(id, e)| id.to_lowercase().contains(&q) || e.title.to_lowercase().contains(&q))
            .map(|(_, e)| e)
            .collect()
    }

    pub fn clear(&mut self) {
        self.explanations.clear();
        self.interfaces.clear();
    }

    /// Load one YAML spec document (upstream `_load_yaml_file`).
    pub fn load_yaml_str(&mut self, text: &str) -> Result<(), String> {
        let value: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
        let data = yaml_to_json(&value);
        if !data.truthy() {
            return Ok(());
        }
        let Json::Obj(_) = data else {
            return Err("spec document is not a mapping".into());
        };
        if data.contains_key("manufacturer") {
            self.load_manufacturer_spec(&data)
        } else if data.contains_key("interface") {
            self.load_interface_spec(&data)
        } else {
            Ok(())
        }
    }

    fn load_manufacturer_spec(&mut self, data: &Json) -> Result<(), String> {
        let manufacturer = py_str_or(data.get("manufacturer"), "");
        let manufacturer_key = normalize_manufacturer(&manufacturer);
        let base_url = py_str_or(data.get("url"), "");
        let version = py_str_or(data.get("last_updated"), "");

        let empty = Json::obj();
        let rules = data.get("rules").unwrap_or(&empty);
        let Json::Obj(rules) = rules else {
            return Err("'rules' is not a mapping".into());
        };
        for (rule_id, rule_data) in rules {
            let spec_ref = SpecReference {
                name: format!("{manufacturer} Manufacturing Capabilities"),
                section: py_str_or(rule_data.get("spec_section"), ""),
                url: base_url.clone(),
                version: version.clone(),
                manufacturer: manufacturer_key.clone(),
            };

            let mut fix_templates = str_list(rule_data.get("fix_templates"));
            if fix_templates.is_empty() {
                if let Some(Json::Obj(values)) = rule_data.get("values") {
                    for (layer_type, value) in values {
                        fix_templates.push(format!(
                            "For {} boards: adjust to {}",
                            layer_type.replace('_', " "),
                            py_str(value)
                        ));
                    }
                }
            }
            let title = py_str_or(rule_data.get("title"), rule_id);
            let explanation = py_str_or(rule_data.get("explanation"), "")
                .trim()
                .to_string();
            let related = str_list(rule_data.get("related_rules"));
            let severity = py_str_or(rule_data.get("severity"), "error");

            if let Some(existing) = self.get_mut(rule_id) {
                if existing
                    .spec_references
                    .iter()
                    .any(|r| r.manufacturer == manufacturer_key)
                {
                    continue;
                }
                let is_default = manufacturer_key == DEFAULT_MANUFACTURER;
                let primary_is_default = existing
                    .spec_references
                    .first()
                    .is_some_and(|r| r.manufacturer == DEFAULT_MANUFACTURER);
                if is_default && !primary_is_default {
                    existing.title = title;
                    existing.explanation = explanation;
                    existing.fix_templates = fix_templates;
                    existing.related_rules = related;
                    existing.severity = severity;
                    existing.spec_references.insert(0, spec_ref);
                } else {
                    existing.spec_references.push(spec_ref);
                }
                continue;
            }

            let mut exp = RuleExplanation::new(rule_id.as_str(), title, explanation);
            exp.spec_references = vec![spec_ref];
            exp.fix_templates = fix_templates;
            exp.related_rules = related;
            exp.severity = severity;
            self.register(rule_id, exp);
        }
        Ok(())
    }

    fn load_interface_spec(&mut self, data: &Json) -> Result<(), String> {
        let interface_name = py_str_or(data.get("interface"), "");
        let spec_doc = py_str_or(data.get("spec_document"), "");
        let spec_url = py_str_or(data.get("spec_url"), "");
        let constraints = data.get("constraints").cloned().unwrap_or_else(Json::obj);

        let spec = InterfaceSpec {
            interface: interface_name.clone(),
            spec_document: spec_doc.clone(),
            spec_url: spec_url.clone(),
            constraints: constraints.clone(),
        };
        let normalized = interface_name
            .to_lowercase()
            .replace(' ', "_")
            .replace('.', "");
        self.register_interface(&normalized, spec);

        let Json::Obj(items) = &constraints else {
            return Err("'constraints' is not a mapping".into());
        };
        for (constraint_id, cdata) in items {
            let spec_ref = SpecReference {
                name: spec_doc.clone(),
                section: py_str_or(cdata.get("section"), ""),
                url: spec_url.clone(),
                ..Default::default()
            };
            let rule_id = format!("{normalized}_{constraint_id}");
            let title = format!(
                "{interface_name} - {}",
                crate::pyjson::py_title(&constraint_id.replace('_', " "))
            );
            let mut exp = RuleExplanation::new(
                rule_id.as_str(),
                title,
                py_str_or(cdata.get("explanation"), "").trim().to_string(),
            );
            exp.spec_references = vec![spec_ref];
            exp.fix_templates = str_list(cdata.get("fix_templates"));
            exp.severity = py_str_or(cdata.get("severity"), "error");
            self.register(&rule_id, exp);
        }
        Ok(())
    }
}

/// Python `str(value)` for a YAML-derived value.
pub fn py_str(v: &Json) -> String {
    match v {
        Json::Str(s) => s.clone(),
        other => other.py_repr(),
    }
}

fn py_str_or(v: Option<&Json>, default: &str) -> String {
    v.map_or_else(|| default.to_string(), py_str)
}

fn str_list(v: Option<&Json>) -> Vec<String> {
    match v {
        Some(Json::Arr(items)) => items.iter().map(py_str).collect(),
        _ => Vec::new(),
    }
}

/// Convert a parsed YAML value into the ordered Python-like [`Json`] model.
pub fn yaml_to_json(v: &serde_yaml::Value) -> Json {
    use serde_yaml::Value as Y;
    match v {
        Y::Null => Json::Null,
        Y::Bool(b) => Json::Bool(*b),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                Json::Int(i)
            } else if let Some(f) = n.as_f64() {
                Json::Float(f)
            } else {
                Json::Null
            }
        }
        Y::String(s) => Json::Str(s.clone()),
        Y::Sequence(items) => Json::Arr(items.iter().map(yaml_to_json).collect()),
        Y::Mapping(m) => {
            let mut out = Json::obj();
            for (k, val) in m {
                let key = match yaml_to_json(k) {
                    Json::Str(s) => s,
                    other => other.py_repr(),
                };
                out.set(&key, yaml_to_json(val));
            }
            out
        }
        Y::Tagged(t) => yaml_to_json(&t.value),
    }
}
