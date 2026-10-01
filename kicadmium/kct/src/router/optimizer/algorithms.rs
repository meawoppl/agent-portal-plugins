//! Port of `kicad_tools.router.optimizer.algorithms`: the per-chain trace
//! cleanup passes.

use super::config::OptimizationConfig;
use super::geometry::SegmentExt;
use super::geometry::{
    is_90_degree_corner, is_connected, is_zigzag, perpendicular_direction, project_point_onto_line,
    same_direction, segment_direction, segment_length, shorten_segment_end, shorten_segment_start,
    translate_segment,
};
use crate::router::primitives::Segment;
use crate::router::quantize::dogleg;

/// Optional collision predicate (`path_is_clear`).
pub type PathIsClear<'a> = Option<&'a dyn Fn(&Segment) -> bool>;

fn clear(path_is_clear: PathIsClear<'_>, seg: &Segment) -> bool {
    path_is_clear.is_none_or(|f| f(seg))
}

/// Merge adjacent connected, same-direction, same-layer/net segments.
pub fn merge_collinear(
    segments: &[Segment],
    config: &OptimizationConfig,
    path_is_clear: PathIsClear<'_>,
) -> Vec<Segment> {
    if segments.len() < 2 {
        return segments.to_vec();
    }
    let mut result = Vec::new();
    let mut current = segments[0].clone();
    for next in &segments[1..] {
        if is_connected(&current, next, config.tolerance)
            && same_direction(&current, next, config.tolerance)
            && current.layer == next.layer
            && current.net == next.net
        {
            let merged = current.with_points(current.x1, current.y1, next.x2, next.y2);
            if clear(path_is_clear, &merged) {
                current = merged;
            } else {
                result.push(current);
                current = next.clone();
            }
        } else {
            result.push(current);
            current = next.clone();
        }
    }
    result.push(current);
    result
}

/// Remove back-and-forth detours when the shortcut is clear.
pub fn eliminate_zigzags(
    segments: &[Segment],
    config: &OptimizationConfig,
    path_is_clear: PathIsClear<'_>,
) -> Vec<Segment> {
    if segments.len() < 3 {
        return segments.to_vec();
    }
    let mut result = vec![segments[0].clone()];
    let mut i = 1;
    while i < segments.len() - 1 {
        let prev = result.last().expect("non-empty").clone();
        let curr = &segments[i];
        let next = &segments[i + 1];
        if is_zigzag(&prev, curr, next, config.tolerance) {
            let shortcut = prev.with_points(prev.x1, prev.y1, curr.x2, curr.y2);
            if clear(path_is_clear, &shortcut) {
                *result.last_mut().expect("non-empty") = shortcut;
            } else {
                result.push(curr.clone());
            }
        } else {
            result.push(curr.clone());
        }
        i += 1;
    }
    result.push(segments[segments.len() - 1].clone());
    result
}

/// Replace alternating-direction staircases with an optimal 1-2 leg path.
pub fn compress_staircase(
    segments: &[Segment],
    config: &OptimizationConfig,
    path_is_clear: PathIsClear<'_>,
) -> Vec<Segment> {
    if !config.compress_staircase || segments.len() < config.min_staircase_segments {
        return segments.to_vec();
    }
    let mut result = Vec::new();
    let mut i = 0;
    while i < segments.len() {
        let end = find_staircase_end(segments, i, config);
        if end - i >= config.min_staircase_segments {
            let start_point = (segments[i].x1, segments[i].y1);
            let end_point = (segments[end - 1].x2, segments[end - 1].y2);
            let replacement = optimal_path(start_point, end_point, &segments[i], config);
            let all_clear = replacement.iter().all(|s| clear(path_is_clear, s));
            if all_clear && !replacement.is_empty() {
                result.extend(replacement);
            } else {
                result.extend_from_slice(&segments[i..end]);
            }
            i = end;
        } else {
            result.push(segments[i].clone());
            i += 1;
        }
    }
    result
}

/// End index (exclusive) of the staircase starting at `start_idx`.
pub fn find_staircase_end(
    segments: &[Segment],
    start_idx: usize,
    config: &OptimizationConfig,
) -> usize {
    if start_idx + 1 >= segments.len() {
        return start_idx + 1;
    }
    let dir1 = segment_direction(&segments[start_idx], config.tolerance);
    let dir2 = segment_direction(&segments[start_idx + 1], config.tolerance);
    let mut angle_diff = (dir1 - dir2).abs();
    if angle_diff > 180.0 {
        angle_diff = 360.0 - angle_diff;
    }
    let diagonal = (30.0..=60.0).contains(&angle_diff);
    let rectilinear = (75.0..=105.0).contains(&angle_diff);
    if !(diagonal || rectilinear) {
        return start_idx + 1;
    }
    let mut i = start_idx + 2;
    while i < segments.len() {
        let dir_i = segment_direction(&segments[i], config.tolerance);
        let expected = if (i - start_idx).is_multiple_of(2) {
            dir1
        } else {
            dir2
        };
        let mut diff = (dir_i - expected).abs();
        if diff > 180.0 {
            diff = 360.0 - diff;
        }
        if diff > 15.0 {
            break;
        }
        i += 1;
    }
    i
}

/// Diagonal-then-orthogonal path from `start` to `end`.
pub fn optimal_path(
    start: (f64, f64),
    end: (f64, f64),
    template: &Segment,
    config: &OptimizationConfig,
) -> Vec<Segment> {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    if dx.abs() < config.tolerance && dy.abs() < config.tolerance {
        return Vec::new();
    }
    let diag = dx.abs().min(dy.abs());
    let mid_x = start.0 + diag.copysign(dx);
    let mid_y = start.1 + diag.copysign(dy);
    let mut result = Vec::new();
    if diag > config.tolerance {
        result.push(template.with_points(start.0, start.1, mid_x, mid_y));
    }
    let rdx = end.0 - mid_x;
    let rdy = end.1 - mid_y;
    if rdx.abs() > config.tolerance || rdy.abs() > config.tolerance {
        result.push(template.with_points(mid_x, mid_y, end.0, end.1));
    }
    if result.is_empty() {
        result.push(template.with_points(start.0, start.1, end.0, end.1));
    }
    result
}

/// Chamfer 90-degree corners with 45-degree legs, restoring terminals.
pub fn convert_corners_45(
    segments: &[Segment],
    config: &OptimizationConfig,
    path_is_clear: PathIsClear<'_>,
    pad_positions: Option<&[(f64, f64)]>,
) -> Vec<Segment> {
    if segments.len() < 2 {
        return segments.to_vec();
    }
    let orig_start = (segments[0].x1, segments[0].y1);
    let last_idx = segments.len() - 1;
    let orig_end = (segments[last_idx].x2, segments[last_idx].y2);
    let chamfer = config.corner_chamfer_size;
    let mut result: Vec<Segment> = Vec::new();

    for (i, seg) in segments.iter().enumerate() {
        if i == 0 {
            let next = &segments[1];
            if is_90_degree_corner(seg, next) {
                match shorten_segment_end(seg, chamfer, config.min_segment_length) {
                    Some(s) => result.push(s),
                    None => result.push(seg.clone()),
                }
            } else {
                result.push(seg.clone());
            }
        } else if i == last_idx {
            let prev = &segments[i - 1];
            if is_90_degree_corner(prev, seg) {
                let shortened = shorten_segment_start(seg, chamfer, config.min_segment_length);
                match (shortened, result.last()) {
                    (Some(shortened), Some(last)) => {
                        let chamfer_seg =
                            seg.with_points(last.x2, last.y2, shortened.x1, shortened.y1);
                        if clear(path_is_clear, &chamfer_seg) {
                            result.push(chamfer_seg);
                            result.push(shortened);
                        } else {
                            result.push(seg.clone());
                        }
                    }
                    _ => result.push(seg.clone()),
                }
            } else {
                result.push(seg.clone());
            }
        } else {
            let prev = &segments[i - 1];
            let next = &segments[i + 1];
            let mut modified = seg.clone();
            if is_90_degree_corner(prev, seg) && !result.is_empty() {
                if let Some(shortened) =
                    shorten_segment_start(&modified, chamfer, config.min_segment_length)
                {
                    let last = result.last().expect("non-empty");
                    let chamfer_seg = seg.with_points(last.x2, last.y2, shortened.x1, shortened.y1);
                    if clear(path_is_clear, &chamfer_seg) {
                        result.push(chamfer_seg);
                        modified = shortened;
                    }
                }
            }
            if is_90_degree_corner(seg, next)
                && shorten_segment_start(next, chamfer, config.min_segment_length).is_some()
            {
                if let Some(shortened) =
                    shorten_segment_end(&modified, chamfer, config.min_segment_length)
                {
                    modified = shortened;
                }
            }
            result.push(modified);
        }
    }
    restore_terminal_endpoints(
        result,
        orig_start,
        orig_end,
        pad_positions,
        config.tolerance,
    )
}

fn point_near_any_pad(point: (f64, f64), pads: &[(f64, f64)], tolerance: f64) -> bool {
    let tol_sq = tolerance * tolerance;
    pads.iter().any(|p| {
        let dx = point.0 - p.0;
        let dy = point.1 - p.1;
        dx * dx + dy * dy < tol_sq
    })
}

fn make_legs(template: &Segment, sx: f64, sy: f64, ex: f64, ey: f64) -> Vec<Segment> {
    let points = dogleg(sx, sy, ex, ey, false);
    points
        .windows(2)
        .filter(|w| w[0] != w[1])
        .map(|w| template.with_points(w[0].0, w[0].1, w[1].0, w[1].1))
        .collect()
}

fn restore_terminal_endpoints(
    mut segments: Vec<Segment>,
    orig_start: (f64, f64),
    orig_end: (f64, f64),
    pad_positions: Option<&[(f64, f64)]>,
    tolerance: f64,
) -> Vec<Segment> {
    if segments.is_empty() {
        return segments;
    }
    let pad_match_tolerance = 0.05;
    let first = segments[0].clone();
    if (first.x1 - orig_start.0).abs() > tolerance || (first.y1 - orig_start.1).abs() > tolerance {
        let restore = pad_positions
            .is_none_or(|pads| point_near_any_pad(orig_start, pads, pad_match_tolerance));
        if restore {
            let legs = make_legs(&first, orig_start.0, orig_start.1, first.x2, first.y2);
            segments.splice(0..1, legs);
        }
    }
    // Python's `segments[-1]` after the splice above; an empty splice can
    // only happen for a degenerate single-segment list.
    let Some(last) = segments.last().cloned() else {
        return segments;
    };
    if (last.x2 - orig_end.0).abs() > tolerance || (last.y2 - orig_end.1).abs() > tolerance {
        let restore = pad_positions
            .is_none_or(|pads| point_near_any_pad(orig_end, pads, pad_match_tolerance));
        if restore {
            let legs = make_legs(&last, last.x1, last.y1, orig_end.0, orig_end.1);
            let n = segments.len();
            segments.splice(n - 1..n, legs);
        }
    }
    segments
}

fn segment_parallel_to(seg: &Segment, ux: f64, uy: f64) -> bool {
    let dx = seg.x2 - seg.x1;
    let dy = seg.y2 - seg.y1;
    let norm = dx.hypot(dy);
    if norm < 1e-9 {
        return true;
    }
    (dx * uy - dy * ux).abs() / norm < 1e-4
}

/// PullTight: translate interior segments perpendicular to shorten chains.
pub fn pull_tight_pass(
    segments: &[Segment],
    config: &OptimizationConfig,
    path_is_clear: PathIsClear<'_>,
) -> Vec<Segment> {
    if segments.len() < 3 {
        return segments.to_vec();
    }
    let mut result = segments.to_vec();
    let tol = config.tolerance;
    for _ in 0..config.pull_tight_max_iterations {
        let mut moved = false;
        let mut i = 1;
        while i + 1 < result.len() {
            let prev = result[i - 1].clone();
            let curr = result[i].clone();
            let nxt = result[i + 1].clone();
            let (perp_x, perp_y) = perpendicular_direction(&curr, tol);
            if perp_x == 0.0 && perp_y == 0.0 {
                i += 1;
                continue;
            }
            if !(segment_parallel_to(&prev, perp_x, perp_y)
                && segment_parallel_to(&nxt, perp_x, perp_y))
            {
                i += 1;
                continue;
            }
            let mid_x = (curr.x1 + curr.x2) / 2.0;
            let mid_y = (curr.y1 + curr.y2) / 2.0;
            let (proj_x, proj_y) =
                project_point_onto_line(mid_x, mid_y, prev.x1, prev.y1, nxt.x2, nxt.y2);
            let perp_component = (proj_x - mid_x) * perp_x + (proj_y - mid_y) * perp_y;
            if perp_component.abs() < tol {
                i += 1;
                continue;
            }
            let mut best = 0.0;
            let (mut lo, mut hi) = (0.0, perp_component.abs());
            let sign = if perp_component > 0.0 { 1.0 } else { -1.0 };
            for _ in 0..16 {
                let mid_disp = (lo + hi) / 2.0;
                let dx = sign * perp_x * mid_disp;
                let dy = sign * perp_y * mid_disp;
                let cand_curr = translate_segment(&curr, dx, dy);
                let cand_prev = prev.with_points(prev.x1, prev.y1, cand_curr.x1, cand_curr.y1);
                let cand_nxt = nxt.with_points(cand_curr.x2, cand_curr.y2, nxt.x2, nxt.y2);
                let ok = match path_is_clear {
                    None => true,
                    Some(f) => f(&cand_prev) && f(&cand_curr) && f(&cand_nxt),
                };
                if ok {
                    best = mid_disp;
                    lo = mid_disp;
                } else {
                    hi = mid_disp;
                }
            }
            if best < tol {
                i += 1;
                continue;
            }
            let dx = sign * perp_x * best;
            let dy = sign * perp_y * best;
            let new_curr = translate_segment(&curr, dx, dy);
            let old_len = segment_length(&prev) + segment_length(&curr) + segment_length(&nxt);
            let new_prev = prev.with_points(prev.x1, prev.y1, new_curr.x1, new_curr.y1);
            let new_nxt = nxt.with_points(new_curr.x2, new_curr.y2, nxt.x2, nxt.y2);
            let new_len =
                segment_length(&new_prev) + segment_length(&new_curr) + segment_length(&new_nxt);
            if new_len < old_len - tol {
                result[i - 1] = new_prev;
                result[i] = new_curr;
                result[i + 1] = new_nxt;
                moved = true;
            }
            i += 1;
        }
        result = merge_collinear(&result, config, path_is_clear);
        result.retain(|s| segment_length(s) > tol);
        if !moved {
            break;
        }
    }
    result
}
