//! Port of `kicad_tools.router.optimizer.consolidate`: topology-preserving
//! collinear consolidation (issue #4732). Only degree-2, non-protected,
//! collinear, anti-parallel vertices are smoothed away, so copper and
//! connectivity are unchanged.

use super::geometry::SegmentExt;
use std::collections::{HashMap, HashSet};

use super::trace::{apply_route_transform_grid_synced, SyncedRouter};
use crate::router::primitives::{Route, Segment};

pub const COLLINEAR_CROSS_EPSILON: f64 = 1e-6;
pub const WIDTH_EPSILON_MM: f64 = 1e-9;
pub const DEFAULT_TOLERANCE_MM: f64 = 1e-4;

/// What one consolidation pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConsolidationStats {
    pub segments_before: usize,
    pub segments_after: usize,
    pub runs_merged: usize,
    pub runs_vetoed: usize,
    pub routes_changed: usize,
}

impl ConsolidationStats {
    pub fn segments_removed(&self) -> i64 {
        self.segments_before as i64 - self.segments_after as i64
    }

    /// One-line human summary for the CLI.
    pub fn summary(&self) -> String {
        let pct = if self.segments_before > 0 {
            self.segments_removed() as f64 / self.segments_before as f64 * 100.0
        } else {
            0.0
        };
        let mut text = format!(
            "{} -> {} segments ({} removed, {pct:.1}%), {} collinear run(s) merged across {} route(s)",
            self.segments_before,
            self.segments_after,
            self.segments_removed(),
            self.runs_merged,
            self.routes_changed
        );
        if self.runs_vetoed > 0 {
            text.push_str(&format!(", {} vetoed", self.runs_vetoed));
        }
        text
    }
}

type QKey = (i64, i64);

fn qkey(x: f64, y: f64, tolerance: f64) -> QKey {
    if tolerance <= 0.0 {
        return (
            (x * 1e9).round_ties_even() as i64,
            (y * 1e9).round_ties_even() as i64,
        );
    }
    (
        (x / tolerance).round_ties_even() as i64,
        (y / tolerance).round_ties_even() as i64,
    )
}

fn length(seg: &Segment) -> f64 {
    (seg.x2 - seg.x1).hypot(seg.y2 - seg.y1)
}

fn unit_away(seg: &Segment, at_start: bool) -> Option<(f64, f64)> {
    let (dx, dy) = if at_start {
        (seg.x2 - seg.x1, seg.y2 - seg.y1)
    } else {
        (seg.x1 - seg.x2, seg.y1 - seg.y2)
    };
    let norm = dx.hypot(dy);
    if norm <= 0.0 {
        return None;
    }
    Some((dx / norm, dy / norm))
}

fn anti_parallel(u: (f64, f64), v: (f64, f64)) -> bool {
    let cross = u.0 * v.1 - u.1 * v.0;
    let dot = u.0 * v.0 + u.1 * v.1;
    dot < 0.0 && cross.abs() <= COLLINEAR_CROSS_EPSILON
}

struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn find(&mut self, mut i: usize) -> usize {
        let mut root = i;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        while self.parent[i] != root {
            let next = self.parent[i];
            self.parent[i] = root;
            i = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra.max(rb)] = ra.min(rb);
        }
    }
}

type Pred<'a> = Option<&'a dyn Fn(&Segment) -> bool>;

fn consolidate_indexed(
    segments: &[Segment],
    protected_points: &[(f64, f64)],
    tolerance: f64,
    path_is_clear: Pred<'_>,
    same_owner: Option<&dyn Fn(usize, usize) -> bool>,
) -> (Vec<(usize, Segment)>, ConsolidationStats) {
    let n = segments.len();
    if n < 2 {
        return (
            segments.iter().cloned().enumerate().collect(),
            ConsolidationStats {
                segments_before: n,
                segments_after: n,
                ..Default::default()
            },
        );
    }
    let protected: HashSet<QKey> = protected_points
        .iter()
        .map(|(x, y)| qkey(*x, *y, tolerance))
        .collect();
    let mut order: Vec<QKey> = Vec::new();
    let mut incident: HashMap<QKey, Vec<(usize, bool)>> = HashMap::new();
    let mut usable = Vec::with_capacity(n);
    for (i, seg) in segments.iter().enumerate() {
        usable.push(length(seg) > 0.0);
        for (x, y, is_start) in [(seg.x1, seg.y1, true), (seg.x2, seg.y2, false)] {
            let key = qkey(x, y, tolerance);
            incident
                .entry(key)
                .or_insert_with(|| {
                    order.push(key);
                    Vec::new()
                })
                .push((i, is_start));
        }
    }
    let mut dsu = DisjointSet {
        parent: (0..n).collect(),
    };
    let mut smoothed: HashSet<QKey> = HashSet::new();
    for key in &order {
        let hits = &incident[key];
        if hits.len() != 2 || protected.contains(key) {
            continue;
        }
        let ((ia, a_start), (ib, b_start)) = (hits[0], hits[1]);
        if ia == ib || !usable[ia] || !usable[ib] {
            continue;
        }
        let (sa, sb) = (&segments[ia], &segments[ib]);
        if sa.layer != sb.layer || sa.net != sb.net {
            continue;
        }
        if (sa.width - sb.width).abs() > WIDTH_EPSILON_MM {
            continue;
        }
        if same_owner.is_some_and(|f| !f(ia, ib)) {
            continue;
        }
        match (unit_away(sa, a_start), unit_away(sb, b_start)) {
            (Some(ua), Some(ub)) if anti_parallel(ua, ub) => {}
            _ => continue,
        }
        dsu.union(ia, ib);
        smoothed.insert(*key);
    }

    let mut group_order: Vec<usize> = Vec::new();
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &ok) in usable.iter().enumerate() {
        if ok {
            let root = dsu.find(i);
            groups
                .entry(root)
                .or_insert_with(|| {
                    group_order.push(root);
                    Vec::new()
                })
                .push(i);
        }
    }

    let mut replacement: HashMap<usize, Segment> = HashMap::new();
    let mut dropped: HashSet<usize> = HashSet::new();
    let (mut runs_merged, mut runs_vetoed) = (0, 0);
    for root in group_order {
        let members = &groups[&root];
        if members.len() < 2 {
            continue;
        }
        let mut ends: Vec<(QKey, f64, f64)> = Vec::new();
        for &i in members {
            let seg = &segments[i];
            for (x, y) in [(seg.x1, seg.y1), (seg.x2, seg.y2)] {
                let key = qkey(x, y, tolerance);
                if !smoothed.contains(&key) {
                    ends.push((key, x, y));
                }
            }
        }
        if ends.len() != 2 || ends[0].0 == ends[1].0 {
            continue;
        }
        let head = *members.iter().min().expect("non-empty");
        let template = &segments[head];
        let (mut first, mut second) = (ends[0], ends[1]);
        if qkey(template.x1, template.y1, tolerance) == second.0 {
            std::mem::swap(&mut first, &mut second);
        }
        let merged = template.with_points(first.1, first.2, second.1, second.2);
        let merged_length = length(&merged);
        if merged_length <= 0.0 {
            continue;
        }
        let parts: f64 = members.iter().map(|&m| length(&segments[m])).sum();
        if (merged_length - parts).abs() > tolerance.max(1e-9) {
            runs_vetoed += 1;
            continue;
        }
        if path_is_clear.is_some_and(|f| !f(&merged)) {
            runs_vetoed += 1;
            continue;
        }
        replacement.insert(head, merged);
        dropped.extend(members.iter().copied().filter(|&m| m != head));
        runs_merged += 1;
    }

    let result: Vec<(usize, Segment)> = segments
        .iter()
        .enumerate()
        .filter(|(i, _)| !dropped.contains(i))
        .map(|(i, s)| (i, replacement.get(&i).cloned().unwrap_or_else(|| s.clone())))
        .collect();
    let stats = ConsolidationStats {
        segments_before: n,
        segments_after: result.len(),
        runs_merged,
        runs_vetoed,
        routes_changed: 0,
    };
    (result, stats)
}

/// Collapse maximal collinear runs into single segments.
pub fn consolidate_segments(
    segments: &[Segment],
    protected_points: &[(f64, f64)],
    tolerance: f64,
    path_is_clear: Pred<'_>,
) -> (Vec<Segment>, ConsolidationStats) {
    let (indexed, stats) =
        consolidate_indexed(segments, protected_points, tolerance, path_is_clear, None);
    (indexed.into_iter().map(|(_, s)| s).collect(), stats)
}

/// Consolidate one net's routes as a single connectivity graph.
pub fn consolidate_net_routes(
    routes: &[Route],
    pad_positions: &[(f64, f64)],
    tolerance: f64,
    path_is_clear: Pred<'_>,
) -> (Vec<Route>, ConsolidationStats) {
    let mut flat = Vec::new();
    let mut owner = Vec::new();
    let mut protected: Vec<(f64, f64)> = pad_positions.to_vec();
    for (ri, route) in routes.iter().enumerate() {
        for seg in &route.segments {
            flat.push(seg.clone());
            owner.push(ri);
        }
        protected.extend(route.vias.iter().map(|v| (v.x, v.y)));
    }
    let base = ConsolidationStats {
        segments_before: flat.len(),
        segments_after: flat.len(),
        ..Default::default()
    };
    if flat.len() < 2 {
        return (routes.to_vec(), base);
    }
    let same_owner = |a: usize, b: usize| owner[a] == owner[b];
    let (indexed, raw) = consolidate_indexed(
        &flat,
        &protected,
        tolerance,
        path_is_clear,
        Some(&same_owner),
    );
    if raw.runs_merged == 0 {
        return (
            routes.to_vec(),
            ConsolidationStats {
                runs_vetoed: raw.runs_vetoed,
                ..base
            },
        );
    }
    let mut by_owner: Vec<Vec<Segment>> = vec![Vec::new(); routes.len()];
    for (src, seg) in &indexed {
        by_owner[owner[*src]].push(seg.clone());
    }
    let mut out = Vec::new();
    let mut changed = 0;
    for (ri, route) in routes.iter().enumerate() {
        let segs = std::mem::take(&mut by_owner[ri]);
        if segs.len() == route.segments.len() {
            out.push(route.clone());
            continue;
        }
        changed += 1;
        let mut r = route.clone();
        r.segments = segs;
        out.push(r);
    }
    (
        out,
        ConsolidationStats {
            segments_before: flat.len(),
            segments_after: indexed.len(),
            runs_merged: raw.runs_merged,
            runs_vetoed: raw.runs_vetoed,
            routes_changed: changed,
        },
    )
}

/// Consolidate every route on `router` per net, through the grid-synced
/// transform. `pads_by_net` supplies pad positions to protect.
pub fn consolidate_routes_grid_synced<R: SyncedRouter>(
    router: &mut R,
    skip_nets: Option<&HashSet<i64>>,
    path_is_clear: Pred<'_>,
    tolerance: f64,
    pads_by_net: &HashMap<i64, Vec<(f64, f64)>>,
) -> ConsolidationStats {
    let routes = router.routes();
    let mut net_order: Vec<i64> = Vec::new();
    let mut by_net: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, route) in routes.iter().enumerate() {
        if skip_nets.is_some_and(|s| s.contains(&route.net)) {
            continue;
        }
        by_net
            .entry(route.net)
            .or_insert_with(|| {
                net_order.push(route.net);
                Vec::new()
            })
            .push(i);
    }
    let mut replacements: HashMap<usize, Route> = HashMap::new();
    let mut total = ConsolidationStats::default();
    for net in net_order {
        let idxs = &by_net[&net];
        let net_routes: Vec<Route> = idxs.iter().map(|&i| routes[i].clone()).collect();
        let pads = pads_by_net.get(&net).map(Vec::as_slice).unwrap_or(&[]);
        let (new_routes, stats) =
            consolidate_net_routes(&net_routes, pads, tolerance, path_is_clear);
        total.segments_before += stats.segments_before;
        total.segments_after += stats.segments_after;
        total.runs_merged += stats.runs_merged;
        total.runs_vetoed += stats.runs_vetoed;
        total.routes_changed += stats.routes_changed;
        for (&i, new) in idxs.iter().zip(new_routes) {
            if new != routes[i] {
                replacements.insert(i, new);
            }
        }
    }
    if !replacements.is_empty() {
        // Upstream keys replacements by route identity; here by position.
        // The transform runs once per non-skipped route, in order.
        let counter = std::cell::Cell::new(0usize);
        let mut all: Vec<usize> = (0..routes.len()).collect();
        if let Some(skip) = skip_nets {
            all.retain(|&i| !skip.contains(&routes[i].net));
        }
        let remap: HashMap<usize, usize> = all.iter().enumerate().map(|(k, &i)| (k, i)).collect();
        let transform = |route: &Route| {
            let k = counter.get();
            counter.set(k + 1);
            remap
                .get(&k)
                .and_then(|i| replacements.get(i))
                .cloned()
                .unwrap_or_else(|| route.clone())
        };
        apply_route_transform_grid_synced(router, &transform, skip_nets, None);
    }
    total
}
