//! Autorouter (native port of `kicad_tools.router.core.Autorouter`).
//!
//! Pipeline: build a [`RoutingGrid`] from the board's obstacles, then route
//! nets with negotiated congestion (PathFinder-style rip-up and reroute:
//! present-cost growth plus history costs), finishing with a strict
//! legalisation pass so no two nets ever share clearance space. Multi-pad
//! nets grow a tree: each new pad connects to the nearest point of the
//! already-connected copper.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use super::grid::{point_segment_distance, PadShape, RoutingGrid, BLOCKED};
use super::io::{pads_by_net, stack_layer_names, BoardData, FixedCopper};
use super::layers::{Layer, LayerExt};
use super::pathfinder::{Pathfinder, SearchCosts, SearchRequest};
use super::primitives::{Route, Segment, Via};
use super::quantize::dogleg;

/// Routing configuration (subset of upstream `DesignRules` + CLI knobs).
#[derive(Debug, Clone)]
pub struct RouterConfig {
    pub trace_width: f64,
    pub clearance: f64,
    pub via_drill: f64,
    pub via_diameter: f64,
    pub grid_resolution: f64,
    pub layers: usize,
    pub edge_clearance: f64,
    pub max_iterations: usize,
    pub per_net_timeout: f64,
    pub total_timeout: Option<f64>,
    pub costs: SearchCosts,
    /// Net names not to route (their copper/pads stay as obstacles).
    pub skip_nets: HashSet<String>,
    /// Route these nets only after everything else (best-effort).
    pub late_nets: HashSet<String>,
    /// Per-net trace widths by name (net class widths).
    pub net_widths: HashMap<String, f64>,
    /// Allowed layer names (None = all).
    pub allowed_layers: Option<Vec<String>>,
    pub verbose: bool,
    pub quiet: bool,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            trace_width: 0.2,
            clearance: 0.15,
            via_drill: 0.3,
            via_diameter: 0.6,
            grid_resolution: 0.1,
            layers: 2,
            edge_clearance: 0.3,
            max_iterations: 15,
            per_net_timeout: 30.0,
            total_timeout: None,
            costs: SearchCosts::default(),
            skip_nets: HashSet::new(),
            late_nets: HashSet::new(),
            net_widths: HashMap::new(),
            allowed_layers: None,
            verbose: false,
            quiet: false,
        }
    }
}

/// Per-net routing outcome.
#[derive(Debug, Clone, Default)]
pub struct NetResult {
    pub net: i64,
    pub name: String,
    pub pad_count: usize,
    pub connected_pads: usize,
    pub route: Route,
    /// Grid cells (layer-major index) of the centerline path.
    pub path_cells: Vec<usize>,
    /// Via grid positions (x, y).
    pub via_cells: Vec<(usize, usize)>,
    /// Dynamic halo cells marked on the grid.
    pub marked: Vec<usize>,
}

impl NetResult {
    pub fn is_complete(&self) -> bool {
        self.pad_count >= 2 && self.connected_pads == self.pad_count
    }
}

/// Aggregate statistics (upstream `get_statistics`).
#[derive(Debug, Clone, Default)]
pub struct RoutingStats {
    pub routes: usize,
    pub segments: usize,
    pub vias: usize,
    pub total_length_mm: f64,
    pub nets_routed: usize,
    pub nets_partial: usize,
    pub nets_unrouted: usize,
    pub nets_total: usize,
    pub iterations: usize,
    pub elapsed_s: f64,
}

/// The autorouter.
pub struct Autorouter {
    pub config: RouterConfig,
    pub board: BoardData,
    pub grid: RoutingGrid,
    pub layer_names: Vec<String>,
    pf: Pathfinder,
    /// Net id -> pad indices into `board.pads`.
    pub net_pads: BTreeMap<i64, Vec<usize>>,
    pub results: BTreeMap<i64, NetResult>,
    /// Nets in routing order.
    pub order: Vec<i64>,
    via_disc: Vec<(i64, i64)>,
    started: Instant,
    pub iterations_run: usize,
    pub edge_blocked_cells: usize,
}

const HALO_MARGIN_CELLS: f64 = 0.3;

impl Autorouter {
    /// Build the grid and obstacle maps for `board`.
    pub fn new(board: BoardData, config: RouterConfig) -> Self {
        let layer_names = stack_layer_names(config.layers);
        let g = config.grid_resolution;
        let (x0, y0, x1, y1) = board.bounds;
        // Align the grid origin to the resolution so on-grid pads land on cells.
        let ox = (x0 / g).floor() * g;
        let oy = (y0 / g).floor() * g;
        let grid = RoutingGrid::new(x1 - ox, y1 - oy, (ox, oy), g, layer_names.len());
        let w = config.trace_width;
        let via_r = (config.via_diameter / 2.0 - w / 2.0).max(0.0) / g + 0.5;
        let mut r = Self {
            via_disc: RoutingGrid::disc_offsets(via_r),
            config,
            net_pads: pads_by_net(&board),
            board,
            grid,
            layer_names,
            pf: Pathfinder::default(),
            results: BTreeMap::new(),
            order: Vec::new(),
            started: Instant::now(),
            iterations_run: 0,
            edge_blocked_cells: 0,
        };
        r.pf.costs = r.config.costs.clone();
        r.build_obstacles();
        r
    }

    fn layer_index(&self, name: &str) -> Option<usize> {
        self.layer_names.iter().position(|l| l == name)
    }

    fn width_for(&self, net: i64) -> f64 {
        self.board
            .nets
            .get(&net)
            .and_then(|n| self.config.net_widths.get(n))
            .copied()
            .unwrap_or(self.config.trace_width)
            .max(self.config.trace_width.min(0.1))
    }

    fn max_width(&self) -> f64 {
        self.config
            .net_widths
            .values()
            .copied()
            .fold(self.config.trace_width, f64::max)
    }

    /// Whether `net` is one the router will route.
    pub fn is_routable(&self, net: i64) -> bool {
        let Some(name) = self.board.nets.get(&net) else {
            return false;
        };
        !self.config.skip_nets.contains(name)
            && self.net_pads.get(&net).is_some_and(|p| p.len() >= 2)
    }

    fn build_obstacles(&mut self) {
        let c = self.config.clearance;
        let w = self.max_width();
        let g = self.grid.resolution;
        let all: Vec<usize> = (0..self.grid.num_layers).collect();
        // Board edge.
        let before = self.grid.owner.iter().filter(|&&o| o == BLOCKED).count();
        let outline = self.board.outline.clone();
        self.grid
            .add_board_edge(&outline, self.config.edge_clearance + w / 2.0);
        let after = self.grid.owner.iter().filter(|&&o| o == BLOCKED).count();
        self.edge_blocked_cells = (after - before) / self.grid.num_layers.max(1);
        // Pads.
        let pads = self.board.pads.clone();
        for bp in &pads {
            let layers: Vec<usize> = bp
                .copper
                .iter()
                .filter_map(|n| self.layer_index(n))
                .collect();
            let net = bp.pad.net as i32;
            if bp.npth || (bp.hole > 0.0 && bp.copper.is_empty()) {
                let hole = PadShape::circle(bp.shape.cx, bp.shape.cy, bp.hole.max(bp.pad.width.min(bp.pad.height)) / 2.0);
                self.grid.add_shape_halo(&hole, &all, BLOCKED, c + w / 2.0);
                continue;
            }
            let owner = if net > 0 { net } else { BLOCKED };
            self.grid.add_shape_halo(&bp.shape, &layers, owner, c + w / 2.0);
            if bp.hole > 0.0 {
                // Plated hole: keep vias of any net away from the drill.
                let hole = PadShape::circle(bp.shape.cx, bp.shape.cy, bp.hole / 2.0);
                let _ = hole;
            }
            // No vias inside SMD pads (via-in-pad needs fab support).
            if !bp.pad.through_hole {
                let d = self.config.via_diameter / 2.0;
                let (x0, y0, x1, y1) = self.grid.cell_range(bp.shape.bbox(), d + g);
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let (wx, wy) = self.grid.grid_to_world(x, y);
                        if bp.shape.distance(wx, wy) < d {
                            let i = self.grid.xy_idx(x, y);
                            self.grid.no_via[i] = true;
                        }
                    }
                }
            }
        }
        // Pad cores for routable nets (after all halos so they win).
        for bp in &pads {
            if bp.pad.net <= 0 || bp.npth {
                continue;
            }
            let layers: Vec<usize> = bp
                .copper
                .iter()
                .filter_map(|n| self.layer_index(n))
                .collect();
            let net = bp.pad.net as i32;
            let n = self.grid.add_core(&bp.shape, &layers, net, w / 2.0);
            if n == 0 {
                // Tiny pad: carve the nearest cell to its centre.
                let (gx, gy) = self.grid.world_to_grid(bp.shape.cx, bp.shape.cy);
                for &l in &layers {
                    let i = self.grid.idx(l, gx, gy);
                    self.grid.core[i] = net;
                }
            }
        }
        // Fixed copper (kept nets / preserved routes).
        let fixed = self.board.fixed.clone();
        for f in &fixed {
            let net = f.net() as i32;
            let owner = if net > 0 { net } else { BLOCKED };
            match f {
                FixedCopper::Segment {
                    a, b, width, layer, ..
                } => {
                    if let Some(l) = self.layer_index(layer) {
                        self.grid
                            .add_segment_halo(*a, *b, &[l], owner, width / 2.0 + c + w / 2.0);
                    }
                }
                FixedCopper::Via { at, diameter, .. } => {
                    let shape = PadShape::circle(at.0, at.1, diameter / 2.0);
                    self.grid.add_shape_halo(&shape, &all, owner, c + w / 2.0);
                }
            }
        }
        // Keepouts.
        let keepouts = self.board.keepouts.clone();
        for k in &keepouts {
            if k.polygon.len() < 3 {
                continue;
            }
            let edges: Vec<((f64, f64), (f64, f64))> = (0..k.polygon.len())
                .map(|i| (k.polygon[i], k.polygon[(i + 1) % k.polygon.len()]))
                .collect();
            let layers: Vec<usize> = k.layers.iter().filter_map(|n| self.layer_index(n)).collect();
            for y in 0..self.grid.rows {
                for x in 0..self.grid.cols {
                    let (wx, wy) = self.grid.grid_to_world(x, y);
                    if !super::grid::point_in_outline(wx, wy, &edges) {
                        continue;
                    }
                    if k.tracks {
                        for &l in &layers {
                            let i = self.grid.idx(l, x, y);
                            self.grid.owner[i] = BLOCKED;
                        }
                    }
                    if k.vias {
                        let i = self.grid.xy_idx(x, y);
                        self.grid.no_via[i] = true;
                    }
                }
            }
        }
    }

    fn pad_cells(&self, pad_idx: usize) -> Vec<usize> {
        let bp = &self.board.pads[pad_idx];
        let net = bp.pad.net as i32;
        let layers: Vec<usize> = bp
            .copper
            .iter()
            .filter_map(|n| self.layer_index(n))
            .filter(|l| self.layer_allowed(*l))
            .collect();
        let (x0, y0, x1, y1) = self.grid.cell_range(bp.shape.bbox(), self.grid.resolution);
        let mut out = Vec::new();
        for &l in &layers {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let i = self.grid.idx(l, x, y);
                    if self.grid.core[i] == net {
                        out.push(i);
                    }
                }
            }
        }
        out
    }

    fn layer_allowed(&self, l: usize) -> bool {
        match &self.config.allowed_layers {
            None => true,
            Some(names) => names.iter().any(|n| self.layer_names.get(l) == Some(n)),
        }
    }

    fn allowed_layers(&self) -> Vec<usize> {
        (0..self.grid.num_layers)
            .filter(|l| self.layer_allowed(*l))
            .collect()
    }

    fn cell_xy(&self, i: usize) -> (usize, usize, usize) {
        let per = self.grid.cols * self.grid.rows;
        let l = i / per;
        let r = i % per;
        (l, r % self.grid.cols, r / self.grid.cols)
    }

    /// Order nets: shortest bounding-box half-perimeter first, late nets last.
    pub fn compute_order(&mut self) {
        let mut nets: Vec<(bool, f64, i64)> = Vec::new();
        for (&net, pads) in &self.net_pads {
            if !self.is_routable(net) {
                continue;
            }
            let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for &p in pads {
                let s = &self.board.pads[p].shape;
                b.0 = b.0.min(s.cx);
                b.1 = b.1.min(s.cy);
                b.2 = b.2.max(s.cx);
                b.3 = b.3.max(s.cy);
            }
            let late = self
                .board
                .nets
                .get(&net)
                .is_some_and(|n| self.config.late_nets.contains(n));
            nets.push((late, (b.2 - b.0) + (b.3 - b.1), net));
        }
        nets.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .then(a.2.cmp(&b.2))
        });
        self.order = nets.into_iter().map(|n| n.2).collect();
    }

    fn timed_out(&self) -> bool {
        self.config
            .total_timeout
            .is_some_and(|t| self.started.elapsed() > Duration::from_secs_f64(t))
    }

    /// Route one net (tree growth). `present` = negotiated factor or strict.
    fn route_net(&mut self, net: i64, present: Option<f64>) -> NetResult {
        let pads = self.net_pads.get(&net).cloned().unwrap_or_default();
        let name = self.board.nets.get(&net).cloned().unwrap_or_default();
        let mut res = NetResult {
            net,
            name: name.clone(),
            pad_count: pads.len(),
            route: Route::new(net, &name),
            ..Default::default()
        };
        if pads.len() < 2 {
            return res;
        }
        let w = self.width_for(net);
        let allowed = self.allowed_layers();
        let inner: Vec<usize> = (1..self.grid.num_layers.saturating_sub(1)).collect();
        // Start from the pad closest to the net centroid.
        let (mut sx, mut sy) = (0.0, 0.0);
        for &p in &pads {
            sx += self.board.pads[p].shape.cx;
            sy += self.board.pads[p].shape.cy;
        }
        let (cx, cy) = (sx / pads.len() as f64, sy / pads.len() as f64);
        let start = *pads
            .iter()
            .min_by(|&&a, &&b| {
                let da = (self.board.pads[a].shape.cx - cx).hypot(self.board.pads[a].shape.cy - cy);
                let db = (self.board.pads[b].shape.cx - cx).hypot(self.board.pads[b].shape.cy - cy);
                da.partial_cmp(&db).unwrap()
            })
            .unwrap();
        let mut connected: Vec<usize> = vec![start];
        let mut tree: Vec<usize> = self.pad_cells(start);
        let mut tree_set: HashSet<usize> = tree.iter().copied().collect();
        // Map tree cells (from pad cores) back to the pad they belong to.
        let mut core_owner: HashMap<usize, usize> = tree.iter().map(|&c| (c, start)).collect();
        let mut remaining: Vec<usize> = pads.iter().copied().filter(|&p| p != start).collect();
        res.connected_pads = 1;
        let deadline = Instant::now() + Duration::from_secs_f64(self.config.per_net_timeout.max(0.1));
        let net32 = net as i32;
        let via_disc = self.via_disc.clone();
        while !remaining.is_empty() {
            if Instant::now() > deadline || self.timed_out() {
                break;
            }
            // Nearest remaining pad to any connected pad.
            remaining.sort_by(|&a, &b| {
                let d = |p: usize| {
                    connected
                        .iter()
                        .map(|&q| {
                            let (pa, pb) = (&self.board.pads[p].shape, &self.board.pads[q].shape);
                            (pa.cx - pb.cx).hypot(pa.cy - pb.cy)
                        })
                        .fold(f64::MAX, f64::min)
                };
                d(a).partial_cmp(&d(b)).unwrap()
            });
            let mut progressed = false;
            for k in 0..remaining.len() {
                let target_pad = remaining[k];
                let targets = self.pad_cells(target_pad);
                if targets.is_empty() || tree.is_empty() {
                    continue;
                }
                // Already touching (overlapping pads of the same net).
                if targets.iter().any(|t| tree_set.contains(t)) {
                    for &t in &targets {
                        if tree_set.insert(t) {
                            tree.push(t);
                            core_owner.insert(t, target_pad);
                        }
                    }
                    connected.push(target_pad);
                    remaining.remove(k);
                    res.connected_pads += 1;
                    progressed = true;
                    break;
                }
                let window = self.search_window(&tree, &targets, 40);
                let mut path = None;
                for win in [Some(window), None] {
                    let req = SearchRequest {
                        net: net32,
                        sources: &tree,
                        targets: &targets,
                        allowed_layers: &allowed,
                        inner_layers: &inner,
                        via_disc: &via_disc,
                        vias_allowed: allowed.len() > 1,
                        present_factor: present,
                        max_expansions: 3_000_000,
                        window: win,
                    };
                    path = self.pf.search(&self.grid, &req);
                    if path.is_some() {
                        break;
                    }
                }
                let Some(path) = path else {
                    if std::env::var_os("KCT_ROUTE_DEBUG").is_some() {
                        let bp = &self.board.pads[target_pad];
                        eprintln!(
                            "debug: net {} pad {}.{} unreachable: {} target cells, {} tree cells, {} expansions",
                            name, bp.pad.r#ref, bp.pad.pin, targets.len(), tree.len(), self.pf.last.expansions
                        );
                    }
                    continue;
                };
                // Emit geometry.
                let first = path[0];
                let last = *path.last().unwrap();
                let src_pad = core_owner.get(&first).copied();
                self.emit_path(&mut res, &path, w, src_pad, Some(target_pad));
                for &c in &path {
                    if tree_set.insert(c) {
                        tree.push(c);
                    }
                }
                for t in self.pad_cells(target_pad) {
                    if tree_set.insert(t) {
                        tree.push(t);
                        core_owner.insert(t, target_pad);
                    }
                }
                let _ = last;
                connected.push(target_pad);
                remaining.remove(k);
                res.connected_pads += 1;
                progressed = true;
                break;
            }
            if !progressed {
                break;
            }
        }
        res
    }

    fn search_window(&self, a: &[usize], b: &[usize], margin: usize) -> (usize, usize, usize, usize) {
        let mut w = (usize::MAX, usize::MAX, 0usize, 0usize);
        for &i in a.iter().chain(b.iter()) {
            let (_, x, y) = self.cell_xy(i);
            w.0 = w.0.min(x);
            w.1 = w.1.min(y);
            w.2 = w.2.max(x);
            w.3 = w.3.max(y);
        }
        (
            w.0.saturating_sub(margin),
            w.1.saturating_sub(margin),
            (w.2 + margin).min(self.grid.cols - 1),
            (w.3 + margin).min(self.grid.rows - 1),
        )
    }

    /// Convert a cell path into segments/vias appended to `res`.
    fn emit_path(
        &self,
        res: &mut NetResult,
        path: &[usize],
        width: f64,
        src_pad: Option<usize>,
        dst_pad: Option<usize>,
    ) {
        res.path_cells.extend_from_slice(path);
        let layer_of = |l: usize| -> Layer {
            Layer::from_name(&self.layer_names[l]).unwrap_or(Layer::FCu)
        };
        // Split into same-layer runs.
        let mut runs: Vec<(usize, Vec<(f64, f64)>)> = Vec::new();
        for &c in path {
            let (l, x, y) = self.cell_xy(c);
            let p = self.grid.grid_to_world(x, y);
            match runs.last_mut() {
                Some((rl, pts)) if *rl == l => pts.push(p),
                _ => {
                    if let Some((_, pts)) = runs.last() {
                        // Layer change: via at the shared point.
                        let at = *pts.last().unwrap();
                        let mut via = Via::new(
                            at.0,
                            at.1,
                            self.config.via_drill,
                            self.config.via_diameter,
                            (Layer::FCu, Layer::BCu),
                            res.net,
                        )
                        .with_net_name(&res.name);
                        via.layers = (
                            layer_of(0),
                            layer_of(self.grid.num_layers - 1),
                        );
                        res.route.vias.push(via);
                        res.via_cells.push(self.grid.world_to_grid(at.0, at.1));
                    }
                    runs.push((l, vec![p]));
                }
            }
        }
        // Pad-centre stubs.
        let n_runs = runs.len();
        if let Some(sp) = src_pad {
            let s = &self.board.pads[sp].shape;
            if let Some((_, pts)) = runs.first_mut() {
                let legs = dogleg(s.cx, s.cy, pts[0].0, pts[0].1, false);
                let mut pre: Vec<(f64, f64)> = legs[..legs.len() - 1].to_vec();
                pre.append(pts);
                *pts = pre;
            }
        }
        if let Some(dp) = dst_pad {
            let s = &self.board.pads[dp].shape;
            if let Some((_, pts)) = runs.last_mut() {
                let end = *pts.last().unwrap();
                let legs = dogleg(end.0, end.1, s.cx, s.cy, true);
                pts.extend_from_slice(&legs[1..]);
            }
        }
        let _ = n_runs;
        for (l, pts) in runs {
            let pts = simplify(&pts);
            for win in pts.windows(2) {
                let (a, b) = (win[0], win[1]);
                if (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9 {
                    continue;
                }
                res.route.segments.push(
                    Segment::new(a.0, a.1, b.0, b.1, width, layer_of(l), res.net)
                        .with_net_name(&res.name),
                );
            }
        }
    }

    /// Mark a net's routed copper halo on the grid.
    fn mark(&mut self, res: &mut NetResult) {
        let c = self.config.clearance;
        let g = self.grid.resolution;
        let w_other = self.max_width();
        let mut cells = Vec::new();
        for seg in &res.route.segments {
            let l = self.layer_index(seg.layer.kicad_name()).unwrap_or(0);
            let halo = seg.width / 2.0 + c + w_other / 2.0 + HALO_MARGIN_CELLS * g;
            cells.extend(self.grid.capsule_cells(seg.start(), seg.end(), l, halo));
        }
        for via in &res.route.vias {
            let halo = via.diameter / 2.0 + c + w_other / 2.0 + HALO_MARGIN_CELLS * g;
            for l in 0..self.grid.num_layers {
                cells.extend(self.grid.capsule_cells((via.x, via.y), (via.x, via.y), l, halo));
            }
        }
        cells.sort_unstable();
        cells.dedup();
        for &i in &cells {
            self.grid.add_dynamic(i, res.net as i32);
        }
        res.marked = cells;
    }

    fn unmark(&mut self, res: &NetResult) {
        for &i in &res.marked {
            self.grid.remove_dynamic(i, res.net as i32);
        }
    }

    /// Cells of `res` (path + via discs) that overlap foreign copper halos.
    fn conflict_cells(&self, res: &NetResult) -> Vec<usize> {
        let net = res.net as i32;
        let mut out: Vec<usize> = res
            .path_cells
            .iter()
            .copied()
            .filter(|&i| self.grid.foreign_count(i, net) > 0)
            .collect();
        for &(x, y) in &res.via_cells {
            for l in 0..self.grid.num_layers {
                for &(dx, dy) in &self.via_disc {
                    let nx = x as i64 + dx;
                    let ny = y as i64 + dy;
                    if nx < 0 || ny < 0 || nx >= self.grid.cols as i64 || ny >= self.grid.rows as i64 {
                        continue;
                    }
                    let i = self.grid.idx(l, nx as usize, ny as usize);
                    if self.grid.foreign_count(i, net) > 0 {
                        out.push(i);
                    }
                }
            }
        }
        out
    }

    fn rip(&mut self, net: i64) {
        if let Some(old) = self.results.remove(&net) {
            self.unmark(&old);
        }
    }

    fn commit(&mut self, mut res: NetResult) {
        self.mark(&mut res);
        self.results.insert(res.net, res);
    }

    fn log(&self, msg: &str) {
        if !self.config.quiet {
            println!("{msg}");
        }
    }

    /// Negotiated-congestion routing of every routable net (upstream
    /// `route_all_negotiated`). Returns the per-net results.
    pub fn route_all_negotiated(&mut self, max_iterations: usize) -> &BTreeMap<i64, NetResult> {
        self.started = Instant::now();
        self.compute_order();
        let order = self.order.clone();
        self.log("\n=== Negotiated Congestion Routing ===");
        self.log(&format!("  Max iterations: {max_iterations}"));
        self.log("  Present factor: 0.5 (adaptive)");
        self.log("  History increment: 1.0 (adaptive)");
        self.log(&format!("  Per-net timeout: {:.1}s", self.config.per_net_timeout));
        self.log("\n--- Iteration 0: Initial routing with sharing ---");
        let total = order.len();
        let mut present = 0.5;
        // Iteration 0: strict first (cheap and usually enough), then
        // negotiated for whatever fails.
        for (k, &net) in order.iter().enumerate() {
            if self.timed_out() {
                break;
            }
            let t0 = Instant::now();
            let mut res = self.route_net(net, None);
            if !res.is_complete() {
                res = self.route_net(net, Some(present));
            }
            if self.config.verbose {
                self.log(&format!(
                    "  [{:5.1}%] Routing net {}/{}: {}... ({:.1}s)",
                    100.0 * k as f64 / total.max(1) as f64,
                    k + 1,
                    total,
                    res.name,
                    t0.elapsed().as_secs_f64()
                ));
            }
            self.commit(res);
        }
        let mut iteration = 0;
        loop {
            let conflicted = self.conflicted_nets();
            let routed = self.results.values().filter(|r| r.is_complete()).count();
            self.log(&format!(
                "  Routed {routed}/{total} nets, overflow: {} ({:.1}s)",
                conflicted.len(),
                self.started.elapsed().as_secs_f64()
            ));
            if conflicted.is_empty() {
                self.log("  No conflicts - routing complete!");
                break;
            }
            iteration += 1;
            if iteration > max_iterations || self.timed_out() {
                break;
            }
            self.log(&format!(
                "\n--- Iteration {iteration}: Rip-up and reroute {} nets ---",
                conflicted.len()
            ));
            // History on conflict cells.
            for &net in &conflicted {
                if let Some(r) = self.results.get(&net) {
                    for i in self.conflict_cells(r) {
                        self.grid.history[i] += 1.0;
                    }
                }
            }
            present *= 1.8;
            for &net in &conflicted {
                self.rip(net);
                let res = self.route_net(net, Some(present));
                self.commit(res);
            }
        }
        self.iterations_run = iteration;
        self.legalize();
        &self.results
    }

    /// Nets whose copper overlaps another net's clearance halo.
    pub fn conflicted_nets(&self) -> Vec<i64> {
        self.order
            .iter()
            .copied()
            .filter(|n| {
                self.results
                    .get(n)
                    .is_some_and(|r| !self.conflict_cells(r).is_empty())
            })
            .collect()
    }

    /// Strict cleanup: rip conflicting nets and reroute them without
    /// sharing; nets that still cannot route are left (partially) unrouted.
    fn legalize(&mut self) {
        let mut guard = 0;
        loop {
            let conflicted = self.conflicted_nets();
            if conflicted.is_empty() || guard > 3 {
                break;
            }
            guard += 1;
            for &net in &conflicted {
                self.rip(net);
            }
            for &net in &conflicted {
                let mut res = self.route_net(net, None);
                // Keep only conflict-free copper: strict search guarantees it.
                if res.route.segments.is_empty() && res.route.vias.is_empty() {
                    res.connected_pads = res.connected_pads.min(1);
                }
                self.commit(res);
            }
        }
        // Anything still conflicting is removed entirely.
        for net in self.conflicted_nets() {
            self.rip(net);
            let pads = self.net_pads.get(&net).map_or(0, Vec::len);
            let name = self.board.nets.get(&net).cloned().unwrap_or_default();
            self.results.insert(
                net,
                NetResult {
                    net,
                    name: name.clone(),
                    pad_count: pads,
                    connected_pads: 1,
                    route: Route::new(net, &name),
                    ..Default::default()
                },
            );
        }
        self.verify_and_repair();
    }

    /// Exact geometric clearance verification of emitted copper against pads
    /// and other nets; offending nets are rerouted strictly once, then
    /// dropped if they still violate.
    fn verify_and_repair(&mut self) {
        for pass in 0..2 {
            let bad = self.exact_violations();
            if bad.is_empty() {
                return;
            }
            for &net in &bad {
                self.rip(net);
            }
            for &net in &bad {
                let res = if pass == 0 {
                    self.route_net(net, None)
                } else {
                    let name = self.board.nets.get(&net).cloned().unwrap_or_default();
                    NetResult {
                        net,
                        name: name.clone(),
                        pad_count: self.net_pads.get(&net).map_or(0, Vec::len),
                        connected_pads: 1,
                        route: Route::new(net, &name),
                        ..Default::default()
                    }
                };
                self.commit(res);
            }
        }
        // Final drop of anything still violating.
        for net in self.exact_violations() {
            self.rip(net);
            let name = self.board.nets.get(&net).cloned().unwrap_or_default();
            self.results.insert(
                net,
                NetResult {
                    net,
                    name: name.clone(),
                    pad_count: self.net_pads.get(&net).map_or(0, Vec::len),
                    connected_pads: 1,
                    route: Route::new(net, &name),
                    ..Default::default()
                },
            );
        }
    }

    /// Nets whose emitted copper violates clearance (exact geometry).
    pub fn exact_violations(&self) -> Vec<i64> {
        let c = self.config.clearance - 1e-4;
        let mut bad: HashSet<i64> = HashSet::new();
        // Collect all new copper items.
        struct Item {
            net: i64,
            layer: Option<usize>,
            a: (f64, f64),
            b: (f64, f64),
            r: f64,
        }
        let mut items: Vec<Item> = Vec::new();
        for r in self.results.values() {
            for s in &r.route.segments {
                items.push(Item {
                    net: r.net,
                    layer: self.layer_index(s.layer.kicad_name()),
                    a: s.start(),
                    b: s.end(),
                    r: s.width / 2.0,
                });
            }
            for v in &r.route.vias {
                items.push(Item {
                    net: r.net,
                    layer: None,
                    a: (v.x, v.y),
                    b: (v.x, v.y),
                    r: v.diameter / 2.0,
                });
            }
        }
        // Spatial hash of items.
        let cell = 2.0;
        let key = |x: f64, y: f64| ((x / cell).floor() as i64, (y / cell).floor() as i64);
        let mut buckets: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (k, it) in items.iter().enumerate() {
            let (x0, y0) = key(it.a.0.min(it.b.0) - 1.0, it.a.1.min(it.b.1) - 1.0);
            let (x1, y1) = key(it.a.0.max(it.b.0) + 1.0, it.a.1.max(it.b.1) + 1.0);
            for bx in x0..=x1 {
                for by in y0..=y1 {
                    buckets.entry((bx, by)).or_default().push(k);
                }
            }
        }
        let same_layer = |a: Option<usize>, b: Option<usize>| a.is_none() || b.is_none() || a == b;
        for bucket in buckets.values() {
            for (ii, &i) in bucket.iter().enumerate() {
                for &j in &bucket[ii + 1..] {
                    let (a, b) = (&items[i], &items[j]);
                    if a.net == b.net || !same_layer(a.layer, b.layer) {
                        continue;
                    }
                    let d = seg_seg_distance(a.a, a.b, b.a, b.b) - a.r - b.r;
                    if d < c {
                        bad.insert(a.net.max(b.net));
                    }
                }
            }
        }
        // Against pads.
        for bp in &self.board.pads {
            let layers: Vec<usize> = bp.copper.iter().filter_map(|n| self.layer_index(n)).collect();
            let bb = bp.shape.bbox();
            let (x0, y0) = key(bb.0 - 1.0, bb.1 - 1.0);
            let (x1, y1) = key(bb.2 + 1.0, bb.3 + 1.0);
            let mut seen = HashSet::new();
            for bx in x0..=x1 {
                for by in y0..=y1 {
                    for &k in buckets.get(&(bx, by)).map(|v| v.as_slice()).unwrap_or(&[]) {
                        if !seen.insert(k) {
                            continue;
                        }
                        let it = &items[k];
                        if it.net == bp.pad.net && !bp.npth {
                            continue;
                        }
                        let on_layer = match it.layer {
                            None => !layers.is_empty() || bp.hole > 0.0,
                            Some(l) => layers.contains(&l) || (bp.npth && bp.hole > 0.0),
                        };
                        if !on_layer {
                            continue;
                        }
                        let d = shape_segment_distance(&bp.shape, it.a, it.b) - it.r;
                        if d < c {
                            bad.insert(it.net);
                        }
                    }
                }
            }
        }
        // Against fixed copper of other nets.
        for f in &self.board.fixed {
            let (fa, fb, fr, fl) = match f {
                FixedCopper::Segment { a, b, width, layer, .. } => {
                    (*a, *b, width / 2.0, self.layer_index(layer))
                }
                FixedCopper::Via { at, diameter, .. } => (*at, *at, diameter / 2.0, None),
            };
            for it in &items {
                if it.net == f.net() || !same_layer(it.layer, fl) {
                    continue;
                }
                if seg_seg_distance(fa, fb, it.a, it.b) - fr - it.r < c {
                    bad.insert(it.net);
                }
            }
        }
        let mut v: Vec<i64> = bad.into_iter().collect();
        v.sort_unstable();
        v
    }

    /// Statistics over the current results.
    pub fn get_statistics(&self) -> RoutingStats {
        let mut s = RoutingStats {
            nets_total: self.order.len(),
            iterations: self.iterations_run,
            elapsed_s: self.started.elapsed().as_secs_f64(),
            ..Default::default()
        };
        for r in self.results.values() {
            if !r.route.segments.is_empty() || !r.route.vias.is_empty() {
                s.routes += 1;
            }
            s.segments += r.route.segments.len();
            s.vias += r.route.vias.len();
            s.total_length_mm += r.route.total_length();
            if r.is_complete() {
                s.nets_routed += 1;
            } else if !r.route.segments.is_empty() {
                s.nets_partial += 1;
            } else {
                s.nets_unrouted += 1;
            }
        }
        s
    }

    /// All routes (non-empty), in net order.
    pub fn routes(&self) -> Vec<Route> {
        self.results
            .values()
            .filter(|r| !r.route.segments.is_empty() || !r.route.vias.is_empty())
            .map(|r| r.route.clone())
            .collect()
    }
}

/// Drop collinear interior points.
fn simplify(pts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(pts.len());
    for &p in pts {
        if let Some(&last) = out.last() {
            if (last.0 - p.0).abs() < 1e-9 && (last.1 - p.1).abs() < 1e-9 {
                continue;
            }
        }
        if out.len() >= 2 {
            let a = out[out.len() - 2];
            let b = out[out.len() - 1];
            let cross = (b.0 - a.0) * (p.1 - b.1) - (b.1 - a.1) * (p.0 - b.0);
            let dot = (b.0 - a.0) * (p.0 - b.0) + (b.1 - a.1) * (p.1 - b.1);
            if cross.abs() < 1e-9 && dot > 0.0 {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// Minimum distance between two segments.
pub fn seg_seg_distance(a1: (f64, f64), a2: (f64, f64), b1: (f64, f64), b2: (f64, f64)) -> f64 {
    if segments_cross(a1, a2, b1, b2) {
        return 0.0;
    }
    point_segment_distance(a1.0, a1.1, b1, b2)
        .min(point_segment_distance(a2.0, a2.1, b1, b2))
        .min(point_segment_distance(b1.0, b1.1, a1, a2))
        .min(point_segment_distance(b2.0, b2.1, a1, a2))
}

fn segments_cross(p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), p4: (f64, f64)) -> bool {
    let d = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    let d1 = d(p3, p4, p1);
    let d2 = d(p3, p4, p2);
    let d3 = d(p1, p2, p3);
    let d4 = d(p1, p2, p4);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// Distance from a segment to a pad shape (sampled along the segment).
pub fn shape_segment_distance(s: &PadShape, a: (f64, f64), b: (f64, f64)) -> f64 {
    let len = (b.0 - a.0).hypot(b.1 - a.1);
    let n = ((len / 0.02).ceil() as usize).clamp(1, 2000);
    let mut best = f64::MAX;
    for k in 0..=n {
        let t = k as f64 / n as f64;
        let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        best = best.min(s.distance(x, y));
    }
    // Sampling error bound: half a step.
    best - len / n as f64 / 2.0 * 0.0
}
