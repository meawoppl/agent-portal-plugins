//! Trace width-consistency audit (port of
//! `kicad_tools.validate.rules.width_consistency`): short width islands and
//! width transitions on two-terminal routes that no nearby other-net
//! clearance explains. Opt-in heuristic, warning severity by default.

use std::collections::HashMap;

use super::clearance::{pad_on_layer, pad_polygon};
use crate::core::layers::via_spans_layer;
use crate::geometry::shapely::{self as sh, Bounds, Geom};
use crate::geometry::strtree::StrTree;
use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

const ARC_SAMPLE_ERROR_MM: f64 = 0.001;

type C = (f64, f64);
type NodeKey = (i64, i64);

#[derive(Debug, Clone)]
struct Track {
    points: Vec<C>,
    length: f64,
    width: f64,
    net_label: String,
    uuid: String,
}

impl Track {
    fn ends(&self) -> (C, C) {
        (self.points[0], self.points[self.points.len() - 1])
    }

    fn reversed(&self) -> Track {
        let mut t = self.clone();
        t.points.reverse();
        t
    }
}

#[derive(Debug, Clone)]
struct Run {
    tracks: Vec<Track>,
    width: f64,
}

impl Run {
    fn length(&self) -> f64 {
        // Python `sum(...)` (Neumaier compensated in CPython 3.12).
        crate::utils::pymath::py_sum(self.tracks.iter().map(|t| t.length))
    }
}

struct Chain {
    tracks: Vec<Track>,
    start: NodeKey,
    end: NodeKey,
}

struct Terminal {
    net_key: String,
    geometry: Geom,
    label: String,
    min_dimension: f64,
}

struct Obstacle {
    net_key: String,
    geometry: Vec<Geom>,
    bounds: Option<Bounds>,
    label: String,
}

/// Upstream `WidthConsistencyRule` (keyword options as fields).
#[derive(Debug, Clone)]
pub struct WidthConsistencyRule {
    pub max_island_length_mm: f64,
    pub width_tolerance_mm: f64,
    pub clearance_mm: Option<f64>,
    pub obstacle_search_mm: f64,
    pub severity: String,
    pub report_islands: bool,
    pub report_transitions: bool,
    pub report_justified: bool,
    pub node_tolerance_mm: f64,
}

impl Default for WidthConsistencyRule {
    fn default() -> Self {
        WidthConsistencyRule {
            max_island_length_mm: 3.0,
            width_tolerance_mm: 0.001,
            clearance_mm: None,
            obstacle_search_mm: 1.0,
            severity: "warning".into(),
            report_islands: true,
            report_transitions: true,
            report_justified: false,
            node_tolerance_mm: 0.0005,
        }
    }
}

fn geom_distance(a: &[Geom], b: &[Geom]) -> f64 {
    let mut best = f64::INFINITY;
    for x in a {
        for y in b {
            let d = sh::distance(x, y);
            if d < best {
                best = d;
            }
        }
    }
    best
}

impl WidthConsistencyRule {
    /// Apply `{name: value}` options (booleans as non-zero).
    pub fn with_options(mut self, opts: &[(String, f64)]) -> Self {
        for (k, v) in opts {
            match k.as_str() {
                "max_island_length_mm" => self.max_island_length_mm = *v,
                "width_tolerance_mm" => self.width_tolerance_mm = *v,
                "clearance_mm" => self.clearance_mm = Some(*v),
                "obstacle_search_mm" => self.obstacle_search_mm = *v,
                "report_islands" => self.report_islands = *v != 0.0,
                "report_transitions" => self.report_transitions = *v != 0.0,
                "report_justified" => self.report_justified = *v != 0.0,
                "node_tolerance_mm" => self.node_tolerance_mm = *v,
                _ => {}
            }
        }
        self
    }

    fn net_key(names: &[(i64, String)], number: i64, name: &str) -> Option<String> {
        if !name.is_empty() {
            return Some(name.to_string());
        }
        if let Some((_, n)) = names.iter().rev().find(|(k, _)| *k == number) {
            if !n.is_empty() {
                return Some(n.clone());
            }
        }
        (number != 0).then(|| format!("#{number}"))
    }

    fn tracks_by_net(&self, pcb: &Pcb, names: &[(i64, String)], layer: &str) -> Vec<(String, Vec<Track>)> {
        let mut by: Vec<(String, Vec<Track>)> = Vec::new();
        let mut push = |key: String, t: Track| match by.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => e.1.push(t),
            None => by.push((key, vec![t])),
        };
        for s in pcb.segments_on_layer(layer) {
            let Some(key) = Self::net_key(names, s.net_number, &s.net_name) else {
                continue;
            };
            if s.width <= 0.0 {
                continue;
            }
            let length = sh::dist(s.start, s.end);
            if length <= self.node_tolerance_mm {
                continue;
            }
            push(
                key,
                Track {
                    points: vec![s.start, s.end],
                    length,
                    width: s.width,
                    net_label: if s.net_name.is_empty() {
                        format!("net {}", s.net_number)
                    } else {
                        s.net_name.clone()
                    },
                    uuid: s.uuid.clone(),
                },
            );
        }
        for a in pcb.arcs_on_layer(layer) {
            let Some(key) = Self::net_key(names, a.net_number, &a.net_name) else {
                continue;
            };
            if a.width <= 0.0 {
                continue;
            }
            let Ok(points) = a.centerline_points(ARC_SAMPLE_ERROR_MM) else {
                continue;
            };
            let length = a.length();
            if length <= self.node_tolerance_mm {
                continue;
            }
            push(
                key,
                Track {
                    points,
                    length,
                    width: a.width,
                    net_label: if a.net_name.is_empty() {
                        format!("net {}", a.net_number)
                    } else {
                        a.net_name.clone()
                    },
                    uuid: a.uuid.clone(),
                },
            );
        }
        by
    }

    fn layer_copper(&self, pcb: &Pcb, names: &[(i64, String)], layer: &str) -> (Vec<Terminal>, Vec<Obstacle>) {
        let mut terms = Vec::new();
        let mut obs = Vec::new();
        let (ox, oy) = pcb.board_origin();
        for fp in pcb.footprints() {
            for pad in &fp.pads {
                if !pad_on_layer(pad, layer) {
                    continue;
                }
                let Some(poly) = pad_polygon(pad, fp).filter(|p| !p.is_empty()) else {
                    continue;
                };
                let label = format!("pad {}.{}", fp.reference, pad.number);
                let key = Self::net_key(names, pad.net_number, &pad.net_name);
                if let Some(k) = &key {
                    terms.push(Terminal {
                        net_key: k.clone(),
                        geometry: poly.clone(),
                        label: label.clone(),
                        min_dimension: pad.size.0.min(pad.size.1),
                    });
                }
                obs.push(Obstacle {
                    net_key: key.unwrap_or_else(|| format!("<unassigned {label}>")),
                    bounds: poly.bounds(),
                    geometry: vec![poly],
                    label,
                });
            }
        }
        for v in pcb.vias() {
            if v.size <= 0.0 || !via_spans_layer(&v.layers, layer) {
                continue;
            }
            let disc = sh::point_buffer_q(v.position, v.size / 2.0, 16);
            let label = format!("via @({:.3},{:.3})", v.position.0 + ox, v.position.1 + oy);
            let key = Self::net_key(names, v.net_number, &v.net_name);
            if let Some(k) = &key {
                terms.push(Terminal {
                    net_key: k.clone(),
                    geometry: disc.clone(),
                    label: label.clone(),
                    min_dimension: f64::INFINITY,
                });
            }
            obs.push(Obstacle {
                net_key: key.unwrap_or_else(|| format!("<unassigned {label}>")),
                bounds: disc.bounds(),
                geometry: vec![disc],
                label,
            });
        }
        let mut track_obstacle = |pts: Vec<C>, width: f64, uuid: &str, net_name: &str, number: i64| {
            let label = if uuid.is_empty() {
                format!("track on {net_name}")
            } else {
                format!("track {uuid}")
            };
            let key = Self::net_key(names, number, net_name);
            let geom = if pts.len() == 1 {
                sh::point_buffer(pts[0], width / 2.0)
            } else {
                sh::buffer_line(&pts, width / 2.0)
            };
            obs.push(Obstacle {
                net_key: key.unwrap_or_else(|| format!("<unassigned {label}>")),
                bounds: geom.bounds(),
                geometry: vec![geom],
                label,
            });
        };
        for s in pcb.segments_on_layer(layer) {
            if s.width <= 0.0 {
                continue;
            }
            let pts = if sh::dist(s.start, s.end) > 0.0 {
                vec![s.start, s.end]
            } else {
                vec![s.start]
            };
            track_obstacle(pts, s.width, &s.uuid, &s.net_name, s.net_number);
        }
        for a in pcb.arcs_on_layer(layer) {
            if a.width <= 0.0 {
                continue;
            }
            if let Ok(pts) = a.centerline_points(ARC_SAMPLE_ERROR_MM) {
                track_obstacle(pts, a.width, &a.uuid, &a.net_name, a.net_number);
            }
        }
        (terms, obs)
    }

    fn node(&self, p: C) -> NodeKey {
        let q = self.node_tolerance_mm;
        ((p.0 / q).round_ties_even() as i64, (p.1 / q).round_ties_even() as i64)
    }

    fn end_nodes(&self, tracks: &[Track]) -> Vec<(NodeKey, NodeKey)> {
        let tol = self.node_tolerance_mm;
        let points: Vec<C> = tracks
            .iter()
            .flat_map(|t| {
                let (a, b) = t.ends();
                [a, b]
            })
            .collect();
        let buckets: Vec<NodeKey> = points.iter().map(|p| self.node(*p)).collect();
        let mut by_bucket: HashMap<NodeKey, Vec<usize>> = HashMap::new();
        for (i, k) in buckets.iter().enumerate() {
            by_bucket.entry(*k).or_default().push(i);
        }
        let mut parent: Vec<usize> = (0..points.len()).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for (i, &(bx, by)) in buckets.iter().enumerate() {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let Some(js) = by_bucket.get(&(bx + dx, by + dy)) else {
                        continue;
                    };
                    for &j in js {
                        if j <= i || ((dx != 0 || dy != 0) && sh::dist(points[i], points[j]) > tol) {
                            continue;
                        }
                        let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                        if ri != rj {
                            parent[rj] = ri;
                        }
                    }
                }
            }
        }
        let mut canonical: HashMap<usize, NodeKey> = HashMap::new();
        for (i, k) in buckets.iter().enumerate() {
            let r = find(&mut parent, i);
            match canonical.get(&r) {
                Some(c) if c <= k => {}
                _ => {
                    canonical.insert(r, *k);
                }
            }
        }
        let keys: Vec<NodeKey> = (0..points.len())
            .map(|i| canonical[&find(&mut parent, i)])
            .collect();
        (0..tracks.len()).map(|t| (keys[2 * t], keys[2 * t + 1])).collect()
    }

    fn chains(&self, tracks: &[Track], terminals: &[&Terminal]) -> (Vec<Chain>, Vec<NodeKey>) {
        let end_nodes = self.end_nodes(tracks);
        let mut incident: Vec<(NodeKey, Vec<usize>)> = Vec::new();
        let mut coords: Vec<(NodeKey, C)> = Vec::new();
        for (i, t) in tracks.iter().enumerate() {
            let (ea, eb) = t.ends();
            for (end, key) in [(ea, end_nodes[i].0), (eb, end_nodes[i].1)] {
                match incident.iter_mut().find(|(k, _)| *k == key) {
                    Some(e) => e.1.push(i),
                    None => incident.push((key, vec![i])),
                }
                if !coords.iter().any(|(k, _)| *k == key) {
                    coords.push((key, end));
                }
            }
        }
        let inc = |k: &NodeKey| -> &Vec<usize> { &incident.iter().find(|(x, _)| x == k).unwrap().1 };
        let at_terminal: Vec<NodeKey> = coords
            .iter()
            .filter(|(_, xy)| {
                terminals
                    .iter()
                    .any(|t| sh::distance(&t.geometry, &Geom::Point(*xy)) <= self.node_tolerance_mm)
            })
            .map(|(k, _)| *k)
            .collect();
        let mut stops: Vec<NodeKey> = incident
            .iter()
            .filter(|(_, v)| v.len() != 2)
            .map(|(k, _)| *k)
            .chain(at_terminal.iter().copied())
            .collect();
        stops.sort();
        stops.dedup();
        let mut chains = Vec::new();
        let mut used = vec![false; tracks.len()];
        for &start in &stops {
            for &first in inc(&start) {
                if used[first] {
                    continue;
                }
                let mut ordered = Vec::new();
                let (mut node, mut idx) = (start, first);
                let nxt = loop {
                    used[idx] = true;
                    let (a, b) = end_nodes[idx];
                    let nxt = if a == node { b } else { a };
                    ordered.push(if a == node {
                        tracks[idx].clone()
                    } else {
                        tracks[idx].reversed()
                    });
                    if stops.binary_search(&nxt).is_ok() {
                        break nxt;
                    }
                    let cands: Vec<usize> = inc(&nxt).iter().copied().filter(|&j| j != idx).collect();
                    if cands.is_empty() || used[cands[0]] {
                        break nxt;
                    }
                    node = nxt;
                    idx = cands[0];
                };
                chains.push(Chain {
                    tracks: ordered,
                    start,
                    end: nxt,
                });
            }
        }
        (chains, at_terminal)
    }

    fn runs(&self, chain: &Chain) -> Vec<Run> {
        let mut runs: Vec<Run> = Vec::new();
        for t in &chain.tracks {
            match runs.last_mut() {
                Some(r) if (r.width - t.width).abs() <= self.width_tolerance_mm => r.tracks.push(t.clone()),
                _ => runs.push(Run {
                    tracks: vec![t.clone()],
                    width: t.width,
                }),
            }
        }
        runs
    }

    pub fn check(&self, pcb: &Pcb, design_rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        results.set_rule("width_consistency", 1);
        let clearance = self.clearance_mm.unwrap_or(design_rules.min_clearance_mm);
        let names: Vec<(i64, String)> = pcb.nets().iter().map(|n| (n.number, n.name.clone())).collect();
        for layer in pcb.copper_layers() {
            let by_net = self.tracks_by_net(pcb, &names, &layer.name);
            if by_net.is_empty() {
                continue;
            }
            let (terms, obs) = self.layer_copper(pcb, &names, &layer.name);
            let tree = StrTree::new(&obs.iter().map(|o| o.bounds).collect::<Vec<_>>());
            for (net_key, tracks) in &by_net {
                let net_terms: Vec<&Terminal> = terms.iter().filter(|t| t.net_key == *net_key).collect();
                let (chains, terminal_nodes) = self.chains(tracks, &net_terms);
                for chain in &chains {
                    self.audit_chain(
                        chain,
                        &layer.name,
                        net_key,
                        &terminal_nodes,
                        &net_terms,
                        (&tree, &obs),
                        clearance,
                        &mut results,
                    );
                }
            }
        }
        results
    }

    #[allow(clippy::too_many_arguments)]
    fn audit_chain(
        &self,
        chain: &Chain,
        layer: &str,
        net_key: &str,
        terminal_nodes: &[NodeKey],
        terminals: &[&Terminal],
        index: (&StrTree, &[Obstacle]),
        clearance: f64,
        results: &mut DRCResults,
    ) {
        let runs = self.runs(chain);
        if runs.len() < 2 {
            return;
        }
        let tol = self.width_tolerance_mm;
        let mut island_bounds: Vec<usize> = Vec::new();
        for i in 1..runs.len() - 1 {
            let r = &runs[i];
            if !(runs[i - 1].width < r.width - tol && runs[i + 1].width < r.width - tol) {
                continue;
            }
            if r.length() >= self.max_island_length_mm {
                continue;
            }
            island_bounds.push(i - 1);
            island_bounds.push(i);
            if self.report_islands {
                results.add(self.island_violation(r, &runs[i - 1], &runs[i + 1], layer));
            }
        }
        if !self.report_transitions {
            return;
        }
        if !(terminal_nodes.contains(&chain.start) && terminal_nodes.contains(&chain.end)) {
            return;
        }
        let mut necks: Vec<(usize, (f64, C))> = Vec::new();
        for i in 0..runs.len() - 1 {
            if island_bounds.contains(&i) {
                continue;
            }
            let (l, r) = (&runs[i], &runs[i + 1]);
            let narrow = if l.width < r.width { i } else { i + 1 };
            let wide = l.width.max(r.width);
            let last = l.tracks.last().unwrap();
            let at = last.points[last.points.len() - 1];
            match necks.iter_mut().find(|(k, _)| *k == narrow) {
                Some(e) => {
                    if wide > e.1 .0 {
                        e.1 = (wide, at);
                    }
                }
                None => necks.push((narrow, (wide, at))),
            }
        }
        necks.sort_by_key(|(k, _)| *k);
        for (narrow, (wide, at)) in necks {
            if let Some(v) = self.transition_finding(&runs[narrow], wide, at, layer, net_key, terminals, index, clearance) {
                results.add(v);
            }
        }
    }

    fn island_violation(&self, run: &Run, before: &Run, after: &Run, layer: &str) -> DRCViolation {
        let line: Vec<C> = run.tracks.iter().flat_map(|t| t.points.iter().copied()).collect();
        let mid = interpolate_half(&line);
        let net = run.tracks[0].net_label.clone();
        DRCViolation::new(
            "width_island",
            &self.severity,
            format!(
                "Width island on '{net}' ({layer}): {:.3} mm for {:.3} mm between {:.3} mm and \
                 {:.3} mm copper (< {:.3} mm); use one width for the run or make the wide \
                 section deliberate",
                run.width,
                run.length(),
                before.width,
                after.width,
                self.max_island_length_mm
            ),
        )
        .at(py_round(mid.0, 4), py_round(mid.1, 4))
        .layer(layer)
        .actual(py_round(run.length(), 4))
        .required(self.max_island_length_mm)
        .items(run.tracks.iter().filter(|t| !t.uuid.is_empty()).map(|t| t.uuid.clone()))
        .nets([net])
    }

    #[allow(clippy::too_many_arguments)]
    fn transition_finding(
        &self,
        narrow: &Run,
        wide_width: f64,
        at: C,
        layer: &str,
        net_key: &str,
        terminals: &[&Terminal],
        (tree, obstacles): (&StrTree, &[Obstacle]),
        clearance: f64,
    ) -> Option<DRCViolation> {
        let actual: Vec<Geom> = narrow
            .tracks
            .iter()
            .map(|t| sh::buffer_line(&t.points, narrow.width / 2.0))
            .collect();
        let proposal: Vec<Geom> = narrow
            .tracks
            .iter()
            .map(|t| sh::buffer_line(&t.points, wide_width / 2.0))
            .collect();
        let mut env: Option<Bounds> = None;
        for g in &proposal {
            if let Some(b) = g.bounds() {
                env = Some(match env {
                    None => b,
                    Some(e) => (e.0.min(b.0), e.1.min(b.1), e.2.max(b.2), e.3.max(b.3)),
                });
            }
        }
        let r = clearance + self.obstacle_search_mm;
        let mut nearest: Option<(f64, f64, String)> = None;
        if let Some(e) = env {
            for idx in tree.query((e.0 - r, e.1 - r, e.2 + r, e.3 + r)) {
                let o = &obstacles[idx];
                if o.net_key == net_key {
                    continue;
                }
                let after = geom_distance(&proposal, &o.geometry);
                if nearest.as_ref().is_none_or(|n| after < n.0) {
                    nearest = Some((after, geom_distance(&actual, &o.geometry), o.label.clone()));
                }
            }
        }
        let mut reason = String::new();
        if let Some(n) = nearest.as_ref().filter(|n| n.0 < clearance - 1e-6) {
            reason = format!("widening would leave {:.3} mm to {}", n.0, n.2);
        } else {
            let first = narrow.tracks[0].points[0];
            let lt = narrow.tracks.last().unwrap();
            let last = lt.points[lt.points.len() - 1];
            for t in terminals {
                if t.min_dimension < wide_width
                    && [first, last]
                        .iter()
                        .any(|e| sh::distance(&t.geometry, &Geom::Point(*e)) <= self.node_tolerance_mm)
                {
                    reason = format!("pad escape into {} ({:.3} mm)", t.label, t.min_dimension);
                    break;
                }
            }
        }
        let net = narrow.tracks[0].net_label.clone();
        let head = format!(
            "Width transition on '{net}' ({layer}): {:.3} mm run of {:.3} mm meets {wide_width:.3} mm",
            narrow.width,
            narrow.length()
        );
        let (severity, message) = if !reason.is_empty() {
            if !self.report_justified {
                return None;
            }
            ("info".to_string(), format!("{head}; neck-down justified: {reason}"))
        } else {
            let context = match &nearest {
                None => format!("no other-net copper within {:.3} mm", clearance + self.obstacle_search_mm),
                Some(n) => format!(
                    "nearest other-net copper {} is {:.3} mm away ({:.3} mm if widened; clearance \
                     {clearance:.3} mm)",
                    n.2, n.1, n.0
                ),
            };
            (
                self.severity.clone(),
                format!(
                    "{head}; no clearance constraint requires the neck-down -- {context}. Consider \
                     widening the narrow run to {wide_width:.3} mm"
                ),
            )
        };
        Some(
            DRCViolation::new("width_transition", &severity, message)
                .at(py_round(at.0, 4), py_round(at.1, 4))
                .layer(layer)
                .actual_opt(nearest.as_ref().map(|n| py_round(n.1, 4)))
                .required(clearance)
                .items(narrow.tracks.iter().filter(|t| !t.uuid.is_empty()).map(|t| t.uuid.clone()))
                .nets([net]),
        )
    }
}

/// `LineString(pts).interpolate(0.5, normalized=True)` (GEOS
/// `LengthIndexedLine`).
fn interpolate_half(pts: &[C]) -> C {
    let lens: Vec<f64> = pts.windows(2).map(|w| sh::dist(w[0], w[1])).collect();
    let total: f64 = lens.iter().sum();
    let target = 0.5 * total;
    let mut acc = 0.0;
    for (i, l) in lens.iter().enumerate() {
        if acc + l > target && *l > 0.0 {
            let frac = (target - acc) / l;
            let (a, b) = (pts[i], pts[i + 1]);
            return (a.0 + frac * (b.0 - a.0), a.1 + frac * (b.1 - a.1));
        }
        acc += l;
    }
    pts[pts.len() - 1]
}
