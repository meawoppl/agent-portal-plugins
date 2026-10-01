//! Port of `kicad_tools.router.optimizer.collision`: path-clearance checkers
//! used by the optimizer passes.
//!
//! Upstream's checkers read `RoutingGrid` internals directly. The router
//! core is ported separately, so the grid surface the checkers need is the
//! [`CollisionGrid`] trait; `RoutingGrid` implements it once it lands.

use crate::core::geometry::{point_to_segment_distance, segment_to_segment_distance};
use crate::router::layers::Layer;
use crate::router::primitives::{Route, Segment, Via};

/// Per-cell occupancy the checkers read (`grid.cell_at(layer, y, x)`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CellInfo {
    pub blocked: bool,
    pub is_obstacle: bool,
    pub pad_blocked: bool,
    pub net: i64,
    pub usage_count: i64,
}

/// The `RoutingGrid` surface used by [`GridCollisionChecker`] and
/// [`VectorCollisionChecker`].
pub trait CollisionGrid {
    fn world_to_grid(&self, x: f64, y: f64) -> (i64, i64);
    /// Grid layer index for a `Layer` value; `None` when unmapped.
    fn layer_to_index(&self, layer_value: u8) -> Option<usize>;
    fn resolution(&self) -> f64;
    fn cols(&self) -> i64;
    fn rows(&self) -> i64;
    fn trace_clearance(&self) -> f64;
    fn via_clearance(&self) -> f64;
    fn cell_at(&self, layer_idx: usize, gy: i64, gx: i64) -> CellInfo;
    fn routes(&self) -> &[Route];
    /// `grid._route_halo.cell_known(...) is True` (`None` when no halo).
    fn route_halo_cell_known(&self, _gx: i64, _gy: i64, _layer_idx: usize) -> Option<bool> {
        None
    }
    /// `grid.raster_only_blocked_cell(...)` (`None` when unsupported).
    fn raster_only_blocked_cell(&self, _gx: i64, _gy: i64, _layer_idx: usize) -> Option<bool> {
        None
    }
    /// R-tree broad phase over routed segments on a layer; `None` when the
    /// index is unavailable for that layer.
    fn segment_rtree_query(
        &self,
        _layer_idx: usize,
        _envelope: (f64, f64, f64, f64),
    ) -> Option<Vec<Segment>> {
        None
    }
    /// R-tree broad phase over vias; `None` when not built.
    fn via_rtree_query(&self, _envelope: (f64, f64, f64, f64)) -> Option<Vec<Via>> {
        None
    }
    /// `grid._rtree_available and grid._seg_rtree_count > 0`.
    fn rtree_populated(&self) -> bool {
        false
    }
}

/// Protocol: is the path from `(x1, y1)` to `(x2, y2)` clear?
pub trait CollisionChecker {
    #[allow(clippy::too_many_arguments)]
    fn path_is_clear(
        &self,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        layer: Layer,
        width: f64,
        exclude_net: i64,
    ) -> bool;
}

/// Cells within `clearance` (Chebyshev) of the Bresenham line (#5240).
pub fn iter_dilated_line_cells(
    gx1: i64,
    gy1: i64,
    gx2: i64,
    gy2: i64,
    clearance: i64,
) -> Vec<(i64, i64)> {
    let dx = (gx2 - gx1).abs();
    let dy = (gy2 - gy1).abs();
    let sx = if gx1 < gx2 { 1 } else { -1 };
    let sy = if gy1 < gy2 { 1 } else { -1 };
    let mut err = dx - dy;
    let (mut gx, mut gy) = (gx1, gy1);
    let c = clearance;
    let mut out = Vec::new();
    for cy in -c..=c {
        for cx in -c..=c {
            out.push((gx + cx, gy + cy));
        }
    }
    while !(gx == gx2 && gy == gy2) {
        let e2 = 2 * err;
        let (mut step_x, mut step_y) = (0, 0);
        if e2 > -dy {
            err -= dy;
            gx += sx;
            step_x = sx;
        }
        if e2 < dx {
            err += dx;
            gy += sy;
            step_y = sy;
        }
        if step_x != 0 {
            let lead_x = gx + step_x * c;
            for cy in -c..=c {
                out.push((lead_x, gy + cy));
            }
        }
        if step_y != 0 {
            let lead_y = gy + step_y * c;
            for cx in -c..=c {
                out.push((gx + cx, lead_y));
            }
        }
    }
    out
}

fn path_clear_of_segment(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    half_width: f64,
    other: &Segment,
    min_clearance: f64,
) -> bool {
    let dist = segment_to_segment_distance(x1, y1, x2, y2, other.x1, other.y1, other.x2, other.y2);
    dist - half_width - other.width / 2.0 >= min_clearance
}

fn path_clear_of_via(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    half_width: f64,
    via: &Via,
    via_clearance: f64,
) -> bool {
    let dist = point_to_segment_distance(via.x, via.y, x1, y1, x2, y2);
    dist - half_width - via.diameter / 2.0 >= via_clearance
}

fn via_spans_layer(grid: &dyn CollisionGrid, via: &Via, layer_idx: usize) -> bool {
    let (Some(a), Some(b)) = (
        grid.layer_to_index(via.layers.0.value()),
        grid.layer_to_index(via.layers.1.value()),
    ) else {
        return true;
    };
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    lo <= layer_idx && layer_idx <= hi
}

fn soft_cell_is_accountable(grid: &dyn CollisionGrid, gx: i64, gy: i64, layer_idx: usize) -> bool {
    let Some(known) = grid.route_halo_cell_known(gx, gy, layer_idx) else {
        return false;
    };
    if grid.raster_only_blocked_cell(gx, gy, layer_idx) == Some(true) {
        return false;
    }
    known
}

#[allow(clippy::too_many_arguments)]
fn routed_copper_clear(
    grid: &dyn CollisionGrid,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    layer: Layer,
    layer_idx: usize,
    width: f64,
    exclude_net: i64,
) -> bool {
    let half_width = width / 2.0;
    let min_clearance = grid.trace_clearance();
    let via_clearance = min_clearance.max(grid.via_clearance());
    for route in grid.routes() {
        for seg in &route.segments {
            if seg.net == exclude_net || seg.layer != layer {
                continue;
            }
            if !path_clear_of_segment(x1, y1, x2, y2, half_width, seg, min_clearance) {
                return false;
            }
        }
        for via in &route.vias {
            if via.net == exclude_net || !via_spans_layer(grid, via, layer_idx) {
                continue;
            }
            if !path_clear_of_via(x1, y1, x2, y2, half_width, via, via_clearance) {
                return false;
            }
        }
    }
    true
}

/// Raster (Bresenham + clearance window) checker over the routing grid.
pub struct GridCollisionChecker<'a> {
    pub grid: &'a dyn CollisionGrid,
    pub ignore_overflow: bool,
}

impl<'a> GridCollisionChecker<'a> {
    pub fn new(grid: &'a dyn CollisionGrid, ignore_overflow: bool) -> Self {
        GridCollisionChecker {
            grid,
            ignore_overflow,
        }
    }

    /// Deduplicated cells along a path with clearance buffer.
    pub fn get_path_cells(
        &self,
        gx1: i64,
        gy1: i64,
        gx2: i64,
        gy2: i64,
        clearance: i64,
    ) -> Vec<(i64, i64)> {
        let mut seen = std::collections::HashSet::new();
        iter_dilated_line_cells(gx1, gy1, gx2, gy2, clearance)
            .into_iter()
            .filter(|c| seen.insert(*c))
            .collect()
    }
}

impl CollisionChecker for GridCollisionChecker<'_> {
    fn path_is_clear(
        &self,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        layer: Layer,
        width: f64,
        exclude_net: i64,
    ) -> bool {
        let grid = self.grid;
        let (gx1, gy1) = grid.world_to_grid(x1, y1);
        let (gx2, gy2) = grid.world_to_grid(x2, y2);
        let total_clearance = width / 2.0 + grid.trace_clearance();
        let clearance_cells = (total_clearance / grid.resolution()) as i64 + 1;
        let Some(layer_idx) = grid.layer_to_index(layer.value()) else {
            return false;
        };
        let mut needs_exact = false;
        for (gx, gy) in self.get_path_cells(gx1, gy1, gx2, gy2, clearance_cells) {
            if !(0 <= gx && gx < grid.cols() && 0 <= gy && gy < grid.rows()) {
                continue;
            }
            let cell = grid.cell_at(layer_idx, gy, gx);
            if cell.blocked {
                if cell.is_obstacle && cell.net != exclude_net {
                    return false;
                }
                if cell.pad_blocked && cell.net != exclude_net {
                    return false;
                }
                if cell.net != 0 && cell.net != exclude_net {
                    if self.ignore_overflow && cell.usage_count > 1 {
                        continue;
                    }
                    if soft_cell_is_accountable(grid, gx, gy, layer_idx) {
                        needs_exact = true;
                        continue;
                    }
                    return false;
                }
            }
        }
        if needs_exact
            && !routed_copper_clear(grid, x1, y1, x2, y2, layer, layer_idx, width, exclude_net)
        {
            return false;
        }
        true
    }
}

/// R-tree broad phase + exact vector narrow phase checker.
pub struct VectorCollisionChecker<'a> {
    pub grid: &'a dyn CollisionGrid,
    pub ignore_overflow: bool,
}

impl<'a> VectorCollisionChecker<'a> {
    pub fn new(grid: &'a dyn CollisionGrid, ignore_overflow: bool) -> Self {
        VectorCollisionChecker {
            grid,
            ignore_overflow,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_obstacles_clear(
        &self,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        layer_idx: usize,
        width: f64,
        exclude_net: i64,
    ) -> bool {
        let grid = self.grid;
        let (gx1, gy1) = grid.world_to_grid(x1, y1);
        let (gx2, gy2) = grid.world_to_grid(x2, y2);
        let total_clearance = width / 2.0 + grid.trace_clearance();
        let clearance_cells = (total_clearance / grid.resolution()) as i64 + 1;
        let (cols, rows) = (grid.cols(), grid.rows());
        for (cx, cy) in iter_dilated_line_cells(gx1, gy1, gx2, gy2, clearance_cells) {
            if !(0 <= cx && cx < cols && 0 <= cy && cy < rows) {
                continue;
            }
            let cell = grid.cell_at(layer_idx, cy, cx);
            if !cell.blocked {
                continue;
            }
            if cell.is_obstacle || cell.pad_blocked {
                if cell.net != 0 && cell.net == exclude_net {
                    continue;
                }
                if cell.pad_blocked && cell.net == exclude_net {
                    continue;
                }
                return false;
            }
        }
        true
    }
}

impl CollisionChecker for VectorCollisionChecker<'_> {
    fn path_is_clear(
        &self,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        layer: Layer,
        width: f64,
        exclude_net: i64,
    ) -> bool {
        let grid = self.grid;
        let Some(layer_idx) = grid.layer_to_index(layer.value()) else {
            return false;
        };
        let min_clearance = grid.trace_clearance();
        let half_width = width / 2.0;
        let r = half_width + min_clearance;
        let envelope = (x1.min(x2) - r, y1.min(y2) - r, x1.max(x2) + r, y1.max(y2) + r);
        let Some(candidates) = grid.segment_rtree_query(layer_idx, envelope) else {
            return GridCollisionChecker::new(grid, self.ignore_overflow)
                .path_is_clear(x1, y1, x2, y2, layer, width, exclude_net);
        };
        for other in &candidates {
            if other.net == exclude_net {
                continue;
            }
            if !path_clear_of_segment(x1, y1, x2, y2, half_width, other, min_clearance) {
                return false;
            }
        }
        let via_clearance = min_clearance.max(grid.via_clearance());
        let vr = half_width + via_clearance;
        let via_env = (x1.min(x2) - vr, y1.min(y2) - vr, x1.max(x2) + vr, y1.max(y2) + vr);
        match grid.via_rtree_query(via_env) {
            Some(vias) if !vias.is_empty() => {
                for via in &vias {
                    if via.net == exclude_net || !via_spans_layer(grid, via, layer_idx) {
                        continue;
                    }
                    if !path_clear_of_via(x1, y1, x2, y2, half_width, via, via_clearance) {
                        return false;
                    }
                }
            }
            _ => {
                for route in grid.routes() {
                    if route.net == exclude_net {
                        continue;
                    }
                    for via in &route.vias {
                        if !via_spans_layer(grid, via, layer_idx) {
                            continue;
                        }
                        if !path_clear_of_via(x1, y1, x2, y2, half_width, via, via_clearance) {
                            return false;
                        }
                    }
                }
            }
        }
        self.check_obstacles_clear(x1, y1, x2, y2, layer_idx, width, exclude_net)
    }
}

/// Pick the vector checker when the grid's R-tree is populated.
pub fn make_collision_checker<'a>(
    grid: &'a dyn CollisionGrid,
    ignore_overflow: bool,
) -> Box<dyn CollisionChecker + 'a> {
    if grid.rtree_populated() {
        Box::new(VectorCollisionChecker::new(grid, ignore_overflow))
    } else {
        Box::new(GridCollisionChecker::new(grid, ignore_overflow))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dilated_cells_cover_window() {
        let cells: std::collections::HashSet<_> =
            iter_dilated_line_cells(0, 0, 3, 1, 1).into_iter().collect();
        for x in -1..=4 {
            for y in -1..=2 {
                let on_line = cells.contains(&(x, y));
                // Every cell within distance 1 of the Bresenham line is present.
                if (0..=1).contains(&y) || y == -1 && x <= 2 || y == 2 && x >= 1 {
                    assert!(on_line, "missing {x},{y}");
                }
            }
        }
    }
}
