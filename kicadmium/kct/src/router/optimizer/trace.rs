//! Port of `kicad_tools.router.optimizer.trace`: [`TraceOptimizer`], the
//! connectivity guard, and the grid-synced route transform.
//!
//! The router-coupled pieces (`PairwiseRouteGate`,
//! `apply_route_transform_grid_synced`) are expressed over the small
//! [`PairwiseChecker`] / [`SyncedRouter`] traits so they compile ahead of the
//! router core; `Autorouter`/`RoutingGrid` implement them when ported.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;

use super::algorithms::{
    self, compress_staircase, convert_corners_45, eliminate_zigzags, merge_collinear,
    pull_tight_pass,
};
use super::chain::sort_into_chains;
use super::collision::CollisionChecker;
use super::config::{OptimizationConfig, OptimizationStats};
use super::geometry;
use super::pcb::{self, DrcCounter, NetSegments};
use super::via_optimizer::{ViaOptimizationConfig, ViaOptimizationStats, ViaOptimizer};
use crate::router::layers::Layer;
use crate::router::primitives::{Route, Segment, Via};

/// Optimizer for PCB trace cleanup and simplification.
pub struct TraceOptimizer<'a> {
    pub config: OptimizationConfig,
    pub collision_checker: Option<&'a dyn CollisionChecker>,
    via_optimizer: ViaOptimizer<'a>,
}

impl<'a> TraceOptimizer<'a> {
    pub fn new(
        config: Option<OptimizationConfig>,
        collision_checker: Option<&'a dyn CollisionChecker>,
    ) -> Self {
        let config = config.unwrap_or_default();
        let via_config = ViaOptimizationConfig {
            enabled: config.minimize_vias,
            max_detour_factor: config.via_max_detour_factor,
            via_pair_threshold: config.via_pair_threshold,
            min_segment_length: config.min_segment_length,
            tolerance: config.tolerance,
        };
        TraceOptimizer {
            via_optimizer: ViaOptimizer::new(Some(via_config), collision_checker),
            config,
            collision_checker,
        }
    }

    fn path_is_clear(&self, seg: &Segment) -> bool {
        match self.collision_checker {
            None => true,
            Some(c) => c.path_is_clear(seg.x1, seg.y1, seg.x2, seg.y2, seg.layer, seg.width, seg.net),
        }
    }

    fn with_clear<T>(&self, f: impl FnOnce(algorithms::PathIsClear<'_>) -> T) -> T {
        let pred = |s: &Segment| self.path_is_clear(s);
        f(Some(&pred))
    }

    /// Optimize one net's segments: chain split (also at width/layer/net
    /// transitions), then merge, zigzag, staircase, 45-degree, PullTight.
    pub fn optimize_segments(&self, segments: &[Segment]) -> Vec<Segment> {
        if segments.is_empty() {
            return Vec::new();
        }
        let mut chains: Vec<Vec<Segment>> = Vec::new();
        for chain in sort_into_chains(segments, self.config.tolerance) {
            // itertools.groupby on (width, layer, net): consecutive runs.
            let mut run: Vec<Segment> = Vec::new();
            for seg in chain {
                if let Some(last) = run.last() {
                    if (last.width, last.layer, last.net) != (seg.width, seg.layer, seg.net) {
                        chains.push(std::mem::take(&mut run));
                    }
                }
                run.push(seg);
            }
            if !run.is_empty() {
                chains.push(run);
            }
        }
        let mut out = Vec::new();
        for chain in chains {
            let mut result = chain;
            if self.config.merge_collinear {
                result = self.merge_collinear(&result);
            }
            if self.config.eliminate_zigzags {
                result = self.eliminate_zigzags(&result);
            }
            if self.config.compress_staircase {
                result = self.compress_staircase(&result);
            }
            if self.config.convert_45_corners {
                result = self.convert_corners_45(&result);
            }
            if self.config.pull_tight {
                result = self.pull_tight(&result);
            }
            out.extend(result);
        }
        out
    }

    pub fn merge_collinear(&self, segments: &[Segment]) -> Vec<Segment> {
        self.with_clear(|c| merge_collinear(segments, &self.config, c))
    }

    pub fn eliminate_zigzags(&self, segments: &[Segment]) -> Vec<Segment> {
        self.with_clear(|c| eliminate_zigzags(segments, &self.config, c))
    }

    pub fn compress_staircase(&self, segments: &[Segment]) -> Vec<Segment> {
        self.with_clear(|c| compress_staircase(segments, &self.config, c))
    }

    pub fn convert_corners_45(&self, segments: &[Segment]) -> Vec<Segment> {
        self.with_clear(|c| convert_corners_45(segments, &self.config, c, None))
    }

    pub fn pull_tight(&self, segments: &[Segment]) -> Vec<Segment> {
        self.with_clear(|c| pull_tight_pass(segments, &self.config, c))
    }

    /// Optimize a route per layer, then minimize vias; revert whenever a
    /// pad-like endpoint loses connectivity (#2389, #2402).
    pub fn optimize_route(&self, route: &Route) -> Route {
        let tol = self.config.tolerance;
        let pre = collect_terminal_endpoints(&route.segments, tol);
        let mut by_layer: Vec<(Layer, Vec<Segment>)> = Vec::new();
        for seg in &route.segments {
            match by_layer.iter_mut().find(|(l, _)| *l == seg.layer) {
                Some((_, v)) => v.push(seg.clone()),
                None => by_layer.push((seg.layer, vec![seg.clone()])),
            }
        }
        let mut optimized: Vec<Segment> = Vec::new();
        for (_, segs) in &by_layer {
            optimized.extend(self.optimize_segments(segs));
        }
        if !endpoints_preserved(&pre, &optimized, &route.vias, tol) {
            optimized = route.segments.clone();
        }
        let mut out = Route::new(route.net, &route.net_name, optimized, route.vias.clone());
        if self.config.minimize_vias {
            out = self.via_optimizer.optimize_route(&out);
        }
        if !endpoints_preserved(&pre, &out.segments, &out.vias, tol) {
            out = Route::new(
                route.net,
                &route.net_name,
                route.segments.clone(),
                route.vias.clone(),
            );
        }
        out
    }

    /// Optimize traces in a board file (see [`pcb::optimize_pcb`]).
    pub fn optimize_pcb(
        &self,
        pcb_path: &Path,
        output_path: Option<&Path>,
        net_filter: Option<&str>,
        dry_run: bool,
        drc: Option<DrcCounter<'_>>,
    ) -> Result<OptimizationStats> {
        pcb::optimize_pcb(
            pcb_path,
            output_path,
            &|segs| self.optimize_segments(segs),
            &self.config,
            net_filter,
            dry_run,
            drc,
        )
    }

    // ------------------------------------------------------ test helpers

    pub fn is_connected(&self, s1: &Segment, s2: &Segment) -> bool {
        geometry::is_connected(s1, s2, self.config.tolerance)
    }
    pub fn segments_touch(&self, s1: &Segment, s2: &Segment) -> bool {
        geometry::segments_touch(s1, s2, self.config.tolerance)
    }
    pub fn sort_into_chains(&self, segments: &[Segment]) -> Vec<Vec<Segment>> {
        sort_into_chains(segments, self.config.tolerance)
    }
    pub fn same_direction(&self, s1: &Segment, s2: &Segment) -> bool {
        geometry::same_direction(s1, s2, self.config.tolerance)
    }
    pub fn is_zigzag(&self, s1: &Segment, s2: &Segment, s3: &Segment) -> bool {
        geometry::is_zigzag(s1, s2, s3, self.config.tolerance)
    }
    pub fn angle_between(&self, s1: &Segment, s2: &Segment) -> f64 {
        geometry::angle_between(s1, s2, self.config.tolerance)
    }
    pub fn is_90_degree_corner(&self, s1: &Segment, s2: &Segment) -> bool {
        geometry::is_90_degree_corner(s1, s2)
    }
    pub fn shorten_segment_end(&self, seg: &Segment, amount: f64) -> Option<Segment> {
        geometry::shorten_segment_end(seg, amount, self.config.min_segment_length)
    }
    pub fn shorten_segment_start(&self, seg: &Segment, amount: f64) -> Option<Segment> {
        geometry::shorten_segment_start(seg, amount, self.config.min_segment_length)
    }
    pub fn count_corners(&self, segments: &[Segment]) -> usize {
        geometry::count_corners(segments, self.config.tolerance)
    }
    pub fn total_length(&self, segments: &[Segment]) -> f64 {
        geometry::total_length(segments)
    }
    pub fn segment_direction(&self, seg: &Segment) -> f64 {
        geometry::segment_direction(seg, self.config.tolerance)
    }
    pub fn find_staircase_end(&self, segments: &[Segment], start_idx: usize) -> usize {
        algorithms::find_staircase_end(segments, start_idx, &self.config)
    }
    pub fn optimal_path(&self, start: (f64, f64), end: (f64, f64), t: &Segment) -> Vec<Segment> {
        algorithms::optimal_path(start, end, t, &self.config)
    }
    pub fn parse_net_names(&self, pcb_text: &str) -> HashMap<i64, String> {
        pcb::parse_net_names(pcb_text)
    }
    pub fn parse_segments(&self, pcb_text: &str) -> Result<NetSegments> {
        pcb::parse_segments(pcb_text)
    }
    pub fn replace_segments(
        &self,
        pcb_text: &str,
        original: &NetSegments,
        optimized: &NetSegments,
    ) -> Result<String> {
        pcb::replace_segments(pcb_text, original, optimized)
    }

    /// Via statistics: `(vias_before, vias_after, vias_removed, pct)`.
    pub fn get_via_stats(&self) -> ViaOptimizationStats {
        self.via_optimizer.get_stats()
    }

    pub fn reset_via_stats(&self) {
        self.via_optimizer.reset_stats();
    }
}

// --------------------------------------------- connectivity guard (#2389)

type VKey = (i64, i64, u8);

fn vertex_key(x: f64, y: f64, layer: Layer, tolerance: f64) -> VKey {
    let (qx, qy) = if tolerance <= 0.0 {
        ((x * 1e9).round_ties_even(), (y * 1e9).round_ties_even())
    } else {
        ((x / tolerance).round_ties_even(), (y / tolerance).round_ties_even())
    };
    (qx as i64, qy as i64, layer.value())
}

/// Per-layer degree-1 vertices ("pad-like" endpoints) of a segment list.
pub fn collect_terminal_endpoints(segments: &[Segment], tolerance: f64) -> HashSet<VKey> {
    let mut counts: HashMap<VKey, usize> = HashMap::new();
    for seg in segments {
        for (x, y) in [seg.start(), seg.end()] {
            *counts.entry(vertex_key(x, y, seg.layer, tolerance)).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, c)| *c == 1)
        .map(|(k, _)| k)
        .collect()
}

/// Every pre-optimization endpoint is present and in one component.
pub fn endpoints_preserved(
    pre: &HashSet<VKey>,
    segments: &[Segment],
    vias: &[Via],
    tolerance: f64,
) -> bool {
    if pre.is_empty() {
        return true;
    }
    let mut adjacency: HashMap<VKey, HashSet<VKey>> = HashMap::new();
    let mut add = |a: VKey, b: VKey| {
        adjacency.entry(a).or_default().insert(b);
        adjacency.entry(b).or_default().insert(a);
    };
    for seg in segments {
        add(
            vertex_key(seg.x1, seg.y1, seg.layer, tolerance),
            vertex_key(seg.x2, seg.y2, seg.layer, tolerance),
        );
    }
    for via in vias {
        add(
            vertex_key(via.x, via.y, via.layers.0, tolerance),
            vertex_key(via.x, via.y, via.layers.1, tolerance),
        );
    }
    if !pre.iter().all(|k| adjacency.contains_key(k)) {
        return false;
    }
    let start = *pre.iter().next().expect("non-empty");
    let mut visited = HashSet::from([start]);
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
    pre.iter().all(|k| visited.contains(k))
}

// ------------------------------------------------ router-coupled helpers

/// A pairwise (HV creepage) clearance shortfall (`PairwiseViolation`).
#[derive(Debug, Clone, PartialEq)]
pub struct PairwiseViolation {
    pub net_a: String,
    pub net_b: String,
    pub actual_mm: f64,
    pub required_mm: f64,
    pub x: f64,
    pub y: f64,
}

/// `violation_pair_key`: the unordered net pair.
pub fn violation_pair_key(v: &PairwiseViolation) -> (String, String) {
    if v.net_a <= v.net_b {
        (v.net_a.clone(), v.net_b.clone())
    } else {
        (v.net_b.clone(), v.net_a.clone())
    }
}

/// The `PairwisePathChecker` surface the gate uses.
pub trait PairwiseChecker {
    fn foreign_routes(&self) -> Vec<Route>;
    /// `route_pairwise_violation(route, net, foreign, ...)`.
    fn violation(&self, route: &Route, net: i64, foreign: &[Route]) -> Option<PairwiseViolation>;
}

/// Route-level pairwise-clearance accept predicate (issue #4766).
pub struct PairwiseRouteGate<C: PairwiseChecker> {
    pub checker: C,
    pub vetoes: usize,
    pub worst: Option<PairwiseViolation>,
}

impl<C: PairwiseChecker> PairwiseRouteGate<C> {
    pub fn new(checker: C) -> Self {
        PairwiseRouteGate {
            checker,
            vetoes: 0,
            worst: None,
        }
    }

    /// True to accept `optimized`; false to keep `original`.
    pub fn accept(&mut self, original: &Route, optimized: &Route) -> bool {
        let foreign = self.checker.foreign_routes();
        let Some(violation) = self.checker.violation(optimized, original.net, &foreign) else {
            return true;
        };
        if let Some(prior) = self.checker.violation(original, original.net, &foreign) {
            if violation_pair_key(&prior) == violation_pair_key(&violation) {
                return true;
            }
        }
        self.vetoes += 1;
        let shortfall = violation.required_mm - violation.actual_mm;
        let worse = self
            .worst
            .as_ref()
            .is_none_or(|w| shortfall > w.required_mm - w.actual_mm);
        if worse {
            self.worst = Some(violation);
        }
        false
    }

    pub fn advisory(&self) -> Option<String> {
        let w = self.worst.as_ref()?;
        if self.vetoes == 0 {
            return None;
        }
        Some(format!(
            "  HV pairwise gate: {} route(s) kept pre-optimization geometry (worst: {} vs {} \
             {:.3}mm < {:.3}mm required at ({:.3}, {:.3}))",
            self.vetoes, w.net_a, w.net_b, w.actual_mm, w.required_mm, w.x, w.y
        ))
    }

    /// Print the advisory to stderr when any route was vetoed.
    pub fn report(&self) {
        if let Some(line) = self.advisory() {
            eprintln!("{line}");
        }
    }
}

/// The `Autorouter` + `RoutingGrid` surface used by the synced transform.
pub trait SyncedRouter {
    fn routes(&self) -> Vec<Route>;
    fn set_routes(&mut self, routes: Vec<Route>);
    fn unmark_route(&mut self, route: &Route);
    fn mark_route(&mut self, route: &Route);
    fn mark_route_on_cpp_cells(&mut self, _route: &Route) {}
    fn resync_route_occupancy(&mut self, pairs: &[(Route, Route)]);
}

/// Replace every route with `transform(route)`, keeping the grid in sync
/// (#3507 incremental unmark/mark, #3511 batched resync).
pub fn apply_route_transform_grid_synced<R: SyncedRouter>(
    router: &mut R,
    transform: &dyn Fn(&Route) -> Route,
    skip_nets: Option<&HashSet<i64>>,
    mut accept: Option<&mut dyn FnMut(&Route, &Route) -> bool>,
) -> Vec<Route> {
    let mut out = Vec::new();
    let mut mutated = Vec::new();
    for route in router.routes() {
        if skip_nets.is_some_and(|s| s.contains(&route.net)) {
            out.push(route);
            continue;
        }
        let mut optimized = transform(&route);
        if let Some(acc) = accept.as_mut() {
            if optimized != route && !acc(&route, &optimized) {
                optimized = route.clone();
            }
        }
        if optimized.segments == route.segments && optimized.vias == route.vias {
            out.push(route);
            continue;
        }
        router.unmark_route(&route);
        router.mark_route(&optimized);
        router.mark_route_on_cpp_cells(&optimized);
        out.push(optimized.clone());
        mutated.push((route, optimized));
    }
    router.set_routes(out.clone());
    if !mutated.is_empty() {
        router.resync_route_occupancy(&mutated);
    }
    out
}

/// `optimize_routes_grid_synced`: [`apply_route_transform_grid_synced`]
/// with `optimizer.optimize_route` and an optional pairwise gate.
pub fn optimize_routes_grid_synced<R: SyncedRouter, C: PairwiseChecker>(
    router: &mut R,
    optimizer: &TraceOptimizer<'_>,
    skip_nets: Option<&HashSet<i64>>,
    gate: Option<&mut PairwiseRouteGate<C>>,
) -> Vec<Route> {
    let transform = |r: &Route| optimizer.optimize_route(r);
    match gate {
        Some(g) => {
            let routes = {
                let mut acc = |o: &Route, n: &Route| g.accept(o, n);
                apply_route_transform_grid_synced(router, &transform, skip_nets, Some(&mut acc))
            };
            g.report();
            routes
        }
        None => apply_route_transform_grid_synced(router, &transform, skip_nets, None),
    }
}
