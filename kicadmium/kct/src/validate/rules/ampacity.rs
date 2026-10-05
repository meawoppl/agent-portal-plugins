//! Ampacity validation (port of `kicad_tools.validate.rules.ampacity`,
//! issue #4217): every routed segment of a net with a `target_ampacity`
//! must be at least the IPC-2221 width for the board's copper weight.

use crate::manufacturers::DesignRules;
use crate::physics::ampacity::width_for_current;
use crate::pyjson::{py_float_repr, py_round};
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

/// Outer copper layers (IPC-2221 k = 0.048); everything else is internal.
const EXTERNAL_LAYERS: [&str; 2] = ["F.Cu", "B.Cu"];

/// Upstream `AmpacityRule`.
#[derive(Debug, Clone, Default)]
pub struct AmpacityRule {
    /// `{net_name: target_ampacity}` (insertion ordered).
    pub specs: Vec<(String, f64)>,
}

impl AmpacityRule {
    pub fn new(specs: Vec<(String, f64)>) -> Self {
        AmpacityRule { specs }
    }

    pub fn is_external_layer(layer: &str) -> bool {
        EXTERNAL_LAYERS.contains(&layer)
    }

    fn required_width_mm(current_a: f64, rules: &DesignRules, external: bool) -> f64 {
        let (oz, layer) = if external {
            (rules.outer_copper_oz, "external")
        } else {
            (rules.inner_copper_oz, "internal")
        };
        width_for_current(current_a, oz, 10.0, layer).unwrap_or(f64::NAN)
    }

    pub fn check(&self, pcb: &Pcb, design_rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        if self.specs.is_empty() {
            return results;
        }
        let mut cache: Vec<((String, bool), f64)> = Vec::new();
        for seg in pcb.segments() {
            let net = seg.net_name.as_str();
            if net.is_empty() {
                continue;
            }
            // Later duplicate keys win, as in a Python dict.
            let Some(current_a) = self.specs.iter().rev().find(|(n, _)| n == net).map(|s| s.1)
            else {
                continue;
            };
            let layer = seg.layer.as_str();
            let external = Self::is_external_layer(layer);
            let required = match cache.iter().find(|(k, _)| k.0 == net && k.1 == external) {
                Some((_, w)) => *w,
                None => {
                    let w = Self::required_width_mm(current_a, design_rules, external);
                    cache.push(((net.to_string(), external), w));
                    w
                }
            };
            let actual = seg.width;
            if actual >= required {
                continue;
            }
            let layer_class = if external { "external" } else { "internal" };
            let oz = if external {
                design_rules.outer_copper_oz
            } else {
                design_rules.inner_copper_oz
            };
            results.add(
                DRCViolation::new(
                    "ampacity",
                    "error",
                    format!(
                        "Trace on {layer} too narrow for {current_a:.1}A: width {actual:.3}mm, \
                         requires {required:.3}mm (IPC-2221, {}oz {layer_class})",
                        py_float_repr(oz)
                    ),
                )
                .at(
                    py_round((seg.start.0 + seg.end.0) / 2.0, 3),
                    py_round((seg.start.1 + seg.end.1) / 2.0, 3),
                )
                .layer(layer)
                .actual(actual)
                .required(required)
                .items([net.to_string()]),
            );
        }
        results
    }
}
