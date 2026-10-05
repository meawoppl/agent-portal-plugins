//! Vias hidden under bottom-terminated package bodies (port of
//! `kicad_tools.validate.rules.via_under_body`).

use super::clearance::{pad_polygon, uuid8};
use crate::analysis::routing_quality::median;
use crate::geometry::package_body::{footprint_side, package_body_polygon};
use crate::geometry::pcb_adapters::from_geo_multi;
use crate::geometry::shapely::{self as sh, Geom};
use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::schema::pcb::{Footprint, Pad, Pcb, Via};
use crate::utils::pymath::py_sum;
use crate::validate::rules::DRC_TOLERANCE;
use crate::validate::violations::{DRCResults, DRCViolation};

pub const VIA_UNDER_BODY_RULE_ID: &str = "via_under_body";
const EXPOSED_PAD_AREA_RATIO: f64 = 4.0;
const EXPOSED_PAD_NUMBERS: &[&str] = &["EP", "PAD", "TAB"];

/// `DEFAULT_FOOTPRINT_PATTERN` (`QFN|DFN|LGA|(?:^|[^A-Z])[A-Z]{0,2}SON(?![A-Z])`,
/// case-insensitive) evaluated without look-around.
pub fn default_pattern_matches(name: &str) -> bool {
    let u: Vec<char> = name.to_ascii_uppercase().chars().collect();
    let s: String = u.iter().collect();
    if s.contains("QFN") || s.contains("DFN") || s.contains("LGA") {
        return true;
    }
    let letter = |c: char| c.is_ascii_alphabetic();
    for i in 0..u.len().saturating_sub(2) {
        if u[i] != 'S' || u[i + 1] != 'O' || u[i + 2] != 'N' {
            continue;
        }
        if u.get(i + 3).is_some_and(|c| letter(*c)) {
            continue;
        }
        for k in 0..=2usize {
            if k > i {
                break;
            }
            let j = i - k;
            if !(j..i).all(|t| letter(u[t])) {
                continue;
            }
            if j == 0 || !letter(u[j - 1]) {
                return true;
            }
        }
    }
    false
}

fn has_copper(p: &Pad) -> bool {
    p.layers.iter().any(|l| l.ends_with(".Cu"))
}

/// `exposed_pads(footprint)`.
pub fn exposed_pads(fp: &Footprint) -> Vec<&Pad> {
    let copper: Vec<&Pad> = fp
        .pads
        .iter()
        .filter(|p| p.pad_type == "smd" && has_copper(p))
        .collect();
    if copper.is_empty() {
        return vec![];
    }
    let areas: Vec<f64> = copper.iter().map(|p| p.size.0 * p.size.1).collect();
    let typical = (copper.len() >= 3).then(|| median(&areas));
    let threshold = typical
        .filter(|t| *t > 0.0)
        .map(|t| EXPOSED_PAD_AREA_RATIO * t);
    let mut split: Vec<usize> = Vec::new();
    if let (Some(th), Some(t)) = (threshold, typical) {
        let mut clusters: Vec<(i64, Vec<usize>)> = Vec::new();
        for (i, (p, a)) in copper.iter().zip(&areas).enumerate() {
            if p.net_number != 0 && *a > t {
                match clusters.iter_mut().find(|(n, _)| *n == p.net_number) {
                    Some(e) => e.1.push(i),
                    None => clusters.push((p.net_number, vec![i])),
                }
            }
        }
        for (_, m) in clusters {
            if m.len() >= 2 && py_sum(m.iter().map(|&i| areas[i])) >= th {
                split.extend(m);
            }
        }
    }
    copper
        .iter()
        .zip(&areas)
        .enumerate()
        .filter(|(i, (p, a))| {
            threshold.is_some_and(|th| **a >= th)
                || split.contains(i)
                || EXPOSED_PAD_NUMBERS.contains(&p.number.to_uppercase().as_str())
        })
        .map(|(_, (p, _))| *p)
        .collect()
}

#[derive(Debug, Clone)]
pub struct ViaUnderBodyRule {
    pub include_references: Vec<String>,
    pub exclude_references: Vec<String>,
    pub allow_thermal_pad_vias: bool,
    pub fallback_to_courtyard: bool,
    pub severity: &'static str,
}

impl Default for ViaUnderBodyRule {
    fn default() -> Self {
        ViaUnderBodyRule {
            include_references: vec![],
            exclude_references: vec![],
            allow_thermal_pad_vias: true,
            fallback_to_courtyard: true,
            severity: "warning",
        }
    }
}

type Body<'a> = (&'a Footprint, Geom, &'static str, Vec<(i64, Geom)>);

impl ViaUnderBodyRule {
    pub fn selects(&self, fp: &Footprint) -> bool {
        if self.exclude_references.contains(&fp.reference) {
            return false;
        }
        if self.include_references.contains(&fp.reference) {
            return true;
        }
        default_pattern_matches(&fp.name)
    }

    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        let mut bodies: Vec<Body> = Vec::new();
        for fp in pcb.footprints() {
            if !self.selects(fp) {
                continue;
            }
            let Some((body, source)) = package_body_polygon(fp, self.fallback_to_courtyard) else {
                continue;
            };
            bodies.push((fp, from_geo_multi(&body), source, self.thermal_pads(fp)));
        }
        if bodies.is_empty() {
            return results;
        }
        for via in pcb.vias() {
            let r = via.size / 2.0;
            if r <= DRC_TOLERANCE {
                continue;
            }
            let center = Geom::Point(via.position);
            for (fp, body, source, thermal) in &bodies {
                let layer = format!("{}.Cu", footprint_side(*fp));
                if !via.layers.is_empty() && !via.layers.contains(&layer) {
                    continue;
                }
                if sh::distance(body, &center) >= r - DRC_TOLERANCE {
                    continue;
                }
                if thermal
                    .iter()
                    .any(|(n, poly)| via.net_number == *n && sh::covers_point(poly, via.position))
                {
                    continue;
                }
                results.add(self.violation(via, fp, source));
            }
        }
        results
    }

    fn thermal_pads(&self, fp: &Footprint) -> Vec<(i64, Geom)> {
        if !self.allow_thermal_pad_vias {
            return vec![];
        }
        exposed_pads(fp)
            .into_iter()
            .filter(|p| p.net_number != 0)
            .filter_map(|p| pad_polygon(p, fp).map(|g| (p.net_number, g)))
            .collect()
    }

    fn violation(&self, via: &Via, fp: &Footprint, source: &str) -> DRCViolation {
        let vref = if via.uuid.is_empty() {
            "Via".to_string()
        } else {
            format!("Via-{}", uuid8(&via.uuid))
        };
        let net_text = if via.net_name.is_empty() {
            String::new()
        } else {
            format!(" (net '{}')", via.net_name)
        };
        let outline = if source == "fab" {
            "fab outline"
        } else {
            "courtyard (no fab outline)"
        };
        let mut v = DRCViolation::new(
            VIA_UNDER_BODY_RULE_ID,
            self.severity,
            format!(
                "{vref}{net_text} (size {:.3}mm) is under the package body of {} ({}, {outline}); \
                 it cannot be probed, inspected or reworked after assembly -- move it outside the \
                 body or waive via .kct_waivers.json if intentional",
                via.size, fp.reference, fp.name
            ),
        )
        .at(py_round(via.position.0, 3), py_round(via.position.1, 3))
        .layer(fp.layer.clone())
        .actual(py_round(via.size, 4))
        .items([vref, fp.reference.clone()]);
        if !via.net_name.is_empty() {
            v = v.nets([via.net_name.clone()]);
        }
        v
    }
}
