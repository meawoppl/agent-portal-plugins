//! N-trace match-group length skew (port of
//! `kicad_tools.validate.rules.match_group_length_skew`, issue #2702).

use crate::manufacturers::DesignRules;
use crate::pyjson::{py_repr_str, py_round};
use crate::router::match_group_detection::MatchGroup;
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

pub const DEFAULT_MATCH_GROUP_TOLERANCE_MM: f64 = 0.5;

/// Upstream `MatchGroupLengthSkewRule`.
#[derive(Debug, Clone)]
pub struct MatchGroupLengthSkewRule {
    group_skew_data: Vec<(String, f64)>,
    groups: Vec<MatchGroup>,
    threshold_map: Vec<(String, f64)>,
    default_tolerance_mm: f64,
    emit_info: bool,
}

impl Default for MatchGroupLengthSkewRule {
    fn default() -> Self {
        MatchGroupLengthSkewRule::new(vec![], vec![], vec![], false)
    }
}

impl MatchGroupLengthSkewRule {
    pub fn new(
        group_skew_data: Vec<(String, f64)>,
        tracker_match_groups: Vec<MatchGroup>,
        threshold_map: Vec<(String, f64)>,
        emit_info: bool,
    ) -> Self {
        // `{grp.name: grp}`: a later group with the same name wins.
        let mut groups: Vec<MatchGroup> = Vec::new();
        for g in tracker_match_groups {
            match groups.iter_mut().find(|x| x.name == g.name) {
                Some(e) => *e = g,
                None => groups.push(g),
            }
        }
        MatchGroupLengthSkewRule {
            group_skew_data,
            groups,
            threshold_map,
            default_tolerance_mm: DEFAULT_MATCH_GROUP_TOLERANCE_MM,
            emit_info,
        }
    }

    pub fn check(&self, _pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        if self.group_skew_data.is_empty() || self.groups.is_empty() {
            return results;
        }
        let mut items = self.group_skew_data.clone();
        items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        for (name, skew) in items {
            let Some(g) = self.groups.iter().find(|g| g.name == name) else {
                continue;
            };
            results.rules_checked += 1;
            results.bump_rule("match_group_length_skew", 1);
            let tol = self
                .threshold_map
                .iter()
                .find(|e| e.0 == name)
                .map(|e| e.1)
                .unwrap_or(self.default_tolerance_mm);
            let members = g.net_ids.len() + 2 * g.pair_ids.len();
            let plural = if members != 1 { "s" } else { "" };
            let head = format!(
                "Match group {} ({members} member{plural}) length-skew {skew:.3} mm",
                py_repr_str(&g.name)
            );
            let (severity, tail) = if skew > tol {
                ("error", format!("exceeds tolerance {tol:.3} mm"))
            } else if self.emit_info {
                ("info", format!("within tolerance {tol:.3} mm"))
            } else {
                continue;
            };
            results.add(
                DRCViolation::new("match_group_length_skew", severity, format!("{head} {tail}"))
                    .actual(py_round(skew, 4))
                    .required(py_round(tol, 4))
                    .items([g.name.clone()]),
            );
        }
        results
    }
}
