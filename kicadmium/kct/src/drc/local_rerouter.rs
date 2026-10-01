//! Port of `kicad_tools.drc.local_rerouter`: rip up one segment and A*-route
//! around an obstacle on a small scratch grid (0.05 mm cells).
//!
//! Self-contained (no router core needed). The open set reproduces Python's
//! `heapq` exactly so tie-breaking, and therefore the chosen path, matches.

use std::collections::{HashMap, HashSet};

use super::net_compat::resolve_net_atom;
use super::repair_silkscreen::{descendant_paths, node_at, NodePath};
use crate::pyjson::py_round;
use crate::sexp::SExp;

/// Result of one reroute attempt.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RerouteResult {
    pub success: bool,
    pub new_segments: usize,
    pub path_length_mm: f64,
}

/// `(x, y)` atoms of a node's numeric children.
pub(crate) fn xy_of(node: Option<&SExp>) -> Option<(f64, f64)> {
    let n = node?;
    let a: Vec<f64> = n.atoms().map(|v| v.as_f64().unwrap_or(0.0)).collect();
    Some((a.first().copied().unwrap_or(0.0), a.get(1).copied().unwrap_or(0.0)))
}

pub(crate) fn first_f(node: Option<&SExp>, default: f64) -> f64 {
    node.and_then(|n| n.first_atom())
        .and_then(|v| v.as_f64())
        .unwrap_or(default)
}

pub(crate) fn first_text(node: Option<&SExp>) -> Option<String> {
    node.and_then(|n| n.text_at(0))
}

#[derive(Clone)]
struct ANode {
    f: f64,
    g: f64,
    x: i64,
    y: i64,
    parent: Option<usize>,
}

/// Python `heapq` over `ANode` ordered by `f` only.
struct PyHeap {
    items: Vec<usize>,
}

impl PyHeap {
    fn push(&mut self, nodes: &[ANode], item: usize) {
        self.items.push(item);
        let n = self.items.len() - 1;
        self.sift_down(nodes, 0, n);
    }

    fn pop(&mut self, nodes: &[ANode]) -> Option<usize> {
        let last = self.items.pop()?;
        if self.items.is_empty() {
            return Some(last);
        }
        let ret = std::mem::replace(&mut self.items[0], last);
        self.sift_up(nodes, 0);
        Some(ret)
    }

    fn sift_down(&mut self, nodes: &[ANode], start: usize, mut pos: usize) {
        let newitem = self.items[pos];
        while pos > start {
            let parent = (pos - 1) >> 1;
            let p = self.items[parent];
            if nodes[newitem].f < nodes[p].f {
                self.items[pos] = p;
                pos = parent;
                continue;
            }
            break;
        }
        self.items[pos] = newitem;
    }

    fn sift_up(&mut self, nodes: &[ANode], mut pos: usize) {
        let end = self.items.len();
        let start = pos;
        let newitem = self.items[pos];
        let mut child = 2 * pos + 1;
        while child < end {
            let right = child + 1;
            if right < end && nodes[self.items[child]].f.partial_cmp(&nodes[self.items[right]].f)
                != Some(std::cmp::Ordering::Less) {
                child = right;
            }
            self.items[pos] = self.items[child];
            pos = child;
            child = 2 * pos + 1;
        }
        self.items[pos] = newitem;
        self.sift_down(nodes, start, pos);
    }
}

/// Local A* rerouter over a board's raw tree.
pub struct LocalRerouter {
    pub nets: HashMap<i64, String>,
    pub net_names: HashMap<String, i64>,
    pub resolution: f64,
    pub padding: f64,
}

impl LocalRerouter {
    pub fn new(nets: &HashMap<i64, String>, resolution: f64, padding: f64) -> Self {
        LocalRerouter {
            nets: nets.clone(),
            net_names: nets.iter().map(|(k, v)| (v.clone(), *k)).collect(),
            resolution,
            padding,
        }
    }

    fn net_of(&self, node: &SExp) -> i64 {
        let atom = first_text(node.find("net"));
        resolve_net_atom(atom.as_deref(), Some(&self.nets), Some(&self.net_names)).0
    }

    /// Reroute the segment at `seg_path` (a direct child of `doc`) around
    /// the obstacle; replaces it unless `dry_run`.
    #[allow(clippy::too_many_arguments)]
    pub fn reroute_segment(
        &self,
        doc: &mut SExp,
        seg_path: &NodePath,
        obstacle_x: f64,
        obstacle_y: f64,
        obstacle_radius: f64,
        trace_width: f64,
        trace_clearance: f64,
        dry_run: bool,
        extra_obstacles: &[(f64, f64, f64)],
        same_net_obstacle_segs: &[NodePath],
    ) -> RerouteResult {
        let seg = node_at(doc, seg_path);
        let (Some((sx, sy)), Some((ex, ey))) = (xy_of(seg.find("start")), xy_of(seg.find("end")))
        else {
            return RerouteResult::default();
        };
        let seg_layer = first_text(seg.find("layer")).unwrap_or_else(|| "F.Cu".into());
        let seg_net = self.net_of(seg);
        let seg_width = first_f(seg.find("width"), trace_width);
        let p = self.padding;
        let mut min_x = sx.min(ex) - p;
        let mut min_y = sy.min(ey) - p;
        let mut max_x = sx.max(ex) + p;
        let mut max_y = sy.max(ey) + p;
        min_x = min_x.min(obstacle_x - obstacle_radius - p);
        min_y = min_y.min(obstacle_y - obstacle_radius - p);
        max_x = max_x.max(obstacle_x + obstacle_radius + p);
        max_y = max_y.max(obstacle_y + obstacle_radius + p);
        for &(ox, oy, or) in extra_obstacles {
            min_x = min_x.min(ox - or - p);
            min_y = min_y.min(oy - or - p);
            max_x = max_x.max(ox + or + p);
            max_y = max_y.max(oy + or + p);
        }
        let cols = ((max_x - min_x) / self.resolution) as i64 + 1;
        let rows = ((max_y - min_y) / self.resolution) as i64 + 1;
        let mut blocked: HashSet<(i64, i64)> = HashSet::new();
        let block_radius = obstacle_radius + seg_width / 2.0 + trace_clearance;
        self.mark_circle(&mut blocked, obstacle_x, obstacle_y, block_radius, min_x, min_y, cols, rows);
        for &(ox, oy, or) in extra_obstacles {
            let r = or + seg_width / 2.0 + trace_clearance;
            self.mark_circle(&mut blocked, ox, oy, r, min_x, min_y, cols, rows);
        }
        self.mark_local_obstacles(
            doc,
            &mut blocked,
            (min_x, min_y, max_x, max_y),
            cols,
            rows,
            &seg_layer,
            seg_net,
            seg_path,
            seg_width,
            trace_clearance,
            same_net_obstacle_segs,
        );
        let (sgx, sgy) = self.world_to_grid(sx, sy, min_x, min_y, cols, rows);
        let (egx, egy) = self.world_to_grid(ex, ey, min_x, min_y, cols, rows);
        blocked.remove(&(sgx, sgy));
        blocked.remove(&(egx, egy));
        let Some(path) = astar(sgx, sgy, egx, egy, &blocked, cols, rows) else {
            return RerouteResult::default();
        };
        let world: Vec<(f64, f64)> = path
            .iter()
            .map(|&(gx, gy)| {
                (
                    py_round(min_x + gx as f64 * self.resolution, 4),
                    py_round(min_y + gy as f64 * self.resolution, 4),
                )
            })
            .collect();
        let world = simplify_path(world);
        if world.len() < 2 {
            return RerouteResult::default();
        }
        let mut total = 0.0;
        for w in world.windows(2) {
            let dx = w[1].0 - w[0].0;
            let dy = w[1].1 - w[0].1;
            total += (dx * dx + dy * dy).sqrt();
        }
        if !dry_run {
            replace_segment(doc, seg_path, &world, &seg_layer, seg_net, seg_width);
        }
        RerouteResult {
            success: true,
            new_segments: world.len() - 1,
            path_length_mm: py_round(total, 4),
        }
    }

    fn world_to_grid(&self, x: f64, y: f64, ox: f64, oy: f64, cols: i64, rows: i64) -> (i64, i64) {
        let gx = ((x - ox) / self.resolution).round_ties_even() as i64;
        let gy = ((y - oy) / self.resolution).round_ties_even() as i64;
        (gx.min(cols - 1).max(0), gy.min(rows - 1).max(0))
    }

    #[allow(clippy::too_many_arguments)]
    fn mark_circle(
        &self,
        blocked: &mut HashSet<(i64, i64)>,
        cx: f64,
        cy: f64,
        radius: f64,
        ox: f64,
        oy: f64,
        cols: i64,
        rows: i64,
    ) {
        let r = self.resolution;
        let gx_min = 0.max(((cx - radius - ox) / r) as i64);
        let gx_max = (cols - 1).min(((cx + radius - ox) / r) as i64 + 1);
        let gy_min = 0.max(((cy - radius - oy) / r) as i64);
        let gy_max = (rows - 1).min(((cy + radius - oy) / r) as i64 + 1);
        let rsq = radius * radius;
        for gy in gy_min..=gy_max {
            let wy = oy + gy as f64 * r;
            for gx in gx_min..=gx_max {
                let wx = ox + gx as f64 * r;
                if (wx - cx).powi(2) + (wy - cy).powi(2) <= rsq {
                    blocked.insert((gx, gy));
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn mark_segment(
        &self,
        blocked: &mut HashSet<(i64, i64)>,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        half_width: f64,
        ox: f64,
        oy: f64,
        cols: i64,
        rows: i64,
    ) {
        let r = self.resolution;
        let gx_min = 0.max(((x1.min(x2) - half_width - ox) / r) as i64);
        let gx_max = (cols - 1).min(((x1.max(x2) + half_width - ox) / r) as i64 + 1);
        let gy_min = 0.max(((y1.min(y2) - half_width - oy) / r) as i64);
        let gy_max = (rows - 1).min(((y1.max(y2) + half_width - oy) / r) as i64 + 1);
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len_sq = dx * dx + dy * dy;
        let hw_sq = half_width * half_width;
        for gy in gy_min..=gy_max {
            let wy = oy + gy as f64 * r;
            for gx in gx_min..=gx_max {
                let wx = ox + gx as f64 * r;
                let d = if len_sq < 1e-10 {
                    (wx - x1).powi(2) + (wy - y1).powi(2)
                } else {
                    let t = (((wx - x1) * dx + (wy - y1) * dy) / len_sq).clamp(0.0, 1.0);
                    (wx - (x1 + t * dx)).powi(2) + (wy - (y1 + t * dy)).powi(2)
                };
                if d <= hw_sq {
                    blocked.insert((gx, gy));
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn mark_local_obstacles(
        &self,
        doc: &SExp,
        blocked: &mut HashSet<(i64, i64)>,
        bbox: (f64, f64, f64, f64),
        cols: i64,
        rows: i64,
        layer: &str,
        net: i64,
        exclude: &NodePath,
        trace_width: f64,
        trace_clearance: f64,
        same_net_obs: &[NodePath],
    ) {
        let (min_x, min_y, max_x, max_y) = bbox;
        let our_half = trace_width / 2.0;
        for path in descendant_paths(doc, &|n| n.has_tag("via")) {
            let via = node_at(doc, &path);
            let Some((vx, vy)) = xy_of(via.find("at")) else {
                continue;
            };
            let vr = first_f(via.find("size"), 0.6) / 2.0;
            if vx + vr < min_x || vx - vr > max_x || vy + vr < min_y || vy - vr > max_y {
                continue;
            }
            if let Some(layers) = via.find("layers") {
                let names: Vec<String> = layers.atoms().map(|v| v.to_string()).collect();
                if !names.iter().any(|n| n == layer) {
                    continue;
                }
            }
            let vnet = self.net_of(via);
            if vnet == net && net != 0 {
                continue;
            }
            let br = vr + our_half + trace_clearance;
            self.mark_circle(blocked, vx, vy, br, min_x, min_y, cols, rows);
        }
        for path in descendant_paths(doc, &|n| n.has_tag("segment")) {
            if &path == exclude {
                continue;
            }
            let seg = node_at(doc, &path);
            if first_text(seg.find("layer")).unwrap_or_default() != layer {
                continue;
            }
            let onet = self.net_of(seg);
            let same_obs = same_net_obs.contains(&path);
            if onet == net && net != 0 && !same_obs {
                continue;
            }
            let (Some((osx, osy)), Some((oex, oey))) =
                (xy_of(seg.find("start")), xy_of(seg.find("end")))
            else {
                continue;
            };
            if osx.max(oex) < min_x || osx.min(oex) > max_x {
                continue;
            }
            if osy.max(oey) < min_y || osy.min(oey) > max_y {
                continue;
            }
            let ow = first_f(seg.find("width"), 0.25);
            let half = ow / 2.0 + our_half + trace_clearance;
            self.mark_segment(blocked, osx, osy, oex, oey, half, min_x, min_y, cols, rows);
        }
    }
}

fn astar(
    sx: i64,
    sy: i64,
    ex: i64,
    ey: i64,
    blocked: &HashSet<(i64, i64)>,
    cols: i64,
    rows: i64,
) -> Option<Vec<(i64, i64)>> {
    const NEIGHBORS: [(i64, i64, f64); 8] = [
        (1, 0, 1.0),
        (-1, 0, 1.0),
        (0, 1, 1.0),
        (0, -1, 1.0),
        (1, 1, 1.414),
        (-1, 1, 1.414),
        (1, -1, 1.414),
        (-1, -1, 1.414),
    ];
    let h = |x: i64, y: i64| {
        let dx = (x - ex).abs() as f64;
        let dy = (y - ey).abs() as f64;
        dx.max(dy) + 0.414 * dx.min(dy)
    };
    let mut nodes = vec![ANode {
        f: h(sx, sy),
        g: 0.0,
        x: sx,
        y: sy,
        parent: None,
    }];
    let mut heap = PyHeap { items: vec![0] };
    let mut g_scores: HashMap<(i64, i64), f64> = HashMap::from([((sx, sy), 0.0)]);
    let mut closed: HashSet<(i64, i64)> = HashSet::new();
    while let Some(ci) = heap.pop(&nodes) {
        let (cx, cy, cg) = (nodes[ci].x, nodes[ci].y, nodes[ci].g);
        if cx == ex && cy == ey {
            let mut path = Vec::new();
            let mut cur = Some(ci);
            while let Some(i) = cur {
                path.push((nodes[i].x, nodes[i].y));
                cur = nodes[i].parent;
            }
            path.reverse();
            return Some(path);
        }
        if !closed.insert((cx, cy)) {
            continue;
        }
        for (dx, dy, cost) in NEIGHBORS {
            let (nx, ny) = (cx + dx, cy + dy);
            if nx < 0 || nx >= cols || ny < 0 || ny >= rows {
                continue;
            }
            if closed.contains(&(nx, ny)) || blocked.contains(&(nx, ny)) {
                continue;
            }
            let ng = cg + cost;
            if ng < g_scores.get(&(nx, ny)).copied().unwrap_or(f64::INFINITY) {
                g_scores.insert((nx, ny), ng);
                nodes.push(ANode {
                    f: ng + h(nx, ny),
                    g: ng,
                    x: nx,
                    y: ny,
                    parent: Some(ci),
                });
                heap.push(&nodes, nodes.len() - 1);
            }
        }
    }
    None
}

fn simplify_path(path: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    if path.len() <= 2 {
        return path;
    }
    let mut out = vec![path[0]];
    for i in 1..path.len() - 1 {
        let (px, py) = *out.last().expect("non-empty");
        let (cx, cy) = path[i];
        let (nx, ny) = path[i + 1];
        let cross = (cx - px) * (ny - py) - (cy - py) * (nx - px);
        if cross.abs() > 1e-10 {
            out.push(path[i]);
        }
    }
    out.push(path[path.len() - 1]);
    out
}

/// KiCad `(segment ...)` node (uuid before net).
pub fn make_segment_node(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    width: f64,
    layer: &str,
    net: i64,
) -> SExp {
    SExp::list(
        "segment",
        [
            SExp::list("start", [SExp::atom(py_round(x1, 4)), SExp::atom(py_round(y1, 4))]),
            SExp::list("end", [SExp::atom(py_round(x2, 4)), SExp::atom(py_round(y2, 4))]),
            SExp::list("width", [SExp::atom(py_round(width, 4))]),
            SExp::list("layer", [SExp::quoted(layer)]),
            SExp::list("uuid", [SExp::quoted(crate::router::primitives::make_uuid())]),
            SExp::list("net", [SExp::atom(net)]),
        ],
    )
}

fn replace_segment(
    doc: &mut SExp,
    seg_path: &NodePath,
    path: &[(f64, f64)],
    layer: &str,
    net: i64,
    width: f64,
) {
    // Upstream only replaces direct children of the root.
    if seg_path.len() != 1 {
        return;
    }
    let idx = seg_path[0];
    if idx >= doc.children.len() {
        return;
    }
    doc.children.remove(idx);
    for (j, w) in path.windows(2).enumerate() {
        let node = make_segment_node(w[0].0, w[0].1, w[1].0, w[1].1, width, layer, net);
        doc.children.insert(idx + j, node);
    }
}
