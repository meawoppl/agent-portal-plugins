//! Port of `kicad_tools.router.optimizer.serpentine`: trombone meanders for
//! length tuning.
//!
//! `LengthTracker.calculate_route_length` (router/length.py) is inlined as
//! [`route_length`]; the reservation-aware segment preference takes the
//! grid through the [`ReservationGrid`] trait.

use std::collections::{HashMap, HashSet};

use super::geometry::segment_length;
use crate::router::primitives::{Route, Segment};
use crate::router::quantize::{dogleg_points, snap_direction_8};

/// Style of serpentine pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerpentineStyle {
    Rectangular,
    Trombone,
    Sawtooth,
}

impl SerpentineStyle {
    pub fn value(self) -> &'static str {
        match self {
            SerpentineStyle::Rectangular => "rectangular",
            SerpentineStyle::Trombone => "trombone",
            SerpentineStyle::Sawtooth => "sawtooth",
        }
    }
}

/// Configuration for serpentine generation.
#[derive(Debug, Clone, PartialEq)]
pub struct SerpentineConfig {
    pub style: SerpentineStyle,
    pub amplitude: f64,
    pub min_spacing: f64,
    pub min_segment_length: f64,
    pub gap_factor: f64,
    pub max_iterations: i64,
    /// `"auto"`, `"outer"` or `"inner"` (issue #2648).
    pub side: String,
    pub outer_normal_hint: Option<(f64, f64)>,
}

impl Default for SerpentineConfig {
    fn default() -> Self {
        SerpentineConfig {
            style: SerpentineStyle::Trombone,
            amplitude: 1.0,
            min_spacing: 0.2,
            min_segment_length: 2.0,
            gap_factor: 2.0,
            max_iterations: 20,
            side: "auto".to_string(),
            outer_normal_hint: None,
        }
    }
}

/// Result of serpentine generation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SerpentineResult {
    pub success: bool,
    pub new_segments: Vec<Segment>,
    pub length_added: f64,
    pub num_loops: i64,
    pub message: String,
}

/// `RoutingGrid.is_reserved_for` surface (issue #4085).
pub trait ReservationGrid {
    fn world_to_grid(&self, x: f64, y: f64) -> (i64, i64);
    fn layer_to_index(&self, layer_value: u8) -> Option<usize>;
    fn is_reserved_for(&self, layer_idx: usize, gx: i64, gy: i64, net_id: i64) -> bool;
}

/// `LengthTracker.calculate_route_length`.
pub fn route_length(route: &Route) -> f64 {
    let mut total = 0.0;
    for seg in &route.segments {
        let dx = seg.x2 - seg.x1;
        let dy = seg.y2 - seg.y1;
        total += (dx * dx + dy * dy).sqrt();
    }
    total
}

/// Generates serpentine patterns for length tuning.
#[derive(Debug, Clone, Default)]
pub struct SerpentineGenerator {
    pub config: SerpentineConfig,
}

impl SerpentineGenerator {
    pub fn new(config: Option<SerpentineConfig>) -> Self {
        SerpentineGenerator {
            config: config.unwrap_or_default(),
        }
    }

    /// Best host segment: long, interior, axis-aligned, reserved-corridor.
    /// `fixed_segment_ids` are indices into `route.segments`.
    pub fn find_best_segment(
        &self,
        route: &Route,
        grid: Option<&dyn ReservationGrid>,
        reserved_net_id: Option<i64>,
        fixed_segment_ids: Option<&HashSet<usize>>,
    ) -> Option<(usize, Segment)> {
        let n = route.segments.len();
        let mut best: Option<(usize, f64)> = None;
        for (i, seg) in route.segments.iter().enumerate() {
            if fixed_segment_ids.is_some_and(|f| f.contains(&i)) {
                continue;
            }
            let length = segment_length(seg);
            if length < self.config.min_segment_length {
                continue;
            }
            let mut score = length;
            if i > 0 && i < n - 1 {
                score *= 1.2;
            }
            let dx = (seg.x2 - seg.x1).abs();
            let dy = (seg.y2 - seg.y1).abs();
            if length > 0.0 && (dx / length > 0.95 || dy / length > 0.95) {
                score *= 1.5;
            }
            if let (Some(g), Some(net)) = (grid, reserved_net_id) {
                if Self::segment_in_reservation(seg, g, net) {
                    score *= 2.0;
                }
            }
            if score > best.map_or(0.0, |b| b.1) {
                best = Some((i, score));
            }
        }
        best.map(|(i, _)| (i, route.segments[i].clone()))
    }

    fn segment_in_reservation(seg: &Segment, grid: &dyn ReservationGrid, net_id: i64) -> bool {
        let mx = (seg.x1 + seg.x2) / 2.0;
        let my = (seg.y1 + seg.y2) / 2.0;
        let (gx, gy) = grid.world_to_grid(mx, my);
        let Some(layer_idx) = grid.layer_to_index(seg.layer.value()) else {
            return false;
        };
        grid.is_reserved_for(layer_idx, gx, gy, net_id)
    }

    /// Trombone (U-shaped) meander replacing `segment`.
    pub fn generate_trombone(&self, segment: &Segment, target_length_add: f64) -> SerpentineResult {
        if target_length_add <= 0.0 {
            return SerpentineResult {
                success: true,
                new_segments: vec![segment.clone()],
                message: "No length addition needed".into(),
                ..Default::default()
            };
        }
        let dx = segment.x2 - segment.x1;
        let dy = segment.y2 - segment.y1;
        let length = (dx * dx + dy * dy).sqrt();
        if length < self.config.min_segment_length {
            return SerpentineResult {
                success: false,
                new_segments: vec![segment.clone()],
                message: format!(
                    "Segment too short ({length:.2}mm < {:.2}mm)",
                    self.config.min_segment_length
                ),
                ..Default::default()
            };
        }
        let (ux, uy) = snap_direction_8(dx, dy);
        let (px, py) = (-uy, ux);
        let (initial_direction, alternate) = match (self.config.side.as_str(), self.config.outer_normal_hint) {
            (side @ ("outer" | "inner"), Some((mut hx, mut hy))) => {
                let mag = (hx * hx + hy * hy).sqrt();
                if mag > 0.0 {
                    hx /= mag;
                    hy /= mag;
                }
                let dot = px * hx + py * hy;
                let dir = if side == "outer" {
                    if dot >= 0.0 { 1.0 } else { -1.0 }
                } else if dot >= 0.0 {
                    -1.0
                } else {
                    1.0
                };
                (dir, false)
            }
            _ => (1.0, true),
        };
        let amplitude = self.config.amplitude;
        let gap = self.config.min_spacing * self.config.gap_factor;
        let length_per_loop = 2.0 * amplitude;
        let mut num_loops = ((target_length_add / length_per_loop).ceil() as i64)
            .min(self.config.max_iterations);
        if num_loops <= 0 {
            return SerpentineResult {
                success: true,
                new_segments: vec![segment.clone()],
                message: "No loops needed".into(),
                ..Default::default()
            };
        }
        let total_forward = num_loops as f64 * gap * 2.0 + gap;
        if total_forward > length * 0.9 {
            num_loops = ((length * 0.9 - gap) / (2.0 * gap)) as i64;
            if num_loops <= 0 {
                return SerpentineResult {
                    success: false,
                    new_segments: vec![segment.clone()],
                    message: format!("Segment too short for serpentine ({length:.2}mm)"),
                    ..Default::default()
                };
            }
        }
        let mut out: Vec<Segment> = Vec::new();
        let (mut cx, mut cy) = (segment.x1, segment.y1);
        let step = |out: &mut Vec<Segment>, cx: &mut f64, cy: &mut f64, nx: f64, ny: f64| {
            out.push(segment.with_points(*cx, *cy, nx, ny));
            *cx = nx;
            *cy = ny;
        };
        let (nx, ny) = (cx + ux * gap, cy + uy * gap);
        step(&mut out, &mut cx, &mut cy, nx, ny);
        let mut direction = initial_direction;
        for lp in 0..num_loops {
            let (nx, ny) = (cx + px * amplitude * direction, cy + py * amplitude * direction);
            step(&mut out, &mut cx, &mut cy, nx, ny);
            let (nx, ny) = (cx + ux * gap, cy + uy * gap);
            step(&mut out, &mut cx, &mut cy, nx, ny);
            let (nx, ny) = (cx - px * amplitude * direction, cy - py * amplitude * direction);
            step(&mut out, &mut cx, &mut cy, nx, ny);
            if lp < num_loops - 1 {
                let (nx, ny) = (cx + ux * gap, cy + uy * gap);
                step(&mut out, &mut cx, &mut cy, nx, ny);
            }
            if alternate {
                direction *= -1.0;
            }
        }
        let exit = dogleg_points(cx, cy, segment.x2, segment.y2, false);
        for w in exit.windows(2) {
            if w[0] == w[1] {
                continue;
            }
            out.push(segment.with_points(w[0].0, w[0].1, w[1].0, w[1].1));
        }
        let original = segment_length(segment);
        let mut new_len = 0.0;
        for s in &out {
            new_len += segment_length(s);
        }
        let added = new_len - original;
        SerpentineResult {
            success: true,
            new_segments: out,
            length_added: added,
            num_loops,
            message: format!("Added {num_loops} loops, {added:.3}mm"),
        }
    }

    /// Add a serpentine to reach `target_length`.
    pub fn add_serpentine(&self, route: &Route, target_length: f64) -> (Route, SerpentineResult) {
        let needed = target_length - route_length(route);
        if needed <= 0.0 {
            return (
                route.clone(),
                SerpentineResult {
                    success: true,
                    new_segments: route.segments.clone(),
                    message: "Route already meets target length".into(),
                    ..Default::default()
                },
            );
        }
        let Some((idx, segment)) = self.find_best_segment(route, None, None, None) else {
            return (
                route.clone(),
                SerpentineResult {
                    success: false,
                    new_segments: route.segments.clone(),
                    message: "No suitable segment found for serpentine".into(),
                    ..Default::default()
                },
            );
        };
        let result = self.generate_trombone(&segment, needed);
        if !result.success {
            return (route.clone(), result);
        }
        let mut segs = route.segments[..idx].to_vec();
        segs.extend(result.new_segments.iter().cloned());
        segs.extend_from_slice(&route.segments[idx + 1..]);
        (
            Route::new(route.net, &route.net_name, segs, route.vias.clone()),
            result,
        )
    }
}

/// Convenience wrapper around [`SerpentineGenerator::add_serpentine`].
pub fn add_serpentine(
    route: &Route,
    target_length: f64,
    config: Option<SerpentineConfig>,
) -> (Route, SerpentineResult) {
    SerpentineGenerator::new(config).add_serpentine(route, target_length)
}

/// Extend shorter routes of a match group to the longest one's length.
pub fn tune_match_group(
    routes: &HashMap<i64, Route>,
    group_net_ids: &[i64],
    tolerance: f64,
    config: Option<SerpentineConfig>,
) -> Vec<(i64, Route, SerpentineResult)> {
    let generator = SerpentineGenerator::new(config);
    let group: Vec<(i64, &Route)> = group_net_ids
        .iter()
        .filter_map(|id| routes.get(id).map(|r| (*id, r)))
        .collect();
    let mut results = Vec::new();
    if group.len() < 2 {
        return results;
    }
    let lengths: Vec<f64> = group.iter().map(|(_, r)| route_length(r)).collect();
    let target = lengths.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    for ((net, route), current) in group.into_iter().zip(lengths) {
        if current >= target - tolerance {
            results.push((
                net,
                route.clone(),
                SerpentineResult {
                    success: true,
                    new_segments: route.segments.clone(),
                    message: "Already within tolerance".into(),
                    ..Default::default()
                },
            ));
        } else {
            let (r, res) = generator.add_serpentine(route, target);
            results.push((net, r, res));
        }
    }
    results
}
