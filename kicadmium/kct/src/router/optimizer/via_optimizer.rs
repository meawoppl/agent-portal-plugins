//! Port of `kicad_tools.router.optimizer.via_optimizer`: remove via pairs
//! and single vias when a same-layer path is clear.
//!
//! Upstream mixes identity (`is`) and equality (`==`, `list.remove`) checks
//! on segment objects; segments here carry a private identity tag so both
//! behaviours are reproduced.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;

use super::collision::CollisionChecker;
use crate::router::layers::Layer;
use crate::router::primitives::{Route, Segment, Via};

type Key = (i64, i64, u8);

fn qkey(x: f64, y: f64, layer: Layer, tolerance: f64) -> Key {
    let (qx, qy) = if tolerance <= 0.0 {
        ((x * 1e9).round_ties_even(), (y * 1e9).round_ties_even())
    } else {
        ((x / tolerance).round_ties_even(), (y / tolerance).round_ties_even())
    };
    (qx as i64, qy as i64, layer.value())
}

/// A segment with an identity tag (Python object identity).
#[derive(Debug, Clone)]
struct Tagged {
    id: usize,
    seg: Segment,
}

#[derive(Debug, Clone)]
struct TaggedVia {
    id: usize,
    via: Via,
}

fn terminals_connected(
    terminals: &HashSet<Key>,
    segments: &[&Segment],
    vias: &[&Via],
    tolerance: f64,
) -> bool {
    let mut adjacency: HashMap<Key, HashSet<Key>> = HashMap::new();
    let mut add = |a: Key, b: Key| {
        adjacency.entry(a).or_default().insert(b);
        adjacency.entry(b).or_default().insert(a);
    };
    for seg in segments {
        add(
            qkey(seg.x1, seg.y1, seg.layer, tolerance),
            qkey(seg.x2, seg.y2, seg.layer, tolerance),
        );
    }
    for via in vias {
        add(
            qkey(via.x, via.y, via.layers.0, tolerance),
            qkey(via.x, via.y, via.layers.1, tolerance),
        );
    }
    if !terminals.iter().all(|k| adjacency.contains_key(k)) {
        return false;
    }
    let Some(&start) = terminals.iter().next() else {
        return true;
    };
    let mut visited: HashSet<Key> = HashSet::from([start]);
    let mut queue = vec![start];
    while let Some(node) = queue.pop() {
        if let Some(nbrs) = adjacency.get(&node) {
            for &n in nbrs {
                if visited.insert(n) {
                    queue.push(n);
                }
            }
        }
    }
    terminals.iter().all(|k| visited.contains(k))
}

/// Degree-1 per-layer vertices of a segment list.
pub fn terminal_endpoints(segments: &[&Segment], tolerance: f64) -> HashSet<Key> {
    let mut counts: HashMap<Key, usize> = HashMap::new();
    for seg in segments {
        for (x, y) in [seg.start(), seg.end()] {
            *counts.entry(qkey(x, y, seg.layer, tolerance)).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, c)| *c == 1)
        .map(|(k, _)| k)
        .collect()
}

fn via_removal_preserves_connectivity(
    segments: &[Tagged],
    vias: &[TaggedVia],
    candidate_via: usize,
    seg_before: usize,
    seg_after: usize,
    alt_path: &[Segment],
    tolerance: f64,
) -> bool {
    let all: Vec<&Segment> = segments.iter().map(|t| &t.seg).collect();
    let terminals = terminal_endpoints(&all, tolerance);
    if terminals.len() < 2 {
        return true;
    }
    let mut hyp: Vec<&Segment> = segments
        .iter()
        .filter(|t| t.id != seg_before && t.id != seg_after)
        .map(|t| &t.seg)
        .collect();
    hyp.extend(alt_path.iter());
    let hyp_vias: Vec<&Via> = vias
        .iter()
        .filter(|v| v.id != candidate_via)
        .map(|v| &v.via)
        .collect();
    terminals_connected(&terminals, &hyp, &hyp_vias, tolerance)
}

/// Segments touching a via (by position and layer).
#[derive(Debug, Clone)]
struct ViaContext {
    via: TaggedVia,
    segments_before: Vec<Tagged>,
    segments_after: Vec<Tagged>,
}

impl ViaContext {
    fn from_layer(&self) -> Layer {
        self.via.via.layers.0
    }
    fn to_layer(&self) -> Layer {
        self.via.via.layers.1
    }
}

/// Configuration for via optimization.
#[derive(Debug, Clone, PartialEq)]
pub struct ViaOptimizationConfig {
    pub enabled: bool,
    pub max_detour_factor: f64,
    pub via_pair_threshold: f64,
    pub min_segment_length: f64,
    pub tolerance: f64,
}

impl Default for ViaOptimizationConfig {
    fn default() -> Self {
        ViaOptimizationConfig {
            enabled: true,
            max_detour_factor: 1.5,
            via_pair_threshold: 2.0,
            min_segment_length: 0.05,
            tolerance: 1e-4,
        }
    }
}

/// Statistics from via optimization.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViaOptimizationStats {
    pub vias_before: i64,
    pub vias_after: i64,
    pub vias_removed_single: i64,
    pub vias_removed_pairs: i64,
}

impl ViaOptimizationStats {
    pub fn vias_removed(&self) -> i64 {
        self.vias_removed_single + self.vias_removed_pairs
    }

    pub fn via_reduction_percent(&self) -> f64 {
        if self.vias_before == 0 {
            return 0.0;
        }
        self.vias_removed() as f64 / self.vias_before as f64 * 100.0
    }
}

/// A layer transition without a via.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerConnectivityError {
    pub point: (f64, f64),
    pub from_layer: Layer,
    pub to_layer: Layer,
}

fn layer_enum_name(layer: Layer) -> &'static str {
    match layer {
        Layer::FCu => "F_CU",
        Layer::In1Cu => "IN1_CU",
        Layer::In2Cu => "IN2_CU",
        Layer::In3Cu => "IN3_CU",
        Layer::In4Cu => "IN4_CU",
        Layer::BCu => "B_CU",
    }
}

impl fmt::Display for LayerConnectivityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Layer transition at ({:.4}, {:.4}) from {} to {} has no via",
            self.point.0,
            self.point.1,
            layer_enum_name(self.from_layer),
            layer_enum_name(self.to_layer)
        )
    }
}

/// Reduces via count in routed traces.
pub struct ViaOptimizer<'a> {
    pub config: ViaOptimizationConfig,
    pub collision_checker: Option<&'a dyn CollisionChecker>,
    stats: RefCell<ViaOptimizationStats>,
    next_id: RefCell<usize>,
}

impl<'a> ViaOptimizer<'a> {
    pub fn new(
        config: Option<ViaOptimizationConfig>,
        collision_checker: Option<&'a dyn CollisionChecker>,
    ) -> Self {
        ViaOptimizer {
            config: config.unwrap_or_default(),
            collision_checker,
            stats: RefCell::new(ViaOptimizationStats::default()),
            next_id: RefCell::new(0),
        }
    }

    pub fn get_stats(&self) -> ViaOptimizationStats {
        self.stats.borrow().clone()
    }

    pub fn reset_stats(&self) {
        *self.stats.borrow_mut() = ViaOptimizationStats::default();
    }

    fn fresh_id(&self) -> usize {
        let mut n = self.next_id.borrow_mut();
        *n += 1;
        *n
    }

    /// Remove via pairs, then single vias; revert if a transition loses
    /// its via.
    pub fn optimize_route(&self, route: &Route) -> Route {
        if !self.config.enabled || route.vias.is_empty() {
            return route.clone();
        }
        self.stats.borrow_mut().vias_before += route.vias.len() as i64;
        let mut segments: Vec<Tagged> = route
            .segments
            .iter()
            .map(|s| Tagged {
                id: self.fresh_id(),
                seg: s.clone(),
            })
            .collect();
        let mut vias: Vec<TaggedVia> = route
            .vias
            .iter()
            .map(|v| TaggedVia {
                id: self.fresh_id(),
                via: v.clone(),
            })
            .collect();

        let mut contexts = self.build_via_contexts(&segments, &vias);
        let pairs_removed = self.eliminate_via_pairs(&contexts, &mut segments, &mut vias, route);
        self.stats.borrow_mut().vias_removed_pairs += pairs_removed * 2;
        if pairs_removed > 0 {
            contexts = self.build_via_contexts(&segments, &vias);
        }
        let singles_removed = self.remove_single_vias(&contexts, &mut segments, &mut vias, route);
        self.stats.borrow_mut().vias_removed_single += singles_removed;

        let optimized = Route::new(
            route.net,
            &route.net_name,
            segments.into_iter().map(|t| t.seg).collect(),
            vias.into_iter().map(|t| t.via).collect(),
        );
        if !self.validate_layer_connectivity(&optimized).is_empty() {
            let mut st = self.stats.borrow_mut();
            st.vias_removed_pairs -= pairs_removed * 2;
            st.vias_removed_single -= singles_removed;
            st.vias_after += route.vias.len() as i64;
            return route.clone();
        }
        self.stats.borrow_mut().vias_after += optimized.vias.len() as i64;
        optimized
    }

    fn build_via_contexts(&self, segments: &[Tagged], vias: &[TaggedVia]) -> Vec<ViaContext> {
        let tol = self.config.tolerance;
        vias.iter()
            .map(|tv| {
                let via = &tv.via;
                let before = segments
                    .iter()
                    .filter(|t| {
                        (t.seg.x2 - via.x).abs() < tol
                            && (t.seg.y2 - via.y).abs() < tol
                            && t.seg.layer == via.layers.0
                    })
                    .cloned()
                    .collect();
                let after = segments
                    .iter()
                    .filter(|t| {
                        (t.seg.x1 - via.x).abs() < tol
                            && (t.seg.y1 - via.y).abs() < tol
                            && t.seg.layer == via.layers.1
                    })
                    .cloned()
                    .collect();
                ViaContext {
                    via: tv.clone(),
                    segments_before: before,
                    segments_after: after,
                }
            })
            .collect()
    }

    fn eliminate_via_pairs(
        &self,
        contexts: &[ViaContext],
        segments: &mut Vec<Tagged>,
        vias: &mut Vec<TaggedVia>,
        route: &Route,
    ) -> i64 {
        if contexts.len() < 2 {
            return 0;
        }
        let mut removed = 0;
        let mut marked: HashSet<usize> = HashSet::new();
        for i in 0..contexts.len() - 1 {
            if marked.contains(&i) {
                continue;
            }
            let (c1, c2) = (&contexts[i], &contexts[i + 1]);
            if !(c1.from_layer() == c2.to_layer() && c1.to_layer() == c2.from_layer()) {
                continue;
            }
            let (v1, v2) = (&c1.via.via, &c2.via.via);
            let dist = ((v2.x - v1.x).powi(2) + (v2.y - v1.y).powi(2)).sqrt();
            if dist > self.config.via_pair_threshold {
                continue;
            }
            let width = route.segments.first().map_or(0.2, |s| s.width);
            if let Some(alt) = self.find_same_layer_path(
                v1.x,
                v1.y,
                v2.x,
                v2.y,
                c1.from_layer(),
                width,
                route.net,
                dist * self.config.max_detour_factor,
            ) {
                self.apply_via_pair_removal(c1, c2, alt, segments, vias, route);
                marked.insert(i);
                marked.insert(i + 1);
                removed += 1;
            }
        }
        removed
    }

    fn remove_single_vias(
        &self,
        contexts: &[ViaContext],
        segments: &mut Vec<Tagged>,
        vias: &mut Vec<TaggedVia>,
        route: &Route,
    ) -> i64 {
        let mut removed = 0;
        for ctx in contexts.iter().rev() {
            if !vias.iter().any(|v| v.via == ctx.via.via) {
                continue;
            }
            let (Some(before), Some(after)) =
                (ctx.segments_before.first(), ctx.segments_after.first())
            else {
                continue;
            };
            let (sx, sy) = (before.seg.x1, before.seg.y1);
            let (ex, ey) = (after.seg.x2, after.seg.y2);
            let direct = ((ex - sx).powi(2) + (ey - sy).powi(2)).sqrt();
            let max_detour = direct * self.config.max_detour_factor;
            for (layer, width) in [
                (ctx.from_layer(), before.seg.width),
                (ctx.to_layer(), after.seg.width),
            ] {
                let Some(alt) =
                    self.find_same_layer_path(sx, sy, ex, ey, layer, width, route.net, max_detour)
                else {
                    continue;
                };
                if via_removal_preserves_connectivity(
                    segments,
                    vias,
                    ctx.via.id,
                    before.id,
                    after.id,
                    &alt,
                    self.config.tolerance,
                ) {
                    self.apply_single_via_removal(ctx, before, after, alt, segments, vias, route);
                    removed += 1;
                    break;
                }
            }
        }
        removed
    }

    #[allow(clippy::too_many_arguments)]
    fn find_same_layer_path(
        &self,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        layer: Layer,
        width: f64,
        net: i64,
        max_detour: f64,
    ) -> Option<Vec<Segment>> {
        let direct = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
        if direct > max_detour {
            return None;
        }
        if let Some(checker) = self.collision_checker {
            if !checker.path_is_clear(x1, y1, x2, y2, layer, width, net) {
                return None;
            }
        }
        Some(vec![Segment::new(x1, y1, x2, y2, width, layer, net, "")])
    }

    fn remove_first_equal_via(vias: &mut Vec<TaggedVia>, via: &Via) {
        if let Some(i) = vias.iter().position(|v| &v.via == via) {
            vias.remove(i);
        }
    }

    fn remove_first_equal_seg(segments: &mut Vec<Tagged>, seg: &Segment) {
        if let Some(i) = segments.iter().position(|t| &t.seg == seg) {
            segments.remove(i);
        }
    }

    fn append_alt(&self, alt: Vec<Segment>, segments: &mut Vec<Tagged>, route: &Route) {
        for mut seg in alt {
            seg.net = route.net;
            seg.net_name = route.net_name.clone();
            segments.push(Tagged {
                id: self.fresh_id(),
                seg,
            });
        }
    }

    fn apply_via_pair_removal(
        &self,
        c1: &ViaContext,
        c2: &ViaContext,
        alt: Vec<Segment>,
        segments: &mut Vec<Tagged>,
        vias: &mut Vec<TaggedVia>,
        route: &Route,
    ) {
        let tol = self.config.tolerance;
        Self::remove_first_equal_via(vias, &c1.via.via);
        Self::remove_first_equal_via(vias, &c2.via.via);
        let (v1, v2) = (&c1.via.via, &c2.via.via);
        let to_remove: Vec<Segment> = segments
            .iter()
            .filter(|t| {
                let s = &t.seg;
                s.layer == c1.to_layer()
                    && (((s.x1 - v1.x).abs() < tol && (s.y1 - v1.y).abs() < tol)
                        || ((s.x2 - v2.x).abs() < tol && (s.y2 - v2.y).abs() < tol))
            })
            .map(|t| t.seg.clone())
            .collect();
        for seg in &to_remove {
            Self::remove_first_equal_seg(segments, seg);
        }
        self.append_alt(alt, segments, route);
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_single_via_removal(
        &self,
        ctx: &ViaContext,
        before: &Tagged,
        after: &Tagged,
        alt: Vec<Segment>,
        segments: &mut Vec<Tagged>,
        vias: &mut Vec<TaggedVia>,
        route: &Route,
    ) {
        Self::remove_first_equal_via(vias, &ctx.via.via);
        Self::remove_first_equal_seg(segments, &before.seg);
        Self::remove_first_equal_seg(segments, &after.seg);
        self.append_alt(alt, segments, route);
    }

    /// Layer transitions (same point, different layers) lacking a via.
    pub fn validate_layer_connectivity(&self, route: &Route) -> Vec<LayerConnectivityError> {
        let tol = self.config.tolerance;
        // `+ 0.0` folds -0.0 into 0.0 (Python float keys compare equal).
        let snap = |v: f64| (v / tol).round_ties_even() * tol + 0.0;
        let via_positions: HashSet<(u64, u64)> = route
            .vias
            .iter()
            .map(|v| (snap(v.x).to_bits(), snap(v.y).to_bits()))
            .collect();
        // Insertion-ordered segment end keys.
        let mut end_keys: Vec<(f64, f64, Layer)> = Vec::new();
        let mut seen_keys: HashSet<(u64, u64, Layer)> = HashSet::new();
        for seg in &route.segments {
            let k = (snap(seg.x2), snap(seg.y2), seg.layer);
            if seen_keys.insert((k.0.to_bits(), k.1.to_bits(), k.2)) {
                end_keys.push(k);
            }
        }
        let mut errors = Vec::new();
        for (x, y, layer1) in end_keys {
            for seg in &route.segments {
                let (sx, sy) = (snap(seg.x1), snap(seg.y1));
                if sx == x
                    && sy == y
                    && layer1 != seg.layer
                    && !via_positions.contains(&(x.to_bits(), y.to_bits()))
                {
                    errors.push(LayerConnectivityError {
                        point: (x, y),
                        from_layer: layer1,
                        to_layer: seg.layer,
                    });
                }
            }
        }
        let mut seen: HashSet<(u64, u64)> = HashSet::new();
        errors
            .into_iter()
            .filter(|e| seen.insert((e.point.0.to_bits(), e.point.1.to_bits())))
            .collect()
    }
}

/// Convenience: optimize one route's vias and return the stats.
pub fn optimize_route_vias(
    route: &Route,
    collision_checker: Option<&dyn CollisionChecker>,
    config: Option<ViaOptimizationConfig>,
) -> (Route, ViaOptimizationStats) {
    let optimizer = ViaOptimizer::new(config, collision_checker);
    let out = optimizer.optimize_route(route);
    (out, optimizer.get_stats())
}
