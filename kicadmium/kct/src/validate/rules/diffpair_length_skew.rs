//! Engaged differential-pair length skew (port of
//! `kicad_tools.validate.rules.diffpair_length_skew`, issue #2649).

use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

pub const DEFAULT_SKEW_TOLERANCE_MM: f64 = 0.5;
pub const SKEW_TOLERANCE_EPSILON_MM: f64 = 1e-6;

fn norm_names(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn norm_ids(a: i64, b: i64) -> (i64, i64) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Upstream `DiffPairLengthSkewRule`.
#[derive(Debug, Clone)]
pub struct DiffPairLengthSkewRule {
    skew_data: Vec<((String, String), f64)>,
    engaged: Vec<(i64, i64)>,
    threshold_map: Vec<((i64, i64), f64)>,
    default_tolerance_mm: f64,
    emit_info: bool,
}

impl Default for DiffPairLengthSkewRule {
    fn default() -> Self {
        DiffPairLengthSkewRule::new(vec![], vec![], vec![], DEFAULT_SKEW_TOLERANCE_MM, false)
    }
}

impl DiffPairLengthSkewRule {
    pub fn new(
        skew_data: Vec<((String, String), f64)>,
        engaged_pairs: Vec<(i64, i64)>,
        threshold_map: Vec<((i64, i64), f64)>,
        default_tolerance_mm: f64,
        emit_info: bool,
    ) -> Self {
        let mut sd: Vec<((String, String), f64)> = Vec::new();
        for ((a, b), s) in skew_data {
            let k = norm_names(&a, &b);
            match sd.iter_mut().find(|e| e.0 == k) {
                Some(e) => e.1 = s,
                None => sd.push((k, s)),
            }
        }
        let mut engaged: Vec<(i64, i64)> = Vec::new();
        for (a, b) in engaged_pairs {
            let k = norm_ids(a, b);
            if !engaged.contains(&k) {
                engaged.push(k);
            }
        }
        let mut tm: Vec<((i64, i64), f64)> = Vec::new();
        for ((a, b), t) in threshold_map {
            let k = norm_ids(a, b);
            match tm.iter_mut().find(|e| e.0 == k) {
                Some(e) => e.1 = t,
                None => tm.push((k, t)),
            }
        }
        DiffPairLengthSkewRule {
            skew_data: sd,
            engaged,
            threshold_map: tm,
            default_tolerance_mm,
            emit_info,
        }
    }

    pub fn check(&self, pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        if self.skew_data.is_empty() || self.engaged.is_empty() {
            return results;
        }
        // `{net.name: net.number}`: last net with a name wins.
        let id_of = |name: &str| {
            pcb.nets()
                .iter()
                .rev()
                .find(|n| !n.name.is_empty() && n.name == name)
                .map(|n| n.number)
        };
        let mut items = self.skew_data.clone();
        items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        for ((na, nb), skew) in items {
            let (Some(ia), Some(ib)) = (id_of(&na), id_of(&nb)) else {
                continue;
            };
            let k = norm_ids(ia, ib);
            if !self.engaged.contains(&k) {
                continue;
            }
            results.rules_checked += 1;
            results.bump_rule("diffpair_length_skew", 1);
            let tol = self
                .threshold_map
                .iter()
                .find(|e| e.0 == k)
                .map(|e| e.1)
                .unwrap_or(self.default_tolerance_mm);
            let (first, second) = norm_names(&na, &nb);
            if skew > tol + SKEW_TOLERANCE_EPSILON_MM {
                results.add(
                    DRCViolation::new(
                        "diffpair_length_skew",
                        "error",
                        format!(
                            "Engaged differential pair {first}/{second} length-skew {skew:.3} mm \
                             exceeds tolerance {tol:.3} mm"
                        ),
                    )
                    .actual(py_round(skew, 4))
                    .required(py_round(tol, 4))
                    .items([first.clone(), second.clone()])
                    .nets([na.clone(), nb.clone()]),
                );
            } else if self.emit_info {
                results.add(
                    DRCViolation::new(
                        "diffpair_length_skew",
                        "info",
                        format!(
                            "Engaged differential pair {first}/{second} length-skew {skew:.3} mm \
                             within tolerance {tol:.3} mm"
                        ),
                    )
                    .actual(py_round(skew, 4))
                    .required(py_round(tol, 4))
                    .nets([first.clone(), second.clone()]),
                );
            }
        }
        results
    }
}
