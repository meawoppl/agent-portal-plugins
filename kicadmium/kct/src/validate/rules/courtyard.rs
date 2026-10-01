//! Courtyard-overlap rule with pair-level waivers (port of
//! `kicad_tools.validate.rules.courtyard`, Issue #4137).

use super::courtyard_waivers::CourtyardWaivers;
use crate::geometry::courtyard::{has_courtyard_geometry, side_has_geometry};
use crate::geometry::pcb_adapters::courtyard_geom;
use crate::geometry::shapely::{self as sh, Geom};
use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

pub const COURTYARD_RULE_ID: &str = "courtyards_overlap";
pub const COURTYARD_UNRESOLVED_RULE_ID: &str = "courtyard_outline_unresolved";
pub const COURTYARD_UNUSED_WAIVER_RULE_ID: &str = "courtyard_waiver_unused";

#[derive(Debug, Clone, Default)]
pub struct CourtyardOverlapRule {
    pub waivers: Option<CourtyardWaivers>,
}

impl CourtyardOverlapRule {
    pub fn new(waivers: Option<CourtyardWaivers>) -> Self {
        CourtyardOverlapRule { waivers }
    }

    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        results.rules_checked += 1;
        let mut polys: Vec<((String, &str), Geom)> = Vec::new();
        for fp in pcb.footprints() {
            let has = has_courtyard_geometry(fp);
            let mut resolved = false;
            for side in ["F", "B"] {
                if !side_has_geometry(fp, side) {
                    continue;
                }
                if let Some(p) = courtyard_geom(fp, side) {
                    let key = (fp.reference.clone(), side);
                    match polys.iter_mut().find(|(k, _)| *k == key) {
                        Some(e) => e.1 = p,
                        None => polys.push((key, p)),
                    }
                    resolved = true;
                }
            }
            if has && !resolved {
                results.add(
                    DRCViolation::new(
                        COURTYARD_UNRESOLVED_RULE_ID,
                        "info",
                        format!(
                            "Could not resolve courtyard outline for {} (unsupported or \
                             non-closing courtyard geometry); pair overlap not checked for this \
                             footprint",
                            fp.reference
                        ),
                    )
                    .at(fp.position.0, fp.position.1)
                    .layer(fp.layer.clone())
                    .items([fp.reference.clone()]),
                );
            }
        }
        let bounds: Vec<_> = polys
            .iter()
            .map(|(_, g)| g.bounds().unwrap_or((0.0, 0.0, 0.0, 0.0)))
            .collect();
        for (i, j) in crate::validate::spatial::candidate_pairs(&bounds, 0.0) {
            let ((ra, sa), pa) = &polys[i];
            let ((rb, sb), pb) = &polys[j];
            if sa != sb || ra == rb || !sh::intersects(pa, pb) {
                continue;
            }
            let inter = sh::intersection(pa, pb);
            let area = inter.area();
            if inter.is_empty() || area <= 0.0 {
                continue;
            }
            self.emit_overlap(&mut results, ra, rb, sa, area);
        }
        if let Some(w) = &self.waivers {
            let present: Vec<&str> = pcb.footprints().iter().map(|f| f.reference.as_str()).collect();
            for e in &w.entries {
                let missing: Vec<&str> = [e.refs.0.as_str(), e.refs.1.as_str()]
                    .into_iter()
                    .filter(|r| !present.contains(r))
                    .collect();
                if !missing.is_empty() {
                    results.add(
                        DRCViolation::new(
                            COURTYARD_UNUSED_WAIVER_RULE_ID,
                            "info",
                            format!(
                                "Unused courtyard waiver for {}/{}: component(s) {} not present on \
                                 the board (stale waiver -- consider pruning)",
                                e.refs.0,
                                e.refs.1,
                                missing.join(", ")
                            ),
                        )
                        .items([e.refs.0.clone(), e.refs.1.clone()]),
                    );
                }
            }
        }
        results
    }

    fn emit_overlap(&self, results: &mut DRCResults, a: &str, b: &str, side: &str, area: f64) {
        let layer = if side == "F" { "F.CrtYd" } else { "B.CrtYd" };
        let base = format!("Courtyards of {a} and {b} overlap ({area:.3} mm^2) on {layer}");
        let waiver = self.waivers.as_ref().and_then(|w| w.find(a, b));
        let mut v = DRCViolation::new(
            COURTYARD_RULE_ID,
            "error",
            match waiver {
                Some(w) => format!("{base} [WAIVED: {}]", w.reason),
                None => base,
            },
        )
        .layer(layer)
        .actual(py_round(area, 4))
        .items([a.to_string(), b.to_string()]);
        if let Some(w) = waiver {
            v.waived = true;
            v.waiver_reason = Some(w.reason.clone());
            v.waiver_issue = Some(w.issue.clone());
        }
        results.add(v);
    }
}
