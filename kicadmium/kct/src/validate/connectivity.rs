//! Port of `kicad_tools.validate.connectivity`: the label-aware
//! `ConnectivityValidator` report, the label-free physical pad partition
//! (`extract_pad_occurrences`, the copper-LVS primitive) and the copper
//! geometry helpers shared with the net-status analyzer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::geometry::copper::{point_segment_distance, segments_copper_touch};
use crate::geometry::shapely::{self as sh, Geom, Poly};
use crate::geometry::strtree::StrTree;
use crate::pyjson::Json;
use crate::schema::pcb::{Footprint, Pad, Pcb, Segment, Via};
use crate::utils::pyset::{hash_int, PySetOrder};

/// Inward erosion applied to pad / via copper before pour bonding.
pub const POUR_PAD_ERODE: f64 = 0.1;

/// `_transform_pad_position(pad_local, fp_x, fp_y, rotation)`.
pub fn transform_pad_position(local: (f64, f64), fp_pos: (f64, f64), rotation: f64) -> (f64, f64) {
    let a = (-rotation).to_radians();
    let (c, s) = (a.cos(), a.sin());
    let (px, py) = local;
    (fp_pos.0 + (px * c - py * s), fp_pos.1 + (px * s + py * c))
}

/// `_pad_copper_polygon(fp, pad, shape_aware)`: rotated size box (or the
/// pad's circle / stadium when `shape_aware`), eroded by
/// [`POUR_PAD_ERODE`]; the un-eroded outline when erosion empties it.
pub fn pad_copper_polygon(fp: &Footprint, pad: &Pad, shape_aware: bool) -> Option<Geom> {
    let (cx, cy) = transform_pad_position(pad.position, fp.position, fp.rotation);
    let (w, h) = pad.size;
    if w <= 0.0 || h <= 0.0 {
        return Some(Geom::Point((cx, cy)));
    }
    let a = (-fp.rotation).to_radians();
    let (c, s) = (a.cos(), a.sin());
    let to_board = |ox: f64, oy: f64| (cx + ox * c - oy * s, cy + ox * s + oy * c);
    let shape = if shape_aware {
        pad.shape.to_lowercase()
    } else {
        String::new()
    };
    let base = if shape == "circle" || shape == "oval" {
        let r = (if h < w { h } else { w }) / 2.0;
        let half = (w - h).abs() / 2.0;
        if half <= 0.0 {
            sh::point_buffer((cx, cy), r)
        } else {
            let (e0, e1) = if w >= h {
                ((-half, 0.0), (half, 0.0))
            } else {
                ((0.0, -half), (0.0, half))
            };
            sh::segment_buffer(to_board(e0.0, e0.1), to_board(e1.0, e1.1), r)
        }
    } else {
        Geom::Poly(Poly::new(vec![
            to_board(-w / 2.0, -h / 2.0),
            to_board(w / 2.0, -h / 2.0),
            to_board(w / 2.0, h / 2.0),
            to_board(-w / 2.0, h / 2.0),
        ]))
    };
    let eroded = sh::erode_convex(&base, POUR_PAD_ERODE);
    if !eroded.is_empty() {
        return Some(eroded);
    }
    Some(base)
}

/// `_fill_solid_region`: the fill polygon, `buffer(0)`-repaired when invalid.
pub fn fill_solid_region(points: &[(f64, f64)]) -> Option<Geom> {
    if points.len() < 3 {
        return None;
    }
    // `Polygon(points)` then `buffer(0)` when invalid.
    let g = sh::buffer0(&Poly::new(points.to_vec()));
    (!g.is_empty()).then_some(g)
}

/// `_via_copper_geom`: eroded via disc (full disc when erosion empties it,
/// a point for degenerate vias).
pub fn via_copper_geom(pos: (f64, f64), radius: f64) -> Geom {
    if radius <= 0.0 {
        return Geom::Point(pos);
    }
    let circle = sh::point_buffer(pos, radius);
    let eroded = sh::erode_convex(&circle, POUR_PAD_ERODE);
    if !eroded.is_empty() {
        return eroded;
    }
    circle
}

/// `_physical_via_annulus`: raw annular copper (quad 64).
pub fn physical_via_annulus(via: &Via) -> Geom {
    let r = via.size.max(0.0) / 2.0;
    if r <= 0.0 {
        return Geom::Empty;
    }
    sh::annulus(via.position, r, via.drill / 2.0, 64)
}

// =================================================== ConnectivityValidator

fn jobj_from(pairs: Vec<(&str, Json)>) -> Json {
    Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// One net connectivity issue.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectivityIssue {
    /// `error` or `warning`.
    pub severity: String,
    /// `unrouted`, `partial`, `isolated` or `zone_island`.
    pub issue_type: String,
    pub net_name: String,
    pub message: String,
    pub suggestion: String,
    pub connected_pads: Vec<String>,
    pub unconnected_pads: Vec<String>,
    pub islands: Vec<Vec<String>>,
}

impl ConnectivityIssue {
    pub fn is_error(&self) -> bool {
        self.severity == "error"
    }

    pub fn is_warning(&self) -> bool {
        self.severity == "warning"
    }

    pub fn to_dict(&self) -> Json {
        let strs = |v: &[String]| Json::Arr(v.iter().map(|s| Json::Str(s.clone())).collect());
        jobj_from(vec![
            ("severity", Json::Str(self.severity.clone())),
            ("issue_type", Json::Str(self.issue_type.clone())),
            ("net_name", Json::Str(self.net_name.clone())),
            ("message", Json::Str(self.message.clone())),
            ("suggestion", Json::Str(self.suggestion.clone())),
            ("connected_pads", strs(&self.connected_pads)),
            ("unconnected_pads", strs(&self.unconnected_pads)),
            (
                "islands",
                Json::Arr(self.islands.iter().map(|i| strs(i)).collect()),
            ),
        ])
    }
}

/// All connectivity issues of a board.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConnectivityResult {
    pub issues: Vec<ConnectivityIssue>,
    pub total_nets: usize,
    pub connected_nets: usize,
    pub zone_connected_nets: usize,
}

impl ConnectivityResult {
    pub fn has_issues(&self) -> bool {
        !self.issues.is_empty()
    }

    pub fn error_count(&self) -> usize {
        self.issues.iter().filter(|i| i.is_error()).count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues.iter().filter(|i| i.is_warning()).count()
    }

    pub fn is_fully_routed(&self) -> bool {
        self.error_count() == 0
    }

    fn of_type(&self, t: &str) -> usize {
        self.issues.iter().filter(|i| i.issue_type == t).count()
    }

    pub fn unconnected_pad_count(&self) -> usize {
        self.issues.iter().map(|i| i.unconnected_pads.len()).sum()
    }

    pub fn to_dict(&self) -> Json {
        jobj_from(vec![
            ("is_fully_routed", Json::Bool(self.is_fully_routed())),
            ("total_nets", Json::Int(self.total_nets as i64)),
            ("connected_nets", Json::Int(self.connected_nets as i64)),
            (
                "zone_connected_nets",
                Json::Int(self.zone_connected_nets as i64),
            ),
            ("error_count", Json::Int(self.error_count() as i64)),
            ("warning_count", Json::Int(self.warning_count() as i64)),
            (
                "unconnected_pads",
                Json::Int(self.unconnected_pad_count() as i64),
            ),
            (
                "issues",
                Json::Arr(self.issues.iter().map(ConnectivityIssue::to_dict).collect()),
            ),
        ])
    }

    pub fn summary(&self) -> String {
        let status = if self.is_fully_routed() {
            "FULLY ROUTED"
        } else {
            "CONNECTIVITY ISSUES"
        };
        let mut parts = vec![
            format!(
                "Net Connectivity {status}: {} errors, {} warnings",
                self.error_count(),
                self.warning_count()
            ),
            format!(
                "  Nets: {}/{} fully connected",
                self.connected_nets, self.total_nets
            ),
        ];
        if self.zone_connected_nets > 0 {
            parts.push(format!(
                "  Zone-connected nets: {} (verified by geometry)",
                self.zone_connected_nets
            ));
        }
        for (t, label) in [
            ("unrouted", "Unrouted nets"),
            ("partial", "Partial connections"),
            ("isolated", "Isolated pads"),
            ("zone_island", "Orphaned zone islands"),
        ] {
            let n = self.of_type(t);
            if n > 0 {
                parts.push(format!("  {label}: {n}"));
            }
        }
        parts.push(format!(
            "  Total unconnected pads: {}",
            self.unconnected_pad_count()
        ));
        parts.join("\n")
    }
}

type Graph = HashMap<String, HashSet<String>>;
/// Occurrence node -> `(ref, pad)`, in pad order.
pub type PadBindings = Vec<(String, (String, String))>;
/// Node positions plus node -> layer list.
type PadNodes = (Vec<(String, (f64, f64))>, HashMap<String, Vec<String>>);
/// `(stable id, description, [(layer, geometry)], terminal)`.
type RelationItem = (String, String, Vec<(String, Geom)>, bool);
type Bridge = ((f64, f64), Option<Vec<String>>);

const POSITION_TOLERANCE: f64 = 0.01;

/// `ConnectivityValidator`: physical copper connectivity of a board.
pub struct ConnectivityValidator {
    pub pcb: Pcb,
    pub pcb_path: Option<PathBuf>,
    pad_ids: HashMap<(usize, usize), String>,
    /// Occurrence node -> `(ref, pad)` in pad order.
    pub pad_bindings: PadBindings,
    last_zone_connected_pads: HashSet<String>,
}

fn add_edge(g: &mut Graph, a: &str, b: &str) {
    if a == b {
        return;
    }
    g.entry(a.to_string()).or_default().insert(b.to_string());
    g.entry(b.to_string()).or_default().insert(a.to_string());
}

fn points_close(a: (f64, f64), b: (f64, f64)) -> bool {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    dx * dx + dy * dy < POSITION_TOLERANCE * POSITION_TOLERANCE
}

fn pad_layer_matches_zone(pad_layers: &[String], zone_layer: &str) -> bool {
    pad_layers.iter().any(|l| {
        l == zone_layer
            || (l == "*.Cu" && zone_layer.ends_with(".Cu"))
            || (l.starts_with("*.") && zone_layer.ends_with(&l[1..]))
    })
}

fn pad_copper_on_layer(layers: &[String], layer: &str) -> bool {
    layers.iter().any(|l| l == layer || l == "*.Cu")
}

/// Ray-casting `_point_in_polygon`.
fn point_in_polygon(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let (x, y) = p;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// `segment_copper_polygon(start, end, width)` (shapely buffer, quad 16).
pub fn segment_copper_geom(start: (f64, f64), end: (f64, f64), width: f64) -> Geom {
    if start == end {
        if width <= 0.0 {
            return Geom::Point(start);
        }
        return sh::point_buffer(start, width / 2.0);
    }
    if width <= 0.0 {
        return Geom::Line(vec![start, end]);
    }
    sh::segment_buffer(start, end, width / 2.0)
}

impl ConnectivityValidator {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        let pcb = Pcb::load(path)?;
        let mut v = Self::from_pcb(pcb);
        v.pcb_path = Some(path.to_path_buf());
        Ok(v)
    }

    pub fn from_pcb(pcb: Pcb) -> Self {
        let mut v = ConnectivityValidator {
            pcb,
            pcb_path: None,
            pad_ids: HashMap::new(),
            pad_bindings: vec![],
            last_zone_connected_pads: HashSet::new(),
        };
        v.refresh_pad_identities();
        v
    }

    fn refresh_pad_identities(&mut self) {
        let mut pads = Vec::new();
        for (fi, fp) in self.pcb.footprints().iter().enumerate() {
            if fp.reference.is_empty() || fp.reference.starts_with('#') {
                continue;
            }
            for (pi, pad) in fp.pads.iter().enumerate() {
                if pad.number.is_empty() {
                    continue;
                }
                pads.push((fi, pi, fp.reference.clone(), pad.number.clone()));
            }
        }
        let mut counts: HashMap<String, usize> = HashMap::new();
        for (_, _, r, n) in &pads {
            *counts.entry(format!("{r}.{n}")).or_default() += 1;
        }
        self.pad_ids.clear();
        self.pad_bindings.clear();
        for (fi, pi, r, n) in pads {
            let logical = format!("{r}.{n}");
            let node = if counts[&logical] == 1 {
                logical
            } else {
                format!("__pad:{fi}:{pi}")
            };
            self.pad_ids.insert((fi, pi), node.clone());
            match self.pad_bindings.iter_mut().find(|(k, _)| *k == node) {
                Some(e) => e.1 = (r, n),
                None => self.pad_bindings.push((node, (r, n))),
            }
        }
    }

    fn pad_display(&self, node: &str) -> String {
        self.pad_bindings
            .iter()
            .find(|(k, _)| k == node)
            .map(|(_, (r, n))| format!("{r}.{n}"))
            .unwrap_or_else(|| node.to_string())
    }

    fn copper_layer_order(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .pcb
            .copper_layers()
            .iter()
            .map(|l| l.name.clone())
            .collect();
        if names.is_empty() {
            names = vec!["F.Cu".into(), "B.Cu".into()];
        }
        names.sort();
        names.dedup();
        let inner_index = |n: &str| n[2..n.len() - 3].trim().parse::<i64>().unwrap_or(0);
        let mut inner: Vec<String> = names
            .iter()
            .filter(|n| n.starts_with("In") && n.ends_with(".Cu") && n.len() >= 5)
            .cloned()
            .collect();
        inner.sort_by_key(|n| inner_index(n));
        let mut order = Vec::new();
        if names.iter().any(|n| n == "F.Cu") {
            order.push("F.Cu".to_string());
        }
        order.extend(inner);
        if names.iter().any(|n| n == "B.Cu") {
            order.push("B.Cu".to_string());
        }
        order
    }

    fn via_bridged_layers(&self, via_layers: &[String]) -> Vec<String> {
        let span: Vec<&String> = via_layers.iter().filter(|l| l.ends_with(".Cu")).collect();
        let order = self.copper_layer_order();
        let idx: Vec<usize> = span
            .iter()
            .filter_map(|l| order.iter().position(|o| o == *l))
            .collect();
        if idx.len() < 2 {
            let mut v: Vec<String> = span.into_iter().cloned().collect();
            v.sort();
            v.dedup();
            return v;
        }
        let (lo, hi) = (*idx.iter().min().unwrap(), *idx.iter().max().unwrap());
        order[lo..=hi].to_vec()
    }

    fn copper_layers_of(&self, layers: &[String]) -> HashSet<String> {
        let mut out = HashSet::new();
        for l in layers {
            if l == "*.Cu" {
                out.extend(self.copper_layer_order());
            } else if l.ends_with(".Cu") {
                out.insert(l.clone());
            }
        }
        out
    }

    fn share_copper(&self, a: &[String], b: &[String]) -> bool {
        let (x, y) = (self.copper_layers_of(a), self.copper_layers_of(b));
        x.iter().any(|l| y.contains(l))
    }

    fn find_pads_at_point(
        &self,
        p: (f64, f64),
        positions: &[(String, (f64, f64))],
        layers: Option<&HashMap<String, Vec<String>>>,
        layer: Option<&str>,
    ) -> Vec<String> {
        positions
            .iter()
            .filter(|(_, q)| points_close(p, *q))
            .filter(|(id, _)| match (layer, layers) {
                (Some(l), Some(map)) => map.get(id).is_none_or(|ls| pad_copper_on_layer(ls, l)),
                _ => true,
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn collect_layer_bridges(
        &self,
        positions: &[(String, (f64, f64))],
        layers: Option<&HashMap<String, Vec<String>>>,
    ) -> Vec<Bridge> {
        let mut out: Vec<Bridge> = Vec::new();
        for v in self.pcb.vias() {
            let set = self.via_bridged_layers(&v.layers);
            if set.len() >= 2 {
                out.push((v.position, Some(set)));
            }
        }
        if let Some(map) = layers {
            for (id, pos) in positions {
                let Some(ls) = map.get(id) else {
                    continue;
                };
                let copper: Vec<&String> = ls.iter().filter(|l| l.ends_with(".Cu")).collect();
                if copper.is_empty() {
                    continue;
                }
                let wildcard = copper.iter().any(|l| l.starts_with("*."));
                let distinct: HashSet<&&String> = copper.iter().collect();
                if wildcard || distinct.len() >= 2 {
                    out.push((
                        *pos,
                        if wildcard {
                            None
                        } else {
                            Some(copper.into_iter().cloned().collect())
                        },
                    ));
                }
            }
        }
        out
    }

    fn layers_bridged_at(p: (f64, f64), a: &str, b: &str, bridges: &[Bridge]) -> bool {
        bridges.iter().any(|(bp, set)| {
            points_close(p, *bp)
                && match set {
                    None => true,
                    Some(s) => s.iter().any(|x| x == a) && s.iter().any(|x| x == b),
                }
        })
    }

    fn segments_chain(a: &Segment, b: &Segment, bridges: &[Bridge]) -> bool {
        let same = a.layer == b.layer;
        if same && segments_copper_touch(a.start, a.end, a.width, b.start, b.end, b.width) {
            return true;
        }
        for pa in [a.start, a.end] {
            for pb in [b.start, b.end] {
                if !points_close(pa, pb) {
                    continue;
                }
                if same || Self::layers_bridged_at(pa, &a.layer, &b.layer, bridges) {
                    return true;
                }
            }
        }
        false
    }

    fn build_segment_chains(
        &self,
        segs: &[&Segment],
        positions: &[(String, (f64, f64))],
        graph: &mut Graph,
        layers: Option<&HashMap<String, Vec<String>>>,
        extra: Option<&HashMap<usize, HashSet<String>>>,
    ) {
        if segs.is_empty() {
            return;
        }
        let bridges = self.collect_layer_bridges(positions, layers);
        let bounds: Vec<sh::Bounds> = segs
            .iter()
            .map(|s| {
                let hw = s.width.max(0.0) / 2.0;
                (
                    s.start.0.min(s.end.0) - hw,
                    s.start.1.min(s.end.1) - hw,
                    s.start.0.max(s.end.0) + hw,
                    s.start.1.max(s.end.1) + hw,
                )
            })
            .collect();
        let mut adj: Vec<Vec<usize>> = vec![vec![]; segs.len()];
        for (i, j) in crate::validate::spatial::candidate_pairs(&bounds, POSITION_TOLERANCE) {
            if Self::segments_chain(segs[i], segs[j], &bridges) {
                adj[i].push(j);
                adj[j].push(i);
            }
        }
        let mut visited = vec![false; segs.len()];
        for start in 0..segs.len() {
            if visited[start] {
                continue;
            }
            let mut comp = Vec::new();
            let mut stack = vec![start];
            while let Some(i) = stack.pop() {
                if visited[i] {
                    continue;
                }
                visited[i] = true;
                comp.push(i);
                stack.extend(adj[i].iter().copied().filter(|j| !visited[*j]));
            }
            let mut pads: Vec<String> = Vec::new();
            for &i in &comp {
                let s = segs[i];
                for p in [s.start, s.end] {
                    for id in self.find_pads_at_point(p, positions, layers, Some(&s.layer)) {
                        if !pads.contains(&id) {
                            pads.push(id);
                        }
                    }
                }
                if let Some(ex) = extra.and_then(|e| e.get(&i)) {
                    for id in ex {
                        if !pads.contains(id) {
                            pads.push(id.clone());
                        }
                    }
                }
            }
            for (k, a) in pads.iter().enumerate() {
                for b in &pads[k + 1..] {
                    add_edge(graph, a, b);
                }
            }
        }
    }

    /// Board-frame pad positions / layers for non-comment, numbered pads.
    fn pad_nodes(&self, net: Option<i64>) -> PadNodes {
        let mut pos = Vec::new();
        let mut layers = HashMap::new();
        for (fi, fp) in self.pcb.footprints().iter().enumerate() {
            if fp.reference.is_empty() || fp.reference.starts_with('#') {
                continue;
            }
            for (pi, pad) in fp.pads.iter().enumerate() {
                let Some(id) = self.pad_ids.get(&(fi, pi)) else {
                    continue;
                };
                if net.is_some_and(|n| pad.net_number != n) {
                    continue;
                }
                pos.push((
                    id.clone(),
                    transform_pad_position(pad.position, fp.position, fp.rotation),
                ));
                layers.insert(id.clone(), pad.layers.clone());
            }
        }
        (pos, layers)
    }

    fn eroded_pad_polygons(&self, shape_aware: bool) -> Vec<(String, Geom)> {
        let mut out = Vec::new();
        for (fi, fp) in self.pcb.footprints().iter().enumerate() {
            if fp.reference.is_empty() || fp.reference.starts_with('#') {
                continue;
            }
            for (pi, pad) in fp.pads.iter().enumerate() {
                let Some(id) = self.pad_ids.get(&(fi, pi)) else {
                    continue;
                };
                if let Some(g) = pad_copper_polygon(fp, pad, shape_aware) {
                    match out.iter_mut().find(|(k, _): &&mut (String, Geom)| k == id) {
                        Some(e) => e.1 = g,
                        None => out.push((id.clone(), g)),
                    }
                }
            }
        }
        out
    }

    /// Physical pad partition from routed copper (label-free), with the
    /// occurrence -> `(ref, pad)` bindings.
    pub fn extract_pad_occurrences(&mut self) -> (Vec<Vec<String>>, PadBindings) {
        self.refresh_pad_identities();
        let (mut positions, mut layers) = self.pad_nodes(None);
        let mut synthetic: HashSet<String> = HashSet::new();
        for (i, v) in self.pcb.vias().iter().enumerate() {
            let id = format!("__via{i}");
            positions.push((id.clone(), v.position));
            layers.insert(id.clone(), self.via_bridged_layers(&v.layers));
            synthetic.insert(id);
        }
        let mut graph: Graph = positions
            .iter()
            .map(|(id, _)| (id.clone(), HashSet::new()))
            .collect();
        let segs: Vec<&Segment> = self.pcb.segments().iter().collect();
        for s in &segs {
            let sp = self.find_pads_at_point(s.start, &positions, Some(&layers), Some(&s.layer));
            let ep = self.find_pads_at_point(s.end, &positions, Some(&layers), Some(&s.layer));
            for a in &sp {
                for b in &ep {
                    add_edge(&mut graph, a, b);
                }
            }
            for group in [&sp, &ep] {
                for (k, a) in group.iter().enumerate() {
                    for b in &group[k + 1..] {
                        add_edge(&mut graph, a, b);
                    }
                }
            }
        }
        let mut extra: HashMap<usize, HashSet<String>> = HashMap::new();
        for (vi, v) in self.pcb.vias().iter().enumerate() {
            let node = format!("__via{vi}");
            let span = self.via_bridged_layers(&v.layers);
            let r = v.size / 2.0;
            if r <= 0.0 {
                continue;
            }
            for (si, s) in segs.iter().enumerate() {
                if !span.contains(&s.layer) {
                    continue;
                }
                let reach = r + s.width / 2.0 - 1e-3;
                if reach <= 0.0 {
                    continue;
                }
                if point_segment_distance(v.position, s.start, s.end) < reach {
                    extra.entry(si).or_default().insert(node.clone());
                    let mut hits =
                        self.find_pads_at_point(s.start, &positions, Some(&layers), Some(&s.layer));
                    hits.extend(self.find_pads_at_point(
                        s.end,
                        &positions,
                        Some(&layers),
                        Some(&s.layer),
                    ));
                    for h in hits {
                        add_edge(&mut graph, &node, &h);
                    }
                }
            }
        }
        self.connect_segment_in_pad(&segs, &layers, &mut extra);
        self.build_segment_chains(&segs, &positions, &mut graph, Some(&layers), Some(&extra));
        for v in self.pcb.vias() {
            let span = self.via_bridged_layers(&v.layers);
            let via_pads: Vec<String> = self
                .find_pads_at_point(v.position, &positions, None, None)
                .into_iter()
                .filter(|id| {
                    self.copper_layers_of(layers.get(id).map(Vec::as_slice).unwrap_or(&[]))
                        .iter()
                        .any(|l| span.contains(l))
                })
                .collect();
            for (k, a) in via_pads.iter().enumerate() {
                for b in &via_pads[k + 1..] {
                    add_edge(&mut graph, a, b);
                }
            }
        }
        self.connect_via_in_pad(&positions, &layers, &synthetic, &mut graph);
        self.connect_pour_pads_label_free(&positions, &layers, &synthetic, &mut graph, &extra);
        for (k, (a, pa)) in positions.iter().enumerate() {
            for (b, pb) in &positions[k + 1..] {
                if !points_close(*pa, *pb) {
                    continue;
                }
                let la = layers.get(a).map(Vec::as_slice).unwrap_or(&[]);
                let lb = layers.get(b).map(Vec::as_slice).unwrap_or(&[]);
                if !self.share_copper(la, lb) {
                    continue;
                }
                add_edge(&mut graph, a, b);
            }
        }
        let mut visited: HashSet<String> = HashSet::new();
        let mut partition: Vec<Vec<String>> = Vec::new();
        for (id, _) in &positions {
            if visited.contains(id) {
                continue;
            }
            let mut comp: Vec<String> = Vec::new();
            let mut stack = vec![id.clone()];
            while let Some(cur) = stack.pop() {
                if !visited.insert(cur.clone()) {
                    continue;
                }
                if let Some(nb) = graph.get(&cur) {
                    stack.extend(nb.iter().filter(|n| !visited.contains(*n)).cloned());
                }
                comp.push(cur);
            }
            let mut real: Vec<String> = comp
                .into_iter()
                .filter(|n| !synthetic.contains(n))
                .collect();
            if !real.is_empty() {
                real.sort();
                partition.push(real);
            }
        }
        partition.sort_by(|a, b| a[0].cmp(&b[0]));
        (partition, self.pad_bindings.clone())
    }

    /// Logical `REF.PAD` groups (see [`Self::extract_pad_occurrences`]).
    pub fn extract_pad_partition(&mut self) -> Vec<Vec<String>> {
        let (groups, bindings) = self.extract_pad_occurrences();
        groups
            .into_iter()
            .map(|g| {
                let mut v: Vec<String> = g
                    .iter()
                    .map(|n| {
                        bindings
                            .iter()
                            .find(|(k, _)| k == n)
                            .map(|(_, (r, p))| format!("{r}.{p}"))
                            .unwrap_or_else(|| n.clone())
                    })
                    .collect();
                v.sort();
                v.dedup();
                v
            })
            .collect()
    }

    fn connect_segment_in_pad(
        &self,
        segs: &[&Segment],
        layers: &HashMap<String, Vec<String>>,
        extra: &mut HashMap<usize, HashSet<String>>,
    ) {
        let polys = self.eroded_pad_polygons(true);
        if polys.is_empty() {
            return;
        }
        let mut bounds: Vec<(String, f64, f64, f64)> = Vec::new();
        for (fi, fp) in self.pcb.footprints().iter().enumerate() {
            if fp.reference.is_empty() || fp.reference.starts_with('#') {
                continue;
            }
            for (pi, pad) in fp.pads.iter().enumerate() {
                let Some(id) = self.pad_ids.get(&(fi, pi)) else {
                    continue;
                };
                if !polys.iter().any(|(k, _)| k == id) {
                    continue;
                }
                let (cx, cy) = transform_pad_position(pad.position, fp.position, fp.rotation);
                bounds.push((
                    id.clone(),
                    cx,
                    cy,
                    crate::utils::pymath::hypot(pad.size.0, pad.size.1) / 2.0,
                ));
            }
        }
        for (si, s) in segs.iter().enumerate() {
            let cap = (s.width / 2.0).max(0.0);
            let centerline = if s.start == s.end {
                Geom::Point(s.start)
            } else {
                Geom::Line(vec![s.start, s.end])
            };
            for (id, cx, cy, bound) in &bounds {
                if point_segment_distance((*cx, *cy), s.start, s.end) > bound + cap {
                    continue;
                }
                if !pad_copper_on_layer(layers.get(id).map(Vec::as_slice).unwrap_or(&[]), &s.layer)
                {
                    continue;
                }
                let poly = &polys.iter().rev().find(|(k, _)| k == id).unwrap().1;
                let d = sh::distance(poly, &centerline);
                if d == 0.0 || d < cap - 1e-3 {
                    extra.entry(si).or_default().insert(id.clone());
                }
            }
        }
    }

    fn connect_via_in_pad(
        &self,
        positions: &[(String, (f64, f64))],
        layers: &HashMap<String, Vec<String>>,
        synthetic: &HashSet<String>,
        graph: &mut Graph,
    ) {
        let mut pads = self.eroded_pad_polygons(false);
        let mut raw: HashSet<String> = HashSet::new();
        for (fi, fp) in self.pcb.footprints().iter().enumerate() {
            if fp.reference.is_empty() || fp.reference.starts_with('#') {
                continue;
            }
            for (pi, pad) in fp.pads.iter().enumerate() {
                if pad.number.is_empty() {
                    continue;
                }
                if !["rect", "roundrect", "circle", "oval", "obround"].contains(&pad.shape.as_str())
                {
                    continue;
                }
                let key = &self.pad_ids[&(fi, pi)];
                let Some((_, center)) = positions.iter().find(|(k, _)| k == key) else {
                    continue;
                };
                if let Some(mut g) = crate::validate::rules::clearance::pad_polygon(pad, fp) {
                    if pad.drill > 0.0 {
                        g = sh::difference(&g, &sh::point_buffer_q(*center, pad.drill / 2.0, 64));
                    }
                    match pads.iter_mut().find(|(k, _)| k == key) {
                        Some(e) => e.1 = g,
                        None => pads.push((key.clone(), g)),
                    }
                    raw.insert(key.clone());
                }
            }
        }
        for (i, v) in self.pcb.vias().iter().enumerate() {
            let node = format!("__via{i}");
            if !synthetic.contains(&node) {
                continue;
            }
            let ann = physical_via_annulus(v);
            for (key, g) in &pads {
                let lk = layers.get(key).map(Vec::as_slice).unwrap_or(&[]);
                let ln = layers.get(&node).map(Vec::as_slice).unwrap_or(&[]);
                if !self.share_copper(lk, ln) {
                    continue;
                }
                let touches = if raw.contains(key) {
                    sh::intersects(&ann, g) && sh::intersection(&ann, g).area() > 0.0
                } else {
                    sh::intersects(g, &Geom::Point(v.position))
                };
                if touches {
                    add_edge(graph, &node, key);
                }
            }
        }
    }

    fn connect_pour_pads_label_free(
        &self,
        positions: &[(String, (f64, f64))],
        layers: &HashMap<String, Vec<String>>,
        synthetic: &HashSet<String>,
        graph: &mut Graph,
        extra: &HashMap<usize, HashSet<String>>,
    ) {
        // (key, kind, layers, geometry)
        let mut items: Vec<(Option<String>, &'static str, Vec<String>, Geom)> = Vec::new();
        let mut terminal: Vec<(String, usize)> = Vec::new();
        for (key, g) in self.eroded_pad_polygons(true) {
            if positions.iter().any(|(k, _)| *k == key) {
                terminal.push((key.clone(), items.len()));
                let ls: Vec<String> = self.copper_layers_of(&layers[&key]).into_iter().collect();
                items.push((Some(key), "pad", ls, g));
            }
        }
        for (i, v) in self.pcb.vias().iter().enumerate() {
            let key = format!("__via{i}");
            if synthetic.contains(&key) {
                terminal.push((key.clone(), items.len()));
                items.push((
                    Some(key),
                    "via",
                    self.via_bridged_layers(&v.layers),
                    physical_via_annulus(v),
                ));
            }
        }
        let mut seg_index: HashMap<usize, usize> = HashMap::new();
        for (i, s) in self.pcb.segments().iter().enumerate() {
            let g = segment_copper_geom(s.start, s.end, s.width);
            seg_index.insert(i, items.len());
            items.push((None, "segment", vec![s.layer.clone()], g));
        }
        let mut zone_fill_items: Vec<(f64, Vec<usize>, Vec<String>)> = Vec::new();
        let mut fill_zone: HashMap<usize, usize> = HashMap::new();
        for (zi, z) in self.pcb.zones().iter().enumerate() {
            let mut zitems = Vec::new();
            let mut zlayers = Vec::new();
            for (fi, pts) in z.filled_polygons.iter().enumerate() {
                let Some(region) = fill_solid_region(pts) else {
                    continue;
                };
                let layer = z.filled_polygon_layer(fi).to_string();
                for solid in region.polys() {
                    let idx = items.len();
                    zitems.push(idx);
                    zlayers.push(layer.clone());
                    fill_zone.insert(idx, zi);
                    items.push((None, "fill", vec![layer.clone()], Geom::Poly(solid.clone())));
                }
            }
            if !zitems.is_empty() {
                zone_fill_items.push((z.fill_inflation(), zitems, zlayers));
            }
        }
        if items.is_empty() {
            return;
        }
        let mut parent: Vec<usize> = (0..items.len()).collect();
        fn find(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        fn union(p: &mut [usize], a: usize, b: usize) {
            let (ra, rb) = (find(p, a), find(p, b));
            p[rb] = ra;
        }
        let tindex = |k: &str| terminal.iter().find(|(t, _)| t == k).map(|(_, i)| *i);
        for (key, nbrs) in graph.iter() {
            if let Some(a) = tindex(key) {
                for n in nbrs {
                    if let Some(b) = tindex(n) {
                        union(&mut parent, a, b);
                    }
                }
            }
        }
        for (i, s) in self.pcb.segments().iter().enumerate() {
            let Some(&si) = seg_index.get(&i) else {
                continue;
            };
            let mut terms: HashSet<String> = extra.get(&i).cloned().unwrap_or_default();
            for p in [s.start, s.end] {
                terms.extend(self.find_pads_at_point(p, positions, Some(layers), Some(&s.layer)));
            }
            for t in terms {
                if let Some(ti) = tindex(&t) {
                    union(&mut parent, si, ti);
                }
            }
        }
        let tree = StrTree::new(&items.iter().map(|it| it.3.bounds()).collect::<Vec<_>>());
        for left in 0..items.len() {
            let Some(b) = items[left].3.bounds() else {
                continue;
            };
            for right in tree.query(b) {
                if right <= left {
                    continue;
                }
                let (kl, kr) = (items[left].1, items[right].1);
                if !items[left].2.iter().any(|l| items[right].2.contains(l)) {
                    continue;
                }
                if kl != "fill" && kr != "fill" && (kl, kr) != ("segment", "segment") {
                    continue;
                }
                if kl == "fill" && kr == "fill" && fill_zone[&left] == fill_zone[&right] {
                    continue;
                }
                if !sh::intersects(&items[left].3, &items[right].3) {
                    continue;
                }
                union(&mut parent, left, right);
            }
        }
        for (inflation, zitems, zlayers) in &zone_fill_items {
            if *inflation <= 0.0 || zitems.len() < 2 {
                continue;
            }
            let geoms: Vec<&Geom> = zitems.iter().map(|i| &items[*i].3).collect();
            for (a, b) in adjacent_fill_pairs(&geoms, zlayers, 2.0 * inflation) {
                union(&mut parent, zitems[a], zitems[b]);
            }
        }
        let fill_roots: HashSet<usize> = (0..items.len())
            .filter(|i| items[*i].1 == "fill")
            .map(|i| find(&mut parent, i))
            .collect();
        let mut comps: Vec<(usize, Vec<String>)> = Vec::new();
        for (key, idx) in &terminal {
            let r = find(&mut parent, *idx);
            if fill_roots.contains(&r) {
                match comps.iter_mut().find(|(k, _)| *k == r) {
                    Some(e) => e.1.push(key.clone()),
                    None => comps.push((r, vec![key.clone()])),
                }
            }
        }
        for (_, keys) in comps {
            for k in &keys[1..] {
                add_edge(graph, &keys[0], k);
            }
        }
    }

    fn get_net_pads(&self, net: i64) -> Vec<String> {
        let mut out = Vec::new();
        for (fi, fp) in self.pcb.footprints().iter().enumerate() {
            if fp.reference.is_empty() || fp.reference.starts_with('#') {
                continue;
            }
            for (pi, pad) in fp.pads.iter().enumerate() {
                if pad.net_number == net {
                    if let Some(id) = self.pad_ids.get(&(fi, pi)) {
                        out.push(id.clone());
                    }
                }
            }
        }
        out.sort();
        out
    }

    fn build_connectivity_graph(&mut self, net: i64) -> Graph {
        let mut graph: Graph = HashMap::new();
        let (positions, layers) = self.pad_nodes(Some(net));
        let segs: Vec<&Segment> = self.pcb.segments_in_net(net).collect();
        let vias: Vec<&crate::schema::pcb::Via> = self.pcb.vias_in_net(net).collect();
        let mut copper_pts: Vec<(f64, f64)> = Vec::new();
        for s in &segs {
            copper_pts.push(s.start);
            copper_pts.push(s.end);
        }
        copper_pts.extend(vias.iter().map(|v| v.position));
        for z in self.pcb.zones() {
            if z.net_number == net && !z.filled_polygons.is_empty() {
                for poly in &z.filled_polygons {
                    copper_pts.extend(poly.iter().copied());
                }
            }
        }
        for (id, pos) in &positions {
            for cp in &copper_pts {
                if !points_close(*pos, *cp) {
                    continue;
                }
                for (other, opos) in &positions {
                    if other != id
                        && points_close(*pos, *opos)
                        && self.share_copper(&layers[id], &layers[other])
                    {
                        add_edge(&mut graph, id, other);
                    }
                }
            }
        }
        for s in &segs {
            let sp = self.find_pads_at_point(s.start, &positions, Some(&layers), Some(&s.layer));
            let ep = self.find_pads_at_point(s.end, &positions, Some(&layers), Some(&s.layer));
            for a in &sp {
                for b in &ep {
                    add_edge(&mut graph, a, b);
                }
            }
            for group in [&sp, &ep] {
                for a in group.iter() {
                    for b in group.iter() {
                        add_edge(&mut graph, a, b);
                    }
                }
            }
        }
        for v in &vias {
            let vp = self.find_pads_at_point(v.position, &positions, None, None);
            for a in &vp {
                for b in &vp {
                    add_edge(&mut graph, a, b);
                }
            }
        }
        self.build_segment_chains(&segs, &positions, &mut graph, Some(&layers), None);
        let mut zone_pads: HashSet<String> = HashSet::new();
        for z in self.pcb.zones() {
            if z.net_number != net || z.filled_polygons.is_empty() || z.polygon.len() < 3 {
                continue;
            }
            let mut in_zone: Vec<String> = Vec::new();
            for (id, pos) in &positions {
                if !pad_layer_matches_zone(
                    layers.get(id).map(Vec::as_slice).unwrap_or(&[]),
                    &z.layer,
                ) {
                    continue;
                }
                if point_in_polygon(*pos, &z.polygon) {
                    in_zone.push(id.clone());
                    zone_pads.insert(id.clone());
                }
            }
            for v in &vias {
                if v.layers.contains(&z.layer) && point_in_polygon(v.position, &z.polygon) {
                    let vp = self.find_pads_at_point(v.position, &positions, None, None);
                    zone_pads.extend(vp.iter().cloned());
                    in_zone.extend(vp);
                }
            }
            for (k, a) in in_zone.iter().enumerate() {
                for b in &in_zone[k + 1..] {
                    add_edge(&mut graph, a, b);
                }
            }
        }
        self.last_zone_connected_pads = zone_pads;
        graph
    }

    fn find_islands(graph: &Graph, pads: &[String]) -> Vec<Vec<String>> {
        let pad_set: HashSet<&String> = pads.iter().collect();
        let mut visited: HashSet<String> = HashSet::new();
        let mut islands = Vec::new();
        for p in pads {
            if visited.contains(p) {
                continue;
            }
            let mut island = Vec::new();
            let mut queue = std::collections::VecDeque::from([p.clone()]);
            while let Some(cur) = queue.pop_front() {
                if !visited.insert(cur.clone()) {
                    continue;
                }
                if pad_set.contains(&cur) {
                    island.push(cur.clone());
                }
                if let Some(nb) = graph.get(&cur) {
                    queue.extend(nb.iter().filter(|n| !visited.contains(*n)).cloned());
                }
            }
            if !island.is_empty() {
                island.sort();
                islands.push(island);
            }
        }
        islands
    }

    fn create_issue(&self, net_name: &str, islands: &[Vec<String>]) -> ConnectivityIssue {
        let mut islands: Vec<Vec<String>> = islands
            .iter()
            .map(|i| i.iter().map(|n| self.pad_display(n)).collect())
            .collect();
        islands.sort_by_key(|i| std::cmp::Reverse(i.len()));
        let connected = islands.first().cloned().unwrap_or_default();
        let unconnected: Vec<String> = islands.iter().skip(1).flatten().cloned().collect();
        let (message, suggestion) = if islands.len() == 2 {
            (
                format!("Net '{net_name}' has 2 disconnected islands"),
                format!(
                    "Connect islands (missing trace between {} and {})",
                    islands[0].last().cloned().unwrap_or_default(),
                    islands[1][0]
                ),
            )
        } else {
            (
                format!(
                    "Net '{net_name}' has {} disconnected islands",
                    islands.len()
                ),
                format!("Connect {} islands to complete routing", islands.len()),
            )
        };
        ConnectivityIssue {
            severity: "error".into(),
            issue_type: "partial".into(),
            net_name: net_name.to_string(),
            message,
            suggestion,
            connected_pads: connected,
            unconnected_pads: unconnected,
            islands,
        }
    }

    fn unconnected_item_relationships(&self) -> Vec<(i64, Vec<ConnectivityIssue>)> {
        let mut modeled: PySetOrder<i64> = PySetOrder::new();
        for z in self.pcb.zones() {
            if z.net_number != 0 && !z.filled_polygons.is_empty() {
                modeled.add(hash_int(z.net_number), z.net_number);
            }
        }
        let copper_layers: Vec<String> = {
            let v: Vec<String> = self
                .pcb
                .copper_layers()
                .iter()
                .map(|l| l.name.clone())
                .collect();
            if v.is_empty() {
                vec!["F.Cu".into(), "B.Cu".into()]
            } else {
                v
            }
        };
        let mut out = Vec::new();
        for &net in modeled.iter() {
            // (id, desc, {layer: geom}, terminal)
            let mut items: Vec<RelationItem> = Vec::new();
            let mut pad_records: Vec<(usize, Vec<String>, Geom)> = Vec::new();
            let mut zone_records: Vec<(Geom, String, Vec<usize>)> = Vec::new();
            for fp in self.pcb.footprints() {
                for pad in &fp.pads {
                    if pad.net_number != net {
                        continue;
                    }
                    let Some(g) = pad_copper_polygon(fp, pad, false) else {
                        continue;
                    };
                    let ls: Vec<(String, Geom)> = copper_layers
                        .iter()
                        .filter(|l| pad_layer_matches_zone(&pad.layers, l))
                        .map(|l| (l.clone(), g.clone()))
                        .collect();
                    let id = format!("{}.{}", fp.reference, pad.number);
                    items.push((format!("pad:{id}"), format!("Pad {id}"), ls, true));
                    pad_records.push((items.len() - 1, pad.layers.clone(), g));
                }
            }
            for (vi, v) in self.pcb.vias().iter().enumerate() {
                if v.net_number != net {
                    continue;
                }
                let g = via_copper_geom(v.position, v.size.max(0.0) / 2.0);
                let ls = self
                    .via_bridged_layers(&v.layers)
                    .into_iter()
                    .map(|l| (l, g.clone()))
                    .collect();
                items.push((format!("via:{vi}"), format!("Via {vi}"), ls, false));
            }
            for (si, s) in self.pcb.segments().iter().enumerate() {
                if s.net_number != net {
                    continue;
                }
                let g = segment_copper_geom(s.start, s.end, s.width);
                items.push((
                    format!("segment:{si}"),
                    format!("Segment {si}"),
                    vec![(s.layer.clone(), g)],
                    false,
                ));
            }
            for (zi, z) in self.pcb.zones().iter().enumerate() {
                if z.net_number != net {
                    continue;
                }
                let mut fills = Vec::new();
                for (fi, pts) in z.filled_polygons.iter().enumerate() {
                    let Some(region) = fill_solid_region(pts) else {
                        continue;
                    };
                    let layer = z.filled_polygon_layer(fi).to_string();
                    let id = format!("zone[{zi}].fill[{fi}]@{layer}");
                    items.push((
                        id.clone(),
                        format!("Zone island {id}"),
                        vec![(layer, region)],
                        true,
                    ));
                    fills.push(items.len() - 1);
                }
                if let Some(boundary) = fill_solid_region(&z.polygon) {
                    if !fills.is_empty() {
                        zone_records.push((boundary, z.layer.clone(), fills));
                    }
                }
            }
            let mut parent: Vec<usize> = (0..items.len()).collect();
            fn find(p: &mut [usize], mut i: usize) -> usize {
                while p[i] != i {
                    p[i] = p[p[i]];
                    i = p[i];
                }
                i
            }
            fn union(p: &mut [usize], a: usize, b: usize) {
                let (ra, rb) = (find(p, a), find(p, b));
                if ra != rb {
                    p[rb] = ra;
                }
            }
            let geom_on = |it: &(String, String, Vec<(String, Geom)>, bool), layer: &str| {
                it.2.iter()
                    .rev()
                    .find(|(l, _)| l == layer)
                    .map(|(_, g)| g.clone())
            };
            let item_bounds: Vec<Option<sh::Bounds>> = items
                .iter()
                .map(|it| {
                    it.2.iter()
                        .filter_map(|(_, g)| g.bounds())
                        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
                })
                .collect();
            for l in 0..items.len() {
                for r in l + 1..items.len() {
                    let (Some(a), Some(b)) = (item_bounds[l], item_bounds[r]) else {
                        continue;
                    };
                    if a.0 > b.2 || b.0 > a.2 || a.1 > b.3 || b.1 > a.3 {
                        continue;
                    }
                    let touch = items[l].2.iter().any(|(layer, g)| {
                        geom_on(&items[r], layer).is_some_and(|h| sh::intersects(g, &h))
                    });
                    if touch {
                        union(&mut parent, l, r);
                    }
                }
            }
            for (boundary, layer, fills) in &zone_records {
                let layer_fills: Vec<usize> = fills
                    .iter()
                    .copied()
                    .filter(|i| geom_on(&items[*i], layer).is_some())
                    .collect();
                if layer_fills.is_empty() {
                    continue;
                }
                for (pi, pls, pg) in &pad_records {
                    if !pad_layer_matches_zone(pls, layer) || !sh::intersects(boundary, pg) {
                        continue;
                    }
                    let mut best = layer_fills[0];
                    let mut bd = f64::INFINITY;
                    for &fi in &layer_fills {
                        let d = sh::distance(pg, &geom_on(&items[fi], layer).unwrap());
                        if d < bd {
                            bd = d;
                            best = fi;
                        }
                    }
                    union(&mut parent, *pi, best);
                }
            }
            let mut comps: Vec<(usize, Vec<usize>)> = Vec::new();
            for i in 0..items.len() {
                let r = find(&mut parent, i);
                match comps.iter_mut().find(|(k, _)| *k == r) {
                    Some(e) => e.1.push(i),
                    None => comps.push((r, vec![i])),
                }
            }
            let mut terminal: Vec<Vec<usize>> = comps
                .into_iter()
                .map(|(_, c)| c)
                .filter(|c| c.iter().any(|i| items[*i].3))
                .collect();
            terminal.sort_by(|a, b| {
                let ma = a.iter().map(|i| &items[*i].0).min().unwrap();
                let mb = b.iter().map(|i| &items[*i].0).min().unwrap();
                ma.cmp(mb)
            });
            let net_name = self
                .pcb
                .get_net(net)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| format!("net-{net}"));
            let mut issues = Vec::new();
            if let Some(anchor) = terminal.first() {
                let a = *anchor.iter().find(|i| items[**i].3).unwrap();
                for comp in &terminal[1..] {
                    let b = *comp.iter().find(|i| items[**i].3).unwrap();
                    issues.push(ConnectivityIssue {
                        severity: "error".into(),
                        issue_type: "zone_island".into(),
                        net_name: net_name.clone(),
                        message: format!(
                            "Missing connection between {} and {}",
                            items[a].1, items[b].1
                        ),
                        suggestion: "Connect the same-net copper components".into(),
                        connected_pads: vec![],
                        unconnected_pads: vec![],
                        islands: vec![vec![items[a].0.clone()], vec![items[b].0.clone()]],
                    });
                }
            }
            out.push((net, issues));
        }
        out
    }

    /// KiCad's per-item relationships from `kicad-cli pcb drc` (opt-in);
    /// `None` when unavailable.
    fn native_unconnected_relationships(
        &self,
        internal_types: &[(String, String)],
        internal_issue_count: usize,
    ) -> Option<Vec<ConnectivityIssue>> {
        let path = self.pcb_path.as_ref()?;
        let cli = crate::cli::runner::find_kicad_cli()?;
        let report = std::env::temp_dir().join(format!(
            "kct-native-conn-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let status = std::process::Command::new(cli)
            .args(["pcb", "drc", "--refill-zones", "--format", "json", "-o"])
            .arg(&report)
            .arg(path)
            .output()
            .ok()?;
        let code = status.status.code().unwrap_or(-1);
        if code != 0 && code != 5 {
            let _ = std::fs::remove_file(&report);
            eprintln!(
                "RuntimeWarning: kicad-cli pcb drc exited with unexpected code {code} during \
                 native connectivity reconciliation; substituting the internal analysis \
                 ({internal_issue_count} issue(s))"
            );
            return None;
        }
        let parsed = crate::drc::report::DRCReport::load(&report).ok();
        let _ = std::fs::remove_file(&report);
        let parsed = parsed?;
        let mut issues = Vec::new();
        for (i, v) in parsed.connectivity_items().into_iter().enumerate() {
            let ids: Vec<String> = if v.items.is_empty() {
                vec![format!("native-unconnected[{i}]")]
            } else {
                v.items.clone()
            };
            let net_name = v.nets.first().cloned().unwrap_or_else(|| "<native>".into());
            let pads: Vec<String> = v
                .items
                .iter()
                .filter(|x| !x.starts_with("Zone "))
                .cloned()
                .collect();
            let issue_type = if pads.is_empty() {
                "zone_island".to_string()
            } else {
                internal_types
                    .iter()
                    .rev()
                    .find(|(n, _)| *n == net_name)
                    .map(|(_, t)| t.clone())
                    .unwrap_or_else(|| "zone_island".into())
            };
            let mut message = v.message.clone();
            if message == "Missing connection between items" && ids.len() >= 2 {
                message = format!("Missing connection between {} and {}", ids[0], ids[1]);
            }
            issues.push(ConnectivityIssue {
                severity: "error".into(),
                issue_type,
                net_name,
                message,
                suggestion: "Connect the items reported by kicad-cli".into(),
                connected_pads: vec![],
                unconnected_pads: pads,
                islands: ids.iter().map(|x| vec![x.clone()]).collect(),
            });
        }
        Some(issues)
    }

    /// `validate(reconcile_native=...)`.
    pub fn validate(&mut self, reconcile_native: bool) -> ConnectivityResult {
        self.refresh_pad_identities();
        let mut result = ConnectivityResult::default();
        let rel = self.unconnected_item_relationships();
        for (_, issues) in &rel {
            result.issues.extend(issues.iter().cloned());
        }
        let nets: Vec<(i64, String)> = {
            let mut v: Vec<(i64, String)> = Vec::new();
            for n in self.pcb.nets() {
                if n.number == 0 || n.name.is_empty() {
                    continue;
                }
                match v.iter_mut().find(|(k, _)| *k == n.number) {
                    Some(e) => e.1 = n.name.clone(),
                    None => v.push((n.number, n.name.clone())),
                }
            }
            v
        };
        result.total_nets = nets.len();
        let has_fps = !self.pcb.footprints().is_empty();
        let (mut connected, mut zone_connected) = (0, 0);
        for (num, name) in &nets {
            if let Some((_, issues)) = rel.iter().find(|(k, _)| k == num) {
                if issues.is_empty() {
                    connected += 1;
                    zone_connected += 1;
                }
                continue;
            }
            let pads = self.get_net_pads(*num);
            if pads.is_empty() && has_fps {
                continue;
            }
            if pads.len() < 2 {
                connected += 1;
                continue;
            }
            self.last_zone_connected_pads.clear();
            let graph = self.build_connectivity_graph(*num);
            let islands = Self::find_islands(&graph, &pads);
            if islands.len() <= 1 {
                connected += 1;
                if !self.last_zone_connected_pads.is_empty() {
                    zone_connected += 1;
                }
                continue;
            }
            result.issues.push(self.create_issue(name, &islands));
        }
        result.connected_nets = connected;
        result.zone_connected_nets = zone_connected;
        if reconcile_native {
            let mut types: Vec<(String, String)> = Vec::new();
            for i in &result.issues {
                if ["unrouted", "partial", "isolated"].contains(&i.issue_type.as_str()) {
                    types.push((i.net_name.clone(), i.issue_type.clone()));
                }
            }
            if let Some(native) = self.native_unconnected_relationships(&types, result.issues.len())
            {
                result.issues = native;
            }
        }
        result
    }
}

/// `_adjacent_fill_pairs`: same-layer fill pairs within `reach`.
fn adjacent_fill_pairs(regions: &[&Geom], layers: &[String], reach: f64) -> Vec<(usize, usize)> {
    if regions.len() < 2 {
        return vec![];
    }
    let tree = StrTree::new(&regions.iter().map(|g| g.bounds()).collect::<Vec<_>>());
    let mut out = Vec::new();
    for (i, g) in regions.iter().enumerate() {
        let Some(b) = g.bounds() else {
            continue;
        };
        for j in tree.query((b.0 - reach, b.1 - reach, b.2 + reach, b.3 + reach)) {
            if j <= i || sh::distance(g, regions[j]) > reach {
                continue;
            }
            if layers[i] == layers[j] {
                out.push((i, j));
            }
        }
    }
    out
}
