//! A* pathfinder over [`RoutingGrid`] (native replacement for
//! `kicad_tools.router.pathfinder.Router` and the C++ `Pathfinder`).
//!
//! 8-connected moves on each layer plus through-via layer changes. Supports
//! multi-source / multi-target searches (tree growth for multi-pad nets),
//! strict mode (foreign routed copper is impassable) and negotiated mode
//! (foreign copper costs `present_factor * (1 + history)`).

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use super::grid::RoutingGrid;

/// Search cost parameters (upstream `DesignRules` A* costs).
#[derive(Debug, Clone)]
pub struct SearchCosts {
    pub cost_straight: f64,
    pub cost_diagonal: f64,
    pub cost_turn: f64,
    pub cost_via: f64,
    /// Extra per-cell cost on non-preferred (inner) layers.
    pub cost_layer_inner: f64,
}

impl Default for SearchCosts {
    fn default() -> Self {
        Self {
            cost_straight: 1.0,
            cost_diagonal: 1.414,
            cost_turn: 5.0,
            cost_via: 10.0,
            cost_layer_inner: 2.0,
        }
    }
}

/// One search request.
#[derive(Debug, Clone)]
pub struct SearchRequest<'a> {
    pub net: i32,
    pub sources: &'a [usize],
    pub targets: &'a [usize],
    pub allowed_layers: &'a [usize],
    /// Inner layers (cost_layer_inner applies).
    pub inner_layers: &'a [usize],
    /// Via keep-out disc (cell offsets) checked on every layer.
    pub via_disc: &'a [(i64, i64)],
    pub vias_allowed: bool,
    /// `None` = strict (foreign copper impassable); `Some(f)` = negotiated.
    pub present_factor: Option<f64>,
    pub max_expansions: usize,
    /// Optional search window (x0, y0, x1, y1) in cells.
    pub window: Option<(usize, usize, usize, usize)>,
}

#[derive(Copy, Clone, PartialEq)]
struct Node {
    f: f64,
    g: f64,
    idx: usize,
}

impl Eq for Node {}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .f
            .partial_cmp(&self.f)
            .unwrap_or(Ordering::Equal)
            .then_with(|| other.g.partial_cmp(&self.g).unwrap_or(Ordering::Equal).reverse())
            .then_with(|| other.idx.cmp(&self.idx))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Search statistics of the last call.
#[derive(Debug, Clone, Default)]
pub struct SearchStats {
    pub expansions: usize,
    pub found: bool,
}

/// Reusable A* state (arrays sized to the grid, generation stamped).
#[derive(Debug, Default)]
pub struct Pathfinder {
    g: Vec<f64>,
    came: Vec<u32>,
    stamp: Vec<u32>,
    closed: Vec<u32>,
    target: Vec<u32>,
    generation: u32,
    pub costs: SearchCosts,
    pub last: SearchStats,
}

const NONE: u32 = u32::MAX;
const DIRS: [(i64, i64); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

impl Pathfinder {
    pub fn new(costs: SearchCosts) -> Self {
        Self {
            costs,
            ..Default::default()
        }
    }

    fn ensure(&mut self, n: usize) {
        if self.g.len() != n {
            self.g = vec![0.0; n];
            self.came = vec![NONE; n];
            self.stamp = vec![0; n];
            self.closed = vec![0; n];
            self.target = vec![0; n];
            self.generation = 0;
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.closed.iter_mut().for_each(|s| *s = 0);
            self.target.iter_mut().for_each(|s| *s = 0);
            self.generation = 1;
        }
    }

    /// Cell passability + congestion cost for `net` (None = impassable).
    #[inline]
    fn cell_cost(grid: &RoutingGrid, i: usize, net: i32, pres: Option<f64>) -> Option<f64> {
        if !grid.static_ok(i, net) {
            return None;
        }
        let foreign = grid.foreign_count(i, net);
        match pres {
            None => {
                if foreign > 0 {
                    None
                } else {
                    Some(grid.history[i] as f64 * 0.0)
                }
            }
            Some(p) => Some((foreign as f64) * p * (1.0 + grid.history[i] as f64) + grid.history[i] as f64),
        }
    }

    /// Whether a through via for `net` may sit at (x, y); returns its
    /// congestion cost.
    fn via_cost(&self, grid: &RoutingGrid, x: usize, y: usize, req: &SearchRequest) -> Option<f64> {
        if grid.no_via[grid.xy_idx(x, y)] {
            return None;
        }
        let mut extra = 0.0;
        for l in 0..grid.num_layers {
            for &(dx, dy) in req.via_disc {
                let nx = x as i64 + dx;
                let ny = y as i64 + dy;
                if nx < 0 || ny < 0 || nx >= grid.cols as i64 || ny >= grid.rows as i64 {
                    return None;
                }
                let i = grid.idx(l, nx as usize, ny as usize);
                let o = grid.owner[i];
                if !(o == 0 || o == req.net) {
                    return None;
                }
                let foreign = grid.foreign_count(i, req.net);
                if foreign > 0 {
                    extra += req.present_factor? * foreign as f64 * 0.05;
                }
            }
        }
        Some(extra)
    }

    fn heuristic(grid: &RoutingGrid, i: usize, tx: (f64, f64, f64, f64), diag: f64) -> f64 {
        let per = grid.cols * grid.rows;
        let r = i % per;
        let x = (r % grid.cols) as f64;
        let y = (r / grid.cols) as f64;
        // Distance to the target bounding box (octile).
        let dx = if x < tx.0 {
            tx.0 - x
        } else if x > tx.2 {
            x - tx.2
        } else {
            0.0
        };
        let dy = if y < tx.1 {
            tx.1 - y
        } else if y > tx.3 {
            y - tx.3
        } else {
            0.0
        };
        let (mn, mx) = if dx < dy { (dx, dy) } else { (dy, dx) };
        mn * diag + (mx - mn)
    }

    /// Run A*; returns the cell path from a source to a target (inclusive).
    pub fn search(&mut self, grid: &RoutingGrid, req: &SearchRequest) -> Option<Vec<usize>> {
        let n = grid.total_cells();
        self.ensure(n);
        let gen = self.generation;
        self.last = SearchStats::default();
        if req.sources.is_empty() || req.targets.is_empty() {
            return None;
        }
        let per = grid.cols * grid.rows;
        let mut tb = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &t in req.targets {
            self.target[t] = gen;
            let r = t % per;
            let (x, y) = ((r % grid.cols) as f64, (r / grid.cols) as f64);
            tb.0 = tb.0.min(x);
            tb.1 = tb.1.min(y);
            tb.2 = tb.2.max(x);
            tb.3 = tb.3.max(y);
        }
        let mut layer_ok = vec![false; grid.num_layers];
        for &l in req.allowed_layers {
            if l < grid.num_layers {
                layer_ok[l] = true;
            }
        }
        let mut inner = vec![false; grid.num_layers];
        for &l in req.inner_layers {
            if l < grid.num_layers {
                inner[l] = true;
            }
        }
        let diag = self.costs.cost_diagonal;
        let mut heap = BinaryHeap::new();
        for &s in req.sources {
            if self.stamp[s] == gen {
                continue;
            }
            self.stamp[s] = gen;
            self.g[s] = 0.0;
            self.came[s] = NONE;
            let h = Self::heuristic(grid, s, tb, diag);
            heap.push(Node { f: h, g: 0.0, idx: s });
        }
        let mut via_cache: HashMap<usize, Option<f64>> = HashMap::new();
        let mut expansions = 0usize;
        while let Some(node) = heap.pop() {
            let cur = node.idx;
            if self.closed[cur] == gen {
                continue;
            }
            self.closed[cur] = gen;
            if self.target[cur] == gen {
                self.last = SearchStats {
                    expansions,
                    found: true,
                };
                return Some(self.reconstruct(cur));
            }
            expansions += 1;
            if expansions > req.max_expansions {
                break;
            }
            let layer = cur / per;
            let r = cur % per;
            let (x, y) = (r % grid.cols, r / grid.cols);
            let g0 = self.g[cur];
            // Direction we arrived from (for turn penalty).
            let prev = self.came[cur];
            let pdir = if prev != NONE && (prev as usize) / per == layer {
                let pr = prev as usize % per;
                let (px, py) = (pr % grid.cols, pr / grid.cols);
                Some((x as i64 - px as i64, y as i64 - py as i64))
            } else {
                None
            };
            for &(dx, dy) in &DIRS {
                let nx = x as i64 + dx;
                let ny = y as i64 + dy;
                if nx < 0 || ny < 0 || nx >= grid.cols as i64 || ny >= grid.rows as i64 {
                    continue;
                }
                let (nx, ny) = (nx as usize, ny as usize);
                if let Some((wx0, wy0, wx1, wy1)) = req.window {
                    if nx < wx0 || nx > wx1 || ny < wy0 || ny > wy1 {
                        continue;
                    }
                }
                let ni = grid.idx(layer, nx, ny);
                if self.closed[ni] == gen {
                    continue;
                }
                let Some(cc) = Self::cell_cost(grid, ni, req.net, req.present_factor) else {
                    continue;
                };
                let diagonal = dx != 0 && dy != 0;
                if diagonal {
                    // No corner cutting through blocked orthogonal neighbours.
                    let a = grid.idx(layer, nx, y);
                    let b = grid.idx(layer, x, ny);
                    if Self::cell_cost(grid, a, req.net, req.present_factor).is_none()
                        && Self::cell_cost(grid, b, req.net, req.present_factor).is_none()
                    {
                        continue;
                    }
                }
                let mut step = if diagonal {
                    self.costs.cost_diagonal
                } else {
                    self.costs.cost_straight
                };
                if let Some(pd) = pdir {
                    if pd != (dx, dy) {
                        // 45-degree turn costs half a 90-degree turn.
                        let dot = pd.0 * dx + pd.1 * dy;
                        step += if dot > 0 {
                            self.costs.cost_turn * 0.2
                        } else {
                            self.costs.cost_turn
                        };
                    }
                }
                if inner[layer] {
                    step += self.costs.cost_layer_inner * 0.1;
                }
                let ng = g0 + step + cc;
                if self.stamp[ni] != gen || ng < self.g[ni] {
                    self.stamp[ni] = gen;
                    self.g[ni] = ng;
                    self.came[ni] = cur as u32;
                    let h = Self::heuristic(grid, ni, tb, diag);
                    heap.push(Node {
                        f: ng + h,
                        g: ng,
                        idx: ni,
                    });
                }
            }
            if req.vias_allowed && grid.num_layers > 1 {
                let xy = grid.xy_idx(x, y);
                let vc = *via_cache
                    .entry(xy)
                    .or_insert_with(|| self.via_cost(grid, x, y, req));
                if let Some(vc) = vc {
                    for (l, ok) in layer_ok.iter().enumerate() {
                        if !*ok || l == layer {
                            continue;
                        }
                        let ni = grid.idx(l, x, y);
                        if self.closed[ni] == gen {
                            continue;
                        }
                        let Some(cc) = Self::cell_cost(grid, ni, req.net, req.present_factor)
                        else {
                            continue;
                        };
                        let ng = g0 + self.costs.cost_via + vc + cc;
                        if self.stamp[ni] != gen || ng < self.g[ni] {
                            self.stamp[ni] = gen;
                            self.g[ni] = ng;
                            self.came[ni] = cur as u32;
                            let h = Self::heuristic(grid, ni, tb, diag);
                            heap.push(Node {
                                f: ng + h,
                                g: ng,
                                idx: ni,
                            });
                        }
                    }
                }
            }
        }
        self.last = SearchStats {
            expansions,
            found: false,
        };
        None
    }

    fn reconstruct(&self, end: usize) -> Vec<usize> {
        let mut path = vec![end];
        let mut cur = end;
        while self.came[cur] != NONE {
            cur = self.came[cur] as usize;
            path.push(cur);
        }
        path.reverse();
        path
    }
}
