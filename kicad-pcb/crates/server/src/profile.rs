//! PCB layout-quality profile (`pcb-profile.yaml`).
//!
//! The profile is a user preference file shared with kicad-tools. It is kept
//! as a generic YAML tree so rule severities can be looked up by dotted key
//! (`vias.in_pad`), plus a typed view of the numeric parameters the in-plugin
//! audits need. Unknown keys are ignored so the profile can grow.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde_yaml::Value;

#[derive(Debug, Clone)]
pub(crate) struct Profile {
    pub name: Option<String>,
    tree: Value,
}

/// Severity a profile value maps to. `Off` suppresses matching findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Error,
    Warning,
    Info,
    Off,
}

impl Level {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Info => "info",
            Level::Off => "off",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Level> {
        match value.trim().to_ascii_lowercase().as_str() {
            "error" | "forbid" | "forbidden" | "required" | "require" | "never" => {
                Some(Level::Error)
            }
            "warn" | "warning" => Some(Level::Warning),
            "info" | "note" => Some(Level::Info),
            "off" | "allow" | "allowed" | "ignore" | "none" => Some(Level::Off),
            _ => None,
        }
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: None,
            tree: Value::Mapping(Default::default()),
        }
    }
}

impl Profile {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let tree: Value = serde_yaml::from_str(text).context("parse quality profile YAML")?;
        if !tree.is_mapping() {
            return Err(anyhow!("quality profile must be a YAML mapping"));
        }
        if let Some(version) = tree.get("version").and_then(Value::as_u64) {
            if version != 1 {
                return Err(anyhow!(
                    "unsupported quality profile version {version}; expected 1"
                ));
            }
        }
        Ok(Self {
            name: tree.get("name").and_then(Value::as_str).map(str::to_string),
            tree,
        })
    }

    pub(crate) fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("profile {}", path.display()))
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        key.split('.')
            .try_fold(&self.tree, |node, part| node.get(part))
    }

    /// Severity for a profile item when its value is a severity word
    /// (`error`, `warn`, `forbid`, ...). `false` disables the item; any other
    /// value (a parameter, `true`, a list) leaves the caller's default.
    pub(crate) fn level(&self, key: &str) -> Option<Level> {
        match self.get(key)? {
            Value::String(value) => Level::parse(value),
            Value::Bool(false) => Some(Level::Off),
            _ => None,
        }
    }

    pub(crate) fn f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(Value::as_f64)
    }

    pub(crate) fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }

    pub(crate) fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    pub(crate) fn strings(&self, key: &str) -> Option<Vec<String>> {
        let seq = self.get(key)?.as_sequence()?;
        Some(
            seq.iter()
                .filter_map(|item| match item {
                    Value::String(value) => Some(value.clone()),
                    Value::Number(value) => Some(value.to_string()),
                    _ => None,
                })
                .collect(),
        )
    }

    /// Package families whose body must stay free of vias.
    pub(crate) fn body_keepout_packages(&self) -> Vec<String> {
        self.strings("routing.no_front_routing_under")
            .unwrap_or_else(|| vec!["QFN".into(), "DFN".into(), "BGA".into()])
            .into_iter()
            .map(|item| item.to_ascii_uppercase())
            .collect()
    }

    pub(crate) fn silk_reference_prefixes(&self) -> Option<Vec<String>> {
        self.strings("silkscreen.reference_prefixes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
version: 1
name: sample
routing:
  octilinear_only: true
  stubs: error
  no_front_routing_under: [QFN, bga]
vias:
  in_pad: forbid
  redundant_ground: warn
  orphans: error
decoupling:
  max_pin_distance_mm: 2.0
  same_side_as_ic: true
silkscreen:
  reference_prefixes: [U, J, SW]
  text_height_mm: 1.0
  explanatory_text: forbid
  pin1_dots: polarized_only
"#;

    #[test]
    fn parses_levels_and_parameters() {
        let profile = Profile::parse(SAMPLE).unwrap();
        assert_eq!(profile.name.as_deref(), Some("sample"));
        assert_eq!(profile.level("vias.in_pad"), Some(Level::Error));
        assert_eq!(profile.level("vias.redundant_ground"), Some(Level::Warning));
        assert_eq!(profile.level("routing.octilinear_only"), None);
        assert_eq!(profile.level("silkscreen.pin1_dots"), None);
        assert_eq!(profile.level("missing.key"), None);
        assert_eq!(profile.f64("decoupling.max_pin_distance_mm"), Some(2.0));
        assert_eq!(profile.bool("decoupling.same_side_as_ic"), Some(true));
        assert_eq!(profile.silk_reference_prefixes().unwrap(), ["U", "J", "SW"]);
        assert_eq!(profile.body_keepout_packages(), ["QFN", "BGA"]);
    }

    #[test]
    fn false_disables_and_defaults_apply() {
        let profile = Profile::parse("vias:\n  orphans: false\n").unwrap();
        assert_eq!(profile.level("vias.orphans"), Some(Level::Off));
        assert_eq!(profile.body_keepout_packages(), ["QFN", "DFN", "BGA"]);
        assert!(profile.silk_reference_prefixes().is_none());
    }

    #[test]
    fn rejects_bad_profiles() {
        assert!(Profile::parse("- a\n- b\n").is_err());
        assert!(Profile::parse("version: 2\n").is_err());
        assert!(Profile::parse("a: [").is_err());
    }

    #[test]
    fn parses_example_profile() {
        let profile = Profile::parse(include_str!("../../../examples/pcb-profile.yaml")).unwrap();
        assert_eq!(profile.level("vias.in_pad"), Some(Level::Error));
        assert_eq!(profile.level("vias.orphans"), Some(Level::Error));
        assert_eq!(
            profile.level("silkscreen.explanatory_text"),
            Some(Level::Error)
        );
        assert_eq!(profile.str("manufacturing.fab"), Some("jlcpcb"));
        assert_eq!(
            profile.silk_reference_prefixes().unwrap(),
            ["U", "J", "SW", "SJ", "AE", "Y"]
        );
    }

    #[test]
    fn parses_bp_test_style_profile() {
        let text = "version: 1\nparts:\n  min_passive_imperial: \"0402\"\nmanufacturing:\n  rf_cpwg_50ohm: {trace_mm: 0.26, gap_mm: 0.15}\n  release:\n    tag: true\n";
        let profile = Profile::parse(text).unwrap();
        assert_eq!(profile.str("parts.min_passive_imperial"), Some("0402"));
        assert_eq!(
            profile.f64("manufacturing.rf_cpwg_50ohm.trace_mm"),
            Some(0.26)
        );
    }
}
