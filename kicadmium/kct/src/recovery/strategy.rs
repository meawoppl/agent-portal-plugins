//! Port of `kicad_tools.recovery.strategy`: generate ranked resolution
//! strategies (moves, vias, reroutes, spreads, layer changes, manual
//! fallback) from a failure analysis.
//!
//! Upstream builds a few lists from Python `set`s of net names (hash-seed
//! dependent order); here those lists are sorted so output is deterministic.

use std::collections::BTreeSet;

use super::types::{
    Action, BlockingElement, Difficulty, FailureAnalysis, FailureCause, Rectangle,
    ResolutionStrategy, SideEffect, StrategyType,
};
use crate::pyjson::Json;
use crate::schema::pcb::{Footprint, Pcb};

/// A candidate position for moving a component.
#[derive(Debug, Clone, PartialEq)]
pub struct MoveCandidate {
    pub position: (f64, f64),
    pub confidence: f64,
    pub improvement: f64,
    pub distance: f64,
}

/// Generates resolution strategies from failure analysis.
#[derive(Debug, Clone, Copy, Default)]
pub struct StrategyGenerator;

fn obj(pairs: &[(&str, Json)]) -> Json {
    let mut d = Json::obj();
    for (k, v) in pairs {
        d.set(k, v.clone());
    }
    d
}

impl StrategyGenerator {
    pub const BYPASS_CAP_DISTANCE_THRESHOLD: f64 = 5.0;
    pub const CLUSTER_DISTANCE_THRESHOLD: f64 = 10.0;

    pub fn new() -> Self {
        StrategyGenerator
    }

    /// Ranked strategies (difficulty, then confidence, then side effects).
    /// `max_movement` bounds per-component move candidates.
    pub fn generate_strategies(
        &self,
        pcb: &Pcb,
        failure: &FailureAnalysis,
        max_movement: Option<f64>,
    ) -> Vec<ResolutionStrategy> {
        let mut out = Vec::new();
        if failure.has_movable_blockers() {
            out.extend(self.generate_move_strategies(pcb, failure, max_movement));
        }
        if matches!(
            failure.root_cause,
            FailureCause::Congestion | FailureCause::BlockedPath
        ) {
            out.extend(self.generate_via_strategies(pcb, failure));
        }
        if failure.has_reroutable_nets() {
            out.extend(self.generate_reroute_strategies(failure));
        }
        if failure.root_cause == FailureCause::Congestion {
            out.extend(self.generate_spread_strategies(pcb, failure));
        }
        if failure.root_cause == FailureCause::LayerConflict {
            out.extend(self.generate_layer_change_strategies(pcb, failure));
        }
        if out.is_empty()
            || matches!(
                failure.root_cause,
                FailureCause::Keepout
                    | FailureCause::LengthConstraint
                    | FailureCause::DifferentialPair
            )
        {
            out.push(self.generate_manual_strategy(failure));
        }
        Self::rank_strategies(out)
    }

    pub fn generate_move_strategies(
        &self,
        pcb: &Pcb,
        failure: &FailureAnalysis,
        max_movement: Option<f64>,
    ) -> Vec<ResolutionStrategy> {
        let mut out = Vec::new();
        for b in &failure.blocking_elements {
            if !b.movable || b.kind != "component" {
                continue;
            }
            let Some(r) = &b.reference else {
                continue;
            };
            let cands = self.find_move_candidates(pcb, b, &failure.failure_area, max_movement);
            for c in cands.iter().take(3) {
                let effects = self.analyze_move_side_effects(pcb, r, c.position);
                let nets = self.get_component_nets(pcb, r);
                let mut s = ResolutionStrategy::new(
                    StrategyType::MoveComponent,
                    Self::assess_move_difficulty(&effects),
                    c.confidence,
                );
                s.actions = vec![Action::new(
                    "move",
                    r.as_str(),
                    obj(&[
                        ("x", Json::Float(c.position.0)),
                        ("y", Json::Float(c.position.1)),
                    ]),
                )];
                s.side_effects = effects;
                s.affected_components = vec![r.clone()];
                s.affected_nets = nets;
                s.estimated_improvement = c.improvement;
                out.push(s);
            }
        }
        out
    }

    pub fn generate_via_strategies(
        &self,
        pcb: &Pcb,
        failure: &FailureAnalysis,
    ) -> Vec<ResolutionStrategy> {
        let Some(net) = &failure.net else {
            return Vec::new();
        };
        self.find_via_positions(pcb, failure)
            .into_iter()
            .take(2)
            .map(|(pos, layer)| {
                let mut s = ResolutionStrategy::new(StrategyType::AddVia, Difficulty::Medium, 0.8);
                s.actions = vec![Action::new(
                    "add_via",
                    net.as_str(),
                    obj(&[
                        ("x", Json::Float(pos.0)),
                        ("y", Json::Float(pos.1)),
                        ("layer", Json::Str(layer.clone())),
                    ]),
                )];
                s.side_effects = vec![
                    SideEffect::new("Uses via budget (adds 1 via)", "info", false),
                    SideEffect::new(format!("Route continues on {layer}"), "info", false),
                ];
                s.affected_nets = vec![net.clone()];
                s.estimated_improvement = 0.7;
                s
            })
            .collect()
    }

    pub fn generate_reroute_strategies(
        &self,
        failure: &FailureAnalysis,
    ) -> Vec<ResolutionStrategy> {
        let nets: BTreeSet<&String> = failure
            .blocking_elements
            .iter()
            .filter(|e| e.kind == "trace")
            .filter_map(|e| e.net.as_ref())
            .filter(|n| !n.is_empty() && Some(*n) != failure.net.as_ref())
            .collect();
        nets.into_iter()
            .take(3)
            .map(|net| {
                let mut s =
                    ResolutionStrategy::new(StrategyType::RerouteNet, Difficulty::Medium, 0.6);
                s.actions = vec![Action::new(
                    "reroute",
                    net.as_str(),
                    obj(&[("avoid_area", failure.failure_area.to_dict())]),
                )];
                s.side_effects = vec![
                    SideEffect::new(
                        format!("Rerouting {net} may affect signal timing"),
                        "warning",
                        true,
                    ),
                    SideEffect::new("Trace length may change", "info", true),
                ];
                s.affected_nets = vec![net.clone()];
                s.estimated_improvement = 0.65;
                s
            })
            .collect()
    }

    pub fn generate_spread_strategies(
        &self,
        pcb: &Pcb,
        failure: &FailureAnalysis,
    ) -> Vec<ResolutionStrategy> {
        let comps: Vec<&String> = failure
            .blocking_elements
            .iter()
            .filter(|e| e.kind == "component" && e.movable)
            .filter_map(|e| e.reference.as_ref())
            .filter(|r| !r.is_empty())
            .collect();
        if comps.len() < 2 {
            return Vec::new();
        }
        let center = failure.failure_area.center();
        let mut actions = Vec::new();
        let mut nets: BTreeSet<String> = BTreeSet::new();
        for r in comps.iter().take(4) {
            let Some(fp) = find_footprint(pcb, r) else {
                continue;
            };
            let (mut dx, mut dy) = (fp.position.0 - center.0, fp.position.1 - center.1);
            let mut dist = (dx * dx + dy * dy).sqrt();
            if dist < 0.1 {
                (dx, dy, dist) = (1.0, 0.0, 1.0);
            }
            let spread = 2.0;
            actions.push(Action::new(
                "move",
                r.as_str(),
                obj(&[
                    ("x", Json::Float(fp.position.0 + dx / dist * spread)),
                    ("y", Json::Float(fp.position.1 + dy / dist * spread)),
                ]),
            ));
            nets.extend(self.get_component_nets(pcb, r));
        }
        if actions.is_empty() {
            return Vec::new();
        }
        let n = actions.len();
        let mut s = ResolutionStrategy::new(StrategyType::MoveMultiple, Difficulty::Hard, 0.7);
        s.actions = actions;
        s.side_effects = vec![
            SideEffect::new(
                format!("Moving {n} components will require rerouting"),
                "warning",
                true,
            ),
            SideEffect::new("Board area usage may increase", "info", false),
        ];
        s.affected_components = comps.iter().take(n).map(|r| (*r).clone()).collect();
        s.affected_nets = nets.into_iter().collect();
        s.estimated_improvement = 0.8;
        vec![s]
    }

    pub fn generate_layer_change_strategies(
        &self,
        pcb: &Pcb,
        failure: &FailureAnalysis,
    ) -> Vec<ResolutionStrategy> {
        let Some(net) = &failure.net else {
            return Vec::new();
        };
        get_copper_layers(pcb)
            .into_iter()
            .take(2)
            .map(|layer| {
                let mut s =
                    ResolutionStrategy::new(StrategyType::ChangeLayer, Difficulty::Easy, 0.75);
                s.actions = vec![Action::new(
                    "change_layer",
                    net.as_str(),
                    obj(&[("layer", Json::Str(layer.clone()))]),
                )];
                s.side_effects = vec![SideEffect::new(
                    format!("Route will use {layer} instead"),
                    "info",
                    false,
                )];
                s.affected_nets = vec![net.clone()];
                s.estimated_improvement = 0.7;
                s
            })
            .collect()
    }

    pub fn generate_manual_strategy(&self, failure: &FailureAnalysis) -> ResolutionStrategy {
        let description = match failure.root_cause {
            FailureCause::Keepout => "Routing crosses keepout zone; review keepout boundaries",
            FailureCause::LengthConstraint => "Cannot meet length constraints; review requirements",
            FailureCause::DifferentialPair => {
                "Cannot maintain differential pair spacing; review layout"
            }
            FailureCause::PinAccess => {
                "Cannot access pin; consider component rotation or repositioning"
            }
            _ => "Complex failure requiring manual review and intervention",
        };
        let net = failure.net.clone().filter(|n| !n.is_empty());
        let mut s =
            ResolutionStrategy::new(StrategyType::ManualIntervention, Difficulty::Expert, 0.5);
        s.actions = vec![Action::new(
            "manual",
            net.clone().unwrap_or_else(|| "board".into()),
            obj(&[
                (
                    "location",
                    obj(&[
                        ("x", Json::Float(failure.failure_location.0)),
                        ("y", Json::Float(failure.failure_location.1)),
                    ]),
                ),
                ("description", Json::Str(description.into())),
            ]),
        )];
        s.side_effects = vec![SideEffect::new(
            "Requires human expertise to resolve",
            "warning",
            false,
        )];
        s.affected_components = failure
            .blocking_elements
            .iter()
            .filter_map(|e| e.reference.clone().filter(|r| !r.is_empty()))
            .collect();
        s.affected_nets = net.into_iter().collect();
        s.estimated_improvement = 0.5;
        s
    }

    /// Candidate positions for moving `blocker`, diversity-sorted.
    pub fn find_move_candidates(
        &self,
        pcb: &Pcb,
        blocker: &BlockingElement,
        failure_area: &Rectangle,
        max_movement: Option<f64>,
    ) -> Vec<MoveCandidate> {
        let Some(r) = &blocker.reference else {
            return Vec::new();
        };
        let Some(fp) = find_footprint(pcb, r) else {
            return Vec::new();
        };
        let (cw, ch) = (blocker.bounds.width(), blocker.bounds.height());
        let mut offsets: Vec<(f64, f64)> = Vec::new();
        let mut meta: Vec<(usize, usize)> = Vec::new();
        match max_movement.filter(|m| *m > 0.0) {
            Some(m) => {
                let mut radii = Vec::new();
                let small = (m * 0.5).max(0.5);
                if small < m {
                    radii.push(small);
                }
                radii.push(m);
                const ANGLES: [f64; 8] = [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0];
                for (ri, radius) in radii.iter().enumerate() {
                    for (ai, ang) in ANGLES.iter().enumerate() {
                        let rad = ang.to_radians();
                        offsets.push((radius * rad.cos(), radius * rad.sin()));
                        meta.push((ri, ai));
                    }
                }
            }
            None => {
                let (w, h) = (failure_area.width() + cw, failure_area.height() + ch);
                offsets = vec![(w, 0.0), (-w, 0.0), (0.0, h), (0.0, -h), (w, h)];
                meta = (0..offsets.len()).map(|i| (0, i)).collect();
            }
        }
        let budget = max_movement.unwrap_or(f64::INFINITY);
        let mut cands: Vec<(MoveCandidate, usize, usize)> = offsets
            .iter()
            .zip(&meta)
            .map(|(&(dx, dy), &(ri, ai))| {
                let (nx, ny) = (fp.position.0 + dx, fp.position.1 + dy);
                let distance = dx.hypot(dy);
                let confidence = if budget.is_finite() {
                    if distance <= budget * 0.6 {
                        0.85
                    } else {
                        0.7
                    }
                } else if distance < 10.0 {
                    0.85
                } else {
                    0.7
                };
                let improvement = if failure_area.contains_point(nx, ny) {
                    0.5
                } else {
                    0.9
                };
                (
                    MoveCandidate {
                        position: (nx, ny),
                        confidence,
                        improvement,
                        distance,
                    },
                    ri,
                    ai,
                )
            })
            .collect();
        let bands = cands
            .iter()
            .map(|c| c.1)
            .collect::<BTreeSet<_>>()
            .len()
            .max(1);
        let spread = |a: usize| -> usize {
            match a {
                0 => 0,
                4 => 1,
                2 => 2,
                6 => 3,
                1 => 4,
                5 => 5,
                3 => 6,
                7 => 7,
                other => other,
            }
        };
        cands.sort_by(|x, y| {
            let key = |c: &(MoveCandidate, usize, usize)| (spread(c.2) * bands + c.1, c.2);
            (-x.0.improvement)
                .total_cmp(&-y.0.improvement)
                .then(key(x).cmp(&key(y)))
                .then(x.0.distance.total_cmp(&y.0.distance))
        });
        cands.into_iter().map(|c| c.0).collect()
    }

    /// Via positions beside the failure area on the first copper layer.
    pub fn find_via_positions(
        &self,
        pcb: &Pcb,
        failure: &FailureAnalysis,
    ) -> Vec<((f64, f64), String)> {
        let (fx, fy) = failure.failure_location;
        let a = &failure.failure_area;
        let cands = [
            (a.min_x - 1.0, fy),
            (a.max_x + 1.0, fy),
            (fx, a.min_y - 1.0),
            (fx, a.max_y + 1.0),
        ];
        let layers = get_copper_layers(pcb);
        let mut out = Vec::new();
        for pos in cands.iter().take(2) {
            for layer in layers.iter().take(1) {
                out.push((*pos, layer.clone()));
            }
        }
        out
    }

    pub fn analyze_move_side_effects(
        &self,
        pcb: &Pcb,
        r: &str,
        new_pos: (f64, f64),
    ) -> Vec<SideEffect> {
        let mut effects = Vec::new();
        if find_footprint(pcb, r).is_none() {
            return effects;
        }
        if is_bypass_cap(r) {
            if let Some(ic) = self.find_decoupled_ic(pcb, r) {
                if let Some(icfp) = find_footprint(pcb, &ic) {
                    let d = ((new_pos.0 - icfp.position.0).powi(2)
                        + (new_pos.1 - icfp.position.1).powi(2))
                    .sqrt();
                    if d > Self::BYPASS_CAP_DISTANCE_THRESHOLD {
                        effects.push(SideEffect::new(
                            format!("Increases distance from {ic}, may affect decoupling"),
                            "warning",
                            false,
                        ));
                    }
                }
            }
        }
        if let Some(cluster) = self.get_component_cluster(pcb, r) {
            if let Some(c) = compute_cluster_center(pcb, &cluster) {
                let d = ((new_pos.0 - c.0).powi(2) + (new_pos.1 - c.1).powi(2)).sqrt();
                if d > Self::CLUSTER_DISTANCE_THRESHOLD {
                    effects.push(SideEffect::new(
                        format!("Separates {r} from functional group"),
                        "warning",
                        true,
                    ));
                }
            }
        }
        effects.push(SideEffect::new(
            "May require rerouting connected traces",
            "info",
            true,
        ));
        effects
    }

    pub fn assess_move_difficulty(effects: &[SideEffect]) -> Difficulty {
        let risks = effects.iter().filter(|e| e.severity == "risk").count();
        let warnings = effects.iter().filter(|e| e.severity == "warning").count();
        if risks > 0 {
            Difficulty::Hard
        } else if warnings >= 2 {
            Difficulty::Medium
        } else if warnings >= 1 {
            Difficulty::Easy
        } else {
            Difficulty::Trivial
        }
    }

    /// Stable sort by (difficulty, -confidence, side-effect count).
    pub fn rank_strategies(mut v: Vec<ResolutionStrategy>) -> Vec<ResolutionStrategy> {
        v.sort_by(|a, b| {
            a.difficulty
                .cmp(&b.difficulty)
                .then((-a.confidence).total_cmp(&-b.confidence))
                .then(a.side_effects.len().cmp(&b.side_effects.len()))
        });
        v
    }

    /// Nets on a component's pads (sorted).
    pub fn get_component_nets(&self, pcb: &Pcb, r: &str) -> Vec<String> {
        find_footprint(pcb, r)
            .map(|fp| {
                fp.pads
                    .iter()
                    .filter(|p| !p.net_name.is_empty())
                    .map(|p| p.net_name.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Nearest `U*` footprint sharing a net with the capacitor.
    pub fn find_decoupled_ic(&self, pcb: &Pcb, cap: &str) -> Option<String> {
        let cfp = find_footprint(pcb, cap)?;
        let nets: BTreeSet<&str> = cfp
            .pads
            .iter()
            .filter(|p| !p.net_name.is_empty())
            .map(|p| p.net_name.as_str())
            .collect();
        let mut best: Option<String> = None;
        let mut best_d = f64::INFINITY;
        for fp in pcb.footprints() {
            if !fp.reference.to_uppercase().starts_with('U') {
                continue;
            }
            if fp.pads.iter().any(|p| nets.contains(p.net_name.as_str())) {
                let d = ((fp.position.0 - cfp.position.0).powi(2)
                    + (fp.position.1 - cfp.position.1).powi(2))
                .sqrt();
                if d < best_d {
                    best_d = d;
                    best = Some(fp.reference.clone());
                }
            }
        }
        best
    }

    /// Nearby footprints sharing a net with `r` (None unless >1 member).
    pub fn get_component_cluster(&self, pcb: &Pcb, r: &str) -> Option<Vec<String>> {
        let fp = find_footprint(pcb, r)?;
        let nets: BTreeSet<String> = self.get_component_nets(pcb, r).into_iter().collect();
        if nets.is_empty() {
            return None;
        }
        let mut cluster = vec![r.to_string()];
        for o in pcb.footprints() {
            if o.reference == r {
                continue;
            }
            let d = ((o.position.0 - fp.position.0).powi(2)
                + (o.position.1 - fp.position.1).powi(2))
            .sqrt();
            if d > Self::CLUSTER_DISTANCE_THRESHOLD {
                continue;
            }
            if o.pads
                .iter()
                .any(|p| !p.net_name.is_empty() && nets.contains(&p.net_name))
            {
                cluster.push(o.reference.clone());
            }
        }
        (cluster.len() > 1).then_some(cluster)
    }
}

pub fn find_footprint<'a>(pcb: &'a Pcb, r: &str) -> Option<&'a Footprint> {
    pcb.footprints().iter().find(|f| f.reference == r)
}

/// Copper layer names (`signal`/`power` containing `Cu`), else F/B.Cu.
pub fn get_copper_layers(pcb: &Pcb) -> Vec<String> {
    let v: Vec<String> = pcb
        .layers()
        .iter()
        .filter(|l| matches!(l.layer_type.as_str(), "signal" | "power") && l.name.contains("Cu"))
        .map(|l| l.name.clone())
        .collect();
    if v.is_empty() {
        vec!["F.Cu".into(), "B.Cu".into()]
    } else {
        v
    }
}

pub fn is_bypass_cap(r: &str) -> bool {
    r.to_uppercase().starts_with('C')
}

pub fn compute_cluster_center(pcb: &Pcb, cluster: &[String]) -> Option<(f64, f64)> {
    let pts: Vec<(f64, f64)> = cluster
        .iter()
        .filter_map(|r| find_footprint(pcb, r).map(|f| f.position))
        .collect();
    if pts.is_empty() {
        return None;
    }
    let n = pts.len() as f64;
    Some((
        pts.iter().map(|p| p.0).sum::<f64>() / n,
        pts.iter().map(|p| p.1).sum::<f64>() / n,
    ))
}
