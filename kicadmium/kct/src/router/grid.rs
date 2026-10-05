//! Routing grid (native replacement for `kicad_tools.router.grid` and the
//! C++ `Grid3D`).
//!
//! The grid stores, per layer and cell, a *static* owner computed from
//! obstacles (pads, board edge, keepouts, fixed copper) and a *dynamic*
//! occupancy list of routed nets whose clearance halo covers the cell.
//!
//! A cell value describes where a trace **centerline** may pass:
//! - static `FREE` (0): any net may use it;
//! - static `n > 0`: only net `n` may use it (inside another obstacle's halo
//!   only when that obstacle is net `n`);
//! - static `BLOCKED` (-1): nobody may route here.
//!
//! Pad "cores" (cells whose centre lies inside the pad shrunk by half a
//! trace width) are always passable for the pad's own net, even when a
//! neighbouring foreign pad's halo covers them: copper there stays inside
//! the pad itself.

use std::collections::HashMap;

type OutlineSegment = ((f64, f64), (f64, f64));

/// Static cell owner: blocked for everyone.
pub const BLOCKED: i32 = -1;
/// Static cell owner: free for everyone.
pub const FREE: i32 = 0;

/// Rounded-rectangle copper shape in board coordinates (covers rect,
/// roundrect, oval and circle pads, and via discs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PadShape {
    pub cx: f64,
    pub cy: f64,
    /// Half extents in the shape's local frame.
    pub hx: f64,
    pub hy: f64,
    /// Corner radius (<= min(hx, hy)).
    pub r: f64,
    /// Rotation in degrees (board frame, KiCad convention).
    pub rot: f64,
}

impl PadShape {
    pub fn circle(cx: f64, cy: f64, radius: f64) -> Self {
        Self {
            cx,
            cy,
            hx: radius,
            hy: radius,
            r: radius,
            rot: 0.0,
        }
    }

    /// Signed distance from (x, y) to the shape boundary (negative inside).
    pub fn distance(&self, x: f64, y: f64) -> f64 {
        let (mut dx, mut dy) = (x - self.cx, y - self.cy);
        if self.rot != 0.0 {
            // Inverse of KiCad's negated-angle rotation.
            let a = self.rot.to_radians();
            let (s, c) = a.sin_cos();
            let (lx, ly) = (dx * c - dy * s, dx * s + dy * c);
            dx = lx;
            dy = ly;
        }
        let qx = dx.abs() - (self.hx - self.r);
        let qy = dy.abs() - (self.hy - self.r);
        let ox = qx.max(0.0);
        let oy = qy.max(0.0);
        (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - self.r
    }

    /// Axis-aligned bounding box (min_x, min_y, max_x, max_y).
    pub fn bbox(&self) -> (f64, f64, f64, f64) {
        let a = self.rot.to_radians();
        let (s, c) = (a.sin().abs(), a.cos().abs());
        let ex = c * self.hx + s * self.hy;
        let ey = s * self.hx + c * self.hy;
        (self.cx - ex, self.cy - ey, self.cx + ex, self.cy + ey)
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.distance(x, y) <= 0.0
    }
}

/// Grid cell coordinate.
pub type Cell = (usize, usize, usize); // (layer, x, y)

/// Multi-layer uniform routing grid.
#[derive(Debug, Clone)]
pub struct RoutingGrid {
    pub cols: usize,
    pub rows: usize,
    pub num_layers: usize,
    pub resolution: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    /// Static owner per cell (layer-major).
    pub owner: Vec<i32>,
    /// Pad-core owner per cell (0 = none).
    pub core: Vec<i32>,
    /// Routed nets whose halo covers the cell: (net, count).
    /// First routed net covering the cell (net 0 = none) and its count.
    pub dynamic: Vec<(i32, u16)>,
    /// Further nets covering multi-occupied cells.
    pub dynamic_extra: HashMap<usize, Vec<(i32, u16)>>,
    /// Negotiated-congestion history cost per cell.
    pub history: Vec<f32>,
    /// Cells where vias are not allowed (in addition to owner checks).
    pub no_via: Vec<bool>,
    /// Committed/pending via holes whose drill keep-out covers the cell
    /// (hole-to-hole spacing, any net).
    pub via_block: Vec<u16>,
}

impl RoutingGrid {
    pub fn new(
        width: f64,
        height: f64,
        origin: (f64, f64),
        resolution: f64,
        layers: usize,
    ) -> Self {
        let cols = (width / resolution).ceil() as usize + 1;
        let rows = (height / resolution).ceil() as usize + 1;
        let n = cols * rows * layers;
        Self {
            cols,
            rows,
            num_layers: layers,
            resolution,
            origin_x: origin.0,
            origin_y: origin.1,
            owner: vec![FREE; n],
            core: vec![0; n],
            dynamic: vec![(0, 0); n],
            dynamic_extra: HashMap::new(),
            history: vec![0.0; n],
            no_via: vec![false; cols * rows],
            via_block: vec![0; cols * rows],
        }
    }

    #[inline]
    pub fn idx(&self, layer: usize, x: usize, y: usize) -> usize {
        (layer * self.rows + y) * self.cols + x
    }

    #[inline]
    pub fn xy_idx(&self, x: usize, y: usize) -> usize {
        y * self.cols + x
    }

    pub fn total_cells(&self) -> usize {
        self.cols * self.rows * self.num_layers
    }

    #[inline]
    pub fn grid_to_world(&self, x: usize, y: usize) -> (f64, f64) {
        (
            self.origin_x + x as f64 * self.resolution,
            self.origin_y + y as f64 * self.resolution,
        )
    }

    /// Nearest grid cell to a world point (clamped).
    pub fn world_to_grid(&self, wx: f64, wy: f64) -> (usize, usize) {
        let gx = ((wx - self.origin_x) / self.resolution).round();
        let gy = ((wy - self.origin_y) / self.resolution).round();
        (
            gx.clamp(0.0, (self.cols - 1) as f64) as usize,
            gy.clamp(0.0, (self.rows - 1) as f64) as usize,
        )
    }

    /// Cell index range covering world box expanded by `margin`.
    pub fn cell_range(
        &self,
        bbox: (f64, f64, f64, f64),
        margin: f64,
    ) -> (usize, usize, usize, usize) {
        let g = self.resolution;
        let fx0 = ((bbox.0 - margin - self.origin_x) / g).floor().max(0.0) as usize;
        let fy0 = ((bbox.1 - margin - self.origin_y) / g).floor().max(0.0) as usize;
        let fx1 =
            (((bbox.2 + margin - self.origin_x) / g).ceil().max(0.0) as usize).min(self.cols - 1);
        let fy1 =
            (((bbox.3 + margin - self.origin_y) / g).ceil().max(0.0) as usize).min(self.rows - 1);
        (fx0, fy0, fx1, fy1)
    }

    /// Merge an obstacle owner into a cell: same net keeps it, conflicting
    /// nets (or a hard obstacle) block it.
    #[inline]
    fn merge_owner(&mut self, i: usize, net: i32) {
        let cur = self.owner[i];
        self.owner[i] = if net <= 0 {
            BLOCKED
        } else if cur == FREE || cur == net {
            net
        } else {
            BLOCKED
        };
    }

    /// Mark the halo of a copper shape: every cell whose centre is within
    /// `halo` of the shape gets owner `net` (BLOCKED for `net <= 0`).
    pub fn add_shape_halo(&mut self, shape: &PadShape, layers: &[usize], net: i32, halo: f64) {
        let (x0, y0, x1, y1) = self.cell_range(shape.bbox(), halo + self.resolution);
        for &l in layers {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (wx, wy) = self.grid_to_world(x, y);
                    if shape.distance(wx, wy) < halo {
                        let i = self.idx(l, x, y);
                        self.merge_owner(i, net);
                    }
                }
            }
        }
    }

    /// Mark pad core cells for `net` (inside the shape by at least `inset`).
    pub fn add_core(&mut self, shape: &PadShape, layers: &[usize], net: i32, inset: f64) -> usize {
        let (x0, y0, x1, y1) = self.cell_range(shape.bbox(), self.resolution);
        let mut n = 0;
        for &l in layers {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (wx, wy) = self.grid_to_world(x, y);
                    if shape.distance(wx, wy) <= -inset {
                        let i = self.idx(l, x, y);
                        self.core[i] = net;
                        n += 1;
                    }
                }
            }
        }
        n
    }

    /// Mark the halo of a segment (capsule of half-width `halo`).
    pub fn add_segment_halo(
        &mut self,
        a: (f64, f64),
        b: (f64, f64),
        layers: &[usize],
        net: i32,
        halo: f64,
    ) {
        let bbox = (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
        let (x0, y0, x1, y1) = self.cell_range(bbox, halo + self.resolution);
        for &l in layers {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (wx, wy) = self.grid_to_world(x, y);
                    if point_segment_distance(wx, wy, a, b) < halo {
                        let i = self.idx(l, x, y);
                        self.merge_owner(i, net);
                    }
                }
            }
        }
    }

    /// Block every cell outside `inside` or within `margin` of the outline.
    pub fn add_board_edge(&mut self, outline: &[OutlineSegment], margin: f64) {
        if outline.is_empty() {
            return;
        }
        for y in 0..self.rows {
            for x in 0..self.cols {
                let (wx, wy) = self.grid_to_world(x, y);
                let inside = point_in_outline(wx, wy, outline);
                let blocked = !inside
                    || outline
                        .iter()
                        .any(|(a, b)| point_segment_distance(wx, wy, *a, *b) < margin);
                if blocked {
                    for l in 0..self.num_layers {
                        let i = self.idx(l, x, y);
                        self.owner[i] = BLOCKED;
                    }
                }
            }
        }
    }

    /// Whether net `net` may put a centerline on the cell, ignoring routed
    /// copper.
    #[inline]
    pub fn static_ok(&self, i: usize, net: i32) -> bool {
        let o = self.owner[i];
        o == FREE || o == net || self.core[i] == net
    }

    /// Number of foreign nets whose routed halo covers the cell.
    #[inline]
    pub fn foreign_count(&self, i: usize, net: i32) -> usize {
        let (n0, _) = self.dynamic[i];
        if n0 == 0 {
            return 0;
        }
        let mut c = usize::from(n0 != net);
        if let Some(extra) = self.dynamic_extra.get(&i) {
            c += extra.iter().filter(|(n, _)| *n != net).count();
        }
        c
    }

    pub fn add_dynamic(&mut self, i: usize, net: i32) {
        let slot = &mut self.dynamic[i];
        if slot.0 == 0 {
            *slot = (net, 1);
        } else if slot.0 == net {
            slot.1 = slot.1.saturating_add(1);
        } else {
            let v = self.dynamic_extra.entry(i).or_default();
            if let Some(e) = v.iter_mut().find(|(n, _)| *n == net) {
                e.1 = e.1.saturating_add(1);
            } else {
                v.push((net, 1));
            }
        }
    }

    pub fn remove_dynamic(&mut self, i: usize, net: i32) {
        if self.dynamic[i].0 == net {
            if self.dynamic[i].1 > 1 {
                self.dynamic[i].1 -= 1;
                return;
            }
            // Promote an overflow occupant into the primary slot.
            let promoted = match self.dynamic_extra.get_mut(&i) {
                Some(v) => {
                    let e = v.pop();
                    if v.is_empty() {
                        self.dynamic_extra.remove(&i);
                    }
                    e
                }
                None => None,
            };
            self.dynamic[i] = promoted.unwrap_or((0, 0));
            return;
        }
        if let Some(v) = self.dynamic_extra.get_mut(&i) {
            if let Some(pos) = v.iter().position(|(n, _)| *n == net) {
                if v[pos].1 > 1 {
                    v[pos].1 -= 1;
                } else {
                    v.swap_remove(pos);
                }
            }
            if v.is_empty() {
                self.dynamic_extra.remove(&i);
            }
        }
    }

    /// Cells covered by a capsule (for dynamic marking).
    pub fn capsule_cells(
        &self,
        a: (f64, f64),
        b: (f64, f64),
        layer: usize,
        halo: f64,
    ) -> Vec<usize> {
        let bbox = (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
        let (x0, y0, x1, y1) = self.cell_range(bbox, halo + self.resolution);
        let mut out = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (wx, wy) = self.grid_to_world(x, y);
                if point_segment_distance(wx, wy, a, b) < halo {
                    out.push(self.idx(layer, x, y));
                }
            }
        }
        out
    }

    /// Add (`delta` = +1) or remove (-1) a via hole keep-out disc.
    pub fn block_via_hole(&mut self, x: f64, y: f64, radius: f64, delta: i32) {
        let (x0, y0, x1, y1) = self.cell_range((x, y, x, y), radius + self.resolution);
        for gy in y0..=y1 {
            for gx in x0..=x1 {
                let (wx, wy) = self.grid_to_world(gx, gy);
                if (wx - x).hypot(wy - y) < radius {
                    let i = self.xy_idx(gx, gy);
                    let v = self.via_block[i] as i32 + delta;
                    self.via_block[i] = v.max(0) as u16;
                }
            }
        }
    }

    /// Disc offsets (dx, dy) with centre distance < radius (in cells).
    pub fn disc_offsets(radius_cells: f64) -> Vec<(i64, i64)> {
        let r = radius_cells.ceil() as i64;
        let mut out = Vec::new();
        for dy in -r..=r {
            for dx in -r..=r {
                if ((dx * dx + dy * dy) as f64).sqrt() < radius_cells {
                    out.push((dx, dy));
                }
            }
        }
        out
    }
}

/// Distance from point to segment.
#[inline]
pub fn point_segment_distance(px: f64, py: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Even-odd point-in-outline test over unordered edge segments.
pub fn point_in_outline(px: f64, py: f64, outline: &[OutlineSegment]) -> bool {
    let mut inside = false;
    for &((x1, y1), (x2, y2)) in outline {
        if (y1 > py) != (y2 > py) {
            let xi = x1 + (py - y1) * (x2 - x1) / (y2 - y1);
            if px < xi {
                inside = !inside;
            }
        }
    }
    inside
}

/// Count of cells per static owner class (for statistics output).
pub fn blocked_cell_count(grid: &RoutingGrid) -> usize {
    grid.owner.iter().filter(|&&o| o == BLOCKED).count() / grid.num_layers.max(1)
}

/// Index helper used by callers that keep per-net cell sets.
pub type CellSet = HashMap<usize, ()>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_distance() {
        let s = PadShape {
            cx: 0.0,
            cy: 0.0,
            hx: 1.0,
            hy: 0.5,
            r: 0.0,
            rot: 0.0,
        };
        assert!((s.distance(2.0, 0.0) - 1.0).abs() < 1e-12);
        assert!(s.distance(0.0, 0.0) < 0.0);
        let r = PadShape { rot: 90.0, ..s };
        assert!((r.distance(0.0, 2.0) - 1.0).abs() < 1e-9);
        let c = PadShape::circle(0.0, 0.0, 1.0);
        assert!((c.distance(3.0, 4.0) - 4.0).abs() < 1e-12);
    }

    #[test]
    fn outline_inside() {
        let sq = vec![
            ((0.0, 0.0), (10.0, 0.0)),
            ((10.0, 0.0), (10.0, 10.0)),
            ((10.0, 10.0), (0.0, 10.0)),
            ((0.0, 10.0), (0.0, 0.0)),
        ];
        assert!(point_in_outline(5.0, 5.0, &sq));
        assert!(!point_in_outline(15.0, 5.0, &sq));
    }

    #[test]
    fn halo_merge() {
        let mut g = RoutingGrid::new(2.0, 2.0, (0.0, 0.0), 0.1, 1);
        let a = PadShape::circle(0.5, 1.0, 0.2);
        let b = PadShape::circle(1.5, 1.0, 0.2);
        g.add_shape_halo(&a, &[0], 1, 0.25);
        g.add_shape_halo(&b, &[0], 2, 0.25);
        let i = g.idx(0, 5, 10);
        assert_eq!(g.owner[i], 1);
        let mid = g.idx(0, 10, 10);
        assert_eq!(g.owner[mid], FREE);
        assert!(g.static_ok(mid, 3));
    }
}
