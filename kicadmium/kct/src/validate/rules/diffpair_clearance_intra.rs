//! Differential-pair within-pair clearance (port of
//! `kicad_tools.validate.rules.diffpair_clearance_intra`, issue #2560).
//!
//! Segment-to-segment only: two segments on the P/N nets of one detected
//! pair must keep at least the pair's `intra_pair_clearance` (falling back
//! to the manufacturer `min_clearance_mm`).

use super::clearance::{segment_segment_clearance, CopperElement};
use super::DRC_TOLERANCE;
use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::router::diffpair::detect_differential_pairs;
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

fn key(a: i64, b: i64) -> (i64, i64) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Upstream `DiffPairClearanceIntraRule`.
#[derive(Debug, Clone, Default)]
pub struct DiffPairClearanceIntraRule {
    /// `(min_net, max_net) -> threshold_mm`.
    pub intra_map: Vec<((i64, i64), f64)>,
    /// Explicit `(p_name, n_name)` pairs augmenting suffix inference.
    pub overrides: Vec<(String, String)>,
}

impl DiffPairClearanceIntraRule {
    pub fn new(intra_map: Vec<((i64, i64), f64)>, overrides: Vec<(String, String)>) -> Self {
        let mut m: Vec<((i64, i64), f64)> = Vec::new();
        for ((a, b), t) in intra_map {
            let k = key(a, b);
            match m.iter_mut().find(|e| e.0 == k) {
                Some(e) => e.1 = t,
                None => m.push((k, t)),
            }
        }
        DiffPairClearanceIntraRule {
            intra_map: m,
            overrides,
        }
    }

    fn build_diff_pair_set(&self, pcb: &Pcb) -> Vec<(i64, i64)> {
        let mut pairs: Vec<(i64, i64)> = Vec::new();
        let mut add = |k: (i64, i64)| {
            if !pairs.contains(&k) {
                pairs.push(k);
            }
        };
        if !self.overrides.is_empty() {
            // `{net.name: net.number}`: the last net with a name wins.
            let id = |name: &str| {
                pcb.nets()
                    .iter()
                    .rev()
                    .find(|n| n.name == name)
                    .map(|n| n.number)
            };
            for (p, n) in &self.overrides {
                let (Some(p), Some(n)) = (id(p), id(n)) else {
                    continue;
                };
                if p == 0 || n == 0 {
                    continue;
                }
                add(key(p, n));
            }
        }
        let names: Vec<(i64, String)> = pcb
            .nets()
            .iter()
            .map(|n| (n.number, n.name.clone()))
            .collect();
        for dp in detect_differential_pairs(&names) {
            let (p, n) = (dp.positive.net_id, dp.negative.net_id);
            if p == 0 || n == 0 {
                continue;
            }
            add(key(p, n));
        }
        pairs
    }

    pub fn check(&self, pcb: &Pcb, design_rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        let pairs = self.build_diff_pair_set(pcb);
        let layers = pcb.copper_layers();
        for layer in &layers {
            let segs: Vec<CopperElement> = pcb
                .segments_on_layer(&layer.name)
                .map(CopperElement::from_segment)
                .collect();
            for (i, e1) in segs.iter().enumerate() {
                for e2 in &segs[i + 1..] {
                    if e1.net_number == e2.net_number || e1.net_number == 0 || e2.net_number == 0 {
                        continue;
                    }
                    let k = key(e1.net_number, e2.net_number);
                    if !pairs.contains(&k) {
                        continue;
                    }
                    let threshold = self
                        .intra_map
                        .iter()
                        .find(|e| e.0 == k)
                        .map(|e| e.1)
                        .unwrap_or(design_rules.min_clearance_mm);
                    let (clearance, lx, ly) = segment_segment_clearance(e1, e2);
                    if clearance + DRC_TOLERANCE < threshold {
                        let mut names = [e1.net_name.clone(), e2.net_name.clone()];
                        names.sort();
                        results.add(
                            DRCViolation::new(
                                "diffpair_clearance_intra",
                                "error",
                                format!(
                                    "Within-pair clearance for diff pair {}/{}: {clearance:.3}mm < \
                                     intra_pair_clearance {threshold:.3}mm",
                                    names[0], names[1]
                                ),
                            )
                            .at(py_round(lx, 3), py_round(ly, 3))
                            .layer(layer.name.clone())
                            .actual(py_round(clearance, 4))
                            .required(threshold)
                            .items([e1.reference.clone(), e2.reference.clone()])
                            .nets([e1.net_name.clone(), e2.net_name.clone()]),
                        );
                    }
                }
            }
        }
        results.rules_checked = layers.len() as i64;
        if !pairs.is_empty() {
            results.set_rule("diffpair_clearance_intra", pairs.len() as i64);
        }
        results
    }
}
