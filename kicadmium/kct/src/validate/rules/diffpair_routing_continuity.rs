//! Engaged differential-pair routing continuity (port of
//! `kicad_tools.validate.rules.diffpair_routing_continuity`, issue #2640).

use super::clearance::{segment_segment_clearance, CopperElement};
use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::schema::pcb::Pcb;
use crate::utils::pymath::{hypot, py_sum};
use crate::utils::pyset::PySetOrder;
use crate::validate::violations::{DRCResults, DRCViolation};

pub const DEFAULT_COUPLED_CONTINUITY_THRESHOLD: f64 = 0.7;
pub const DEFAULT_COUPLING_WINDOW_MM: f64 = 0.5;
pub const DEFAULT_PARALLEL_TOLERANCE_DEG: f64 = 15.0;

fn seg_len(s: &CopperElement) -> f64 {
    let g = &s.geometry;
    hypot(g[2] - g[0], g[3] - g[1])
}

fn seg_angle(s: &CopperElement) -> f64 {
    let g = &s.geometry;
    let mut raw = (g[3] - g[1]).atan2(g[2] - g[0]).to_degrees();
    if raw < 0.0 {
        raw += 180.0;
    }
    if raw >= 180.0 {
        raw -= 180.0;
    }
    raw
}

fn angle_diff(a: f64, b: f64) -> f64 {
    let raw = (a - b).abs();
    if raw > 90.0 {
        180.0 - raw
    } else {
        raw
    }
}

fn coupled_overlap(p: &CopperElement, n: &CopperElement, window: f64, tol_deg: f64) -> f64 {
    if p.layer != n.layer {
        return 0.0;
    }
    if angle_diff(seg_angle(p), seg_angle(n)) > tol_deg {
        return 0.0;
    }
    let (clearance, _, _) = segment_segment_clearance(p, n);
    if clearance > window + 1e-6 {
        return 0.0;
    }
    seg_len(p)
}

/// Python `f"{x:.1%}"`.
fn pct1(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

/// Upstream `DiffPairRoutingContinuityRule`.
#[derive(Debug, Clone)]
pub struct DiffPairRoutingContinuityRule {
    /// Engaged `(min, max)` pairs in CPython set iteration order.
    engaged: Vec<(i64, i64)>,
    threshold_map: Vec<((i64, i64), f64)>,
    coupling_window_mm: f64,
    parallel_tolerance_deg: f64,
    default_threshold: f64,
    emit_info: bool,
}

impl Default for DiffPairRoutingContinuityRule {
    fn default() -> Self {
        DiffPairRoutingContinuityRule::new(&PySetOrder::new(), vec![], false)
    }
}

impl DiffPairRoutingContinuityRule {
    /// Keys of `engaged_pairs` are already `(min, max)`-normalized (as
    /// `derive_engagement_state` produces them), so re-normalizing into a
    /// fresh set preserves the source set's iteration order.
    pub fn new(
        engaged_pairs: &PySetOrder<(i64, i64)>,
        threshold_map: Vec<((i64, i64), f64)>,
        emit_info: bool,
    ) -> Self {
        let engaged: Vec<(i64, i64)> =
            crate::utils::pyset::int_pair_set(engaged_pairs.iter().map(|&(a, b)| {
                if a <= b {
                    (a, b)
                } else {
                    (b, a)
                }
            }))
            .iter()
            .copied()
            .collect();
        let mut tm: Vec<((i64, i64), f64)> = Vec::new();
        for ((a, b), t) in threshold_map {
            let k = if a <= b { (a, b) } else { (b, a) };
            match tm.iter_mut().find(|e| e.0 == k) {
                Some(e) => e.1 = t,
                None => tm.push((k, t)),
            }
        }
        DiffPairRoutingContinuityRule {
            engaged,
            threshold_map: tm,
            coupling_window_mm: DEFAULT_COUPLING_WINDOW_MM,
            parallel_tolerance_deg: DEFAULT_PARALLEL_TOLERANCE_DEG,
            default_threshold: DEFAULT_COUPLED_CONTINUITY_THRESHOLD,
            emit_info,
        }
    }

    fn coupled_length(&self, own: &[CopperElement], partner: &[CopperElement]) -> f64 {
        let mut total = 0.0;
        for o in own {
            let mut best = 0.0;
            for p in partner {
                let ov =
                    coupled_overlap(o, p, self.coupling_window_mm, self.parallel_tolerance_deg);
                if ov > best {
                    best = ov;
                    if best >= seg_len(o) {
                        break;
                    }
                }
            }
            total += best;
        }
        total
    }

    pub fn check(&self, pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        results.rules_checked = self.engaged.len() as i64;
        if self.engaged.is_empty() {
            return results;
        }
        results.set_rule("diffpair_routing_continuity", self.engaged.len() as i64);
        let mut by_net: Vec<(i64, Vec<CopperElement>)> = Vec::new();
        for layer in pcb.copper_layers() {
            for seg in pcb.segments_on_layer(&layer.name) {
                if seg.net_number == 0 {
                    continue;
                }
                let e = CopperElement::from_segment(seg);
                match by_net.iter_mut().find(|(n, _)| *n == seg.net_number) {
                    Some(v) => v.1.push(e),
                    None => by_net.push((seg.net_number, vec![e])),
                }
            }
        }
        let empty: Vec<CopperElement> = Vec::new();
        let segs = |n: i64| {
            by_net
                .iter()
                .find(|(k, _)| *k == n)
                .map(|(_, v)| v)
                .unwrap_or(&empty)
        };
        // `{n.number: n.name}`: last entry per number wins.
        let name_of = |id: i64| {
            pcb.nets()
                .iter()
                .rev()
                .find(|n| n.number == id)
                .map(|n| n.name.clone())
                .unwrap_or_default()
        };
        for &(a, b) in &self.engaged {
            let (sa, sb) = (segs(a), segs(b));
            let ta = py_sum(sa.iter().map(seg_len));
            let tb = py_sum(sb.iter().map(seg_len));
            if ta <= 0.0 || tb <= 0.0 {
                continue;
            }
            let fa = (self.coupled_length(sa, sb) / ta).min(1.0);
            let fb = (self.coupled_length(sb, sa) / tb).min(1.0);
            let frac = (fa + fb) / 2.0;
            let threshold = self
                .threshold_map
                .iter()
                .find(|e| e.0 == (a, b))
                .map(|e| e.1)
                .unwrap_or(self.default_threshold);
            let (na, nb) = (name_of(a), name_of(b));
            if frac + 1e-9 < threshold {
                let la = if na.is_empty() {
                    format!("net-{a}")
                } else {
                    na.clone()
                };
                let lb = if nb.is_empty() {
                    format!("net-{b}")
                } else {
                    nb.clone()
                };
                let (first, second) = if la <= lb { (la, lb) } else { (lb, la) };
                results.add(
                    DRCViolation::new(
                        "diffpair_routing_continuity",
                        "error",
                        format!(
                            "Engaged differential pair {first}/{second} routing continuity {} \
                             below threshold {}",
                            pct1(frac),
                            pct1(threshold)
                        ),
                    )
                    .actual(py_round(frac, 4))
                    .required(py_round(threshold, 4))
                    .items([first.clone(), second.clone()])
                    .nets([na, nb]),
                );
            } else if self.emit_info {
                let (first, second) = if na <= nb { (na, nb) } else { (nb, na) };
                results.add(
                    DRCViolation::new(
                        "diffpair_routing_continuity",
                        "info",
                        format!(
                            "Engaged differential pair {first}/{second} coupled {:.1}% of its \
                             length (>= {:.1}% required)",
                            frac * 100.0,
                            threshold * 100.0
                        ),
                    )
                    .actual(py_round(frac, 4))
                    .required(py_round(threshold, 4))
                    .nets([first, second]),
                );
            }
        }
        results
    }
}
