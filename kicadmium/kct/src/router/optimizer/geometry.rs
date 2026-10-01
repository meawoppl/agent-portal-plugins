//! Port of `kicad_tools.router.optimizer.geometry`: segment helpers.

use crate::router::primitives::Segment;

/// Direction of a segment in degrees (0-360); 0 for zero-length.
pub fn segment_direction(seg: &Segment, tolerance: f64) -> f64 {
    let dx = seg.x2 - seg.x1;
    let dy = seg.y2 - seg.y1;
    if dx.abs() < tolerance && dy.abs() < tolerance {
        return 0.0;
    }
    let mut angle = dy.atan2(dx).to_degrees();
    if angle < 0.0 {
        angle += 360.0;
    }
    angle
}

/// End of `s1` meets start of `s2`.
pub fn is_connected(s1: &Segment, s2: &Segment, tolerance: f64) -> bool {
    (s1.x2 - s2.x1).abs() < tolerance && (s1.y2 - s2.y1).abs() < tolerance
}

/// The segments share any endpoint.
pub fn segments_touch(s1: &Segment, s2: &Segment, tolerance: f64) -> bool {
    let near = |ax: f64, ay: f64, bx: f64, by: f64| {
        (ax - bx).abs() < tolerance && (ay - by).abs() < tolerance
    };
    near(s1.x2, s1.y2, s2.x1, s2.y1)
        || near(s1.x2, s1.y2, s2.x2, s2.y2)
        || near(s1.x1, s1.y1, s2.x1, s2.y1)
        || near(s1.x1, s1.y1, s2.x2, s2.y2)
}

/// Parallel and pointing the same way (zero-length counts as same).
pub fn same_direction(s1: &Segment, s2: &Segment, tolerance: f64) -> bool {
    let (mut dx1, mut dy1) = (s1.x2 - s1.x1, s1.y2 - s1.y1);
    let (mut dx2, mut dy2) = (s2.x2 - s2.x1, s2.y2 - s2.y1);
    let len1 = (dx1 * dx1 + dy1 * dy1).sqrt();
    let len2 = (dx2 * dx2 + dy2 * dy2).sqrt();
    if len1 < tolerance || len2 < tolerance {
        return true;
    }
    dx1 /= len1;
    dy1 /= len1;
    dx2 /= len2;
    dy2 /= len2;
    let cross = (dx1 * dy2 - dy1 * dx2).abs();
    let dot = dx1 * dx2 + dy1 * dy2;
    cross < 0.01 && dot > 0.0
}

/// Angle between two segments in degrees (0-180).
pub fn angle_between(s1: &Segment, s2: &Segment, tolerance: f64) -> f64 {
    let (dx1, dy1) = (s1.x2 - s1.x1, s1.y2 - s1.y1);
    let (dx2, dy2) = (s2.x2 - s2.x1, s2.y2 - s2.y1);
    let len1 = (dx1 * dx1 + dy1 * dy1).sqrt();
    let len2 = (dx2 * dx2 + dy2 * dy2).sqrt();
    if len1 < tolerance || len2 < tolerance {
        return 0.0;
    }
    let dot = dx1 * dx2 + dy1 * dy2;
    let cos_angle = (dot / (len1 * len2)).clamp(-1.0, 1.0);
    cos_angle.acos().to_degrees()
}

/// Two segments form a (roughly) 90-degree corner.
pub fn is_90_degree_corner(s1: &Segment, s2: &Segment) -> bool {
    let angle = angle_between(s1, s2, 1e-4);
    80.0 < angle && angle < 100.0
}

/// `s2` backtracks relative to `s1`.
pub fn is_zigzag(s1: &Segment, s2: &Segment, _s3: &Segment, tolerance: f64) -> bool {
    (angle_between(s1, s2, tolerance) - 180.0).abs() < 30.0
}

pub fn segment_length(seg: &Segment) -> f64 {
    let dx = seg.x2 - seg.x1;
    let dy = seg.y2 - seg.y1;
    (dx * dx + dy * dy).sqrt()
}

/// Shorten from the end by `amount`; `None` if it would get too short.
pub fn shorten_segment_end(seg: &Segment, amount: f64, min_length: f64) -> Option<Segment> {
    let dx = seg.x2 - seg.x1;
    let dy = seg.y2 - seg.y1;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= amount + min_length {
        return None;
    }
    let ratio = (length - amount) / length;
    Some(seg.with_points(seg.x1, seg.y1, seg.x1 + dx * ratio, seg.y1 + dy * ratio))
}

/// Shorten from the start by `amount`; `None` if it would get too short.
pub fn shorten_segment_start(seg: &Segment, amount: f64, min_length: f64) -> Option<Segment> {
    let dx = seg.x2 - seg.x1;
    let dy = seg.y2 - seg.y1;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= amount + min_length {
        return None;
    }
    let ratio = amount / length;
    Some(seg.with_points(seg.x1 + dx * ratio, seg.y1 + dy * ratio, seg.x2, seg.y2))
}

/// Unit vector rotated 90 degrees CCW from the segment; `(0, 0)` if degenerate.
pub fn perpendicular_direction(seg: &Segment, tolerance: f64) -> (f64, f64) {
    let dx = seg.x2 - seg.x1;
    let dy = seg.y2 - seg.y1;
    let length = (dx * dx + dy * dy).sqrt();
    if length < tolerance {
        return (0.0, 0.0);
    }
    (-dy / length, dx / length)
}

/// Project a point onto the infinite line through two points.
pub fn project_point_onto_line(
    px: f64,
    py: f64,
    lx1: f64,
    ly1: f64,
    lx2: f64,
    ly2: f64,
) -> (f64, f64) {
    let dx = lx2 - lx1;
    let dy = ly2 - ly1;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        return (lx1, ly1);
    }
    let t = ((px - lx1) * dx + (py - ly1) * dy) / len_sq;
    (lx1 + t * dx, ly1 + t * dy)
}

/// Copy of `seg` translated by `(dx, dy)`.
pub fn translate_segment(seg: &Segment, dx: f64, dy: f64) -> Segment {
    seg.with_points(seg.x1 + dx, seg.y1 + dy, seg.x2 + dx, seg.y2 + dy)
}

/// Number of direction changes along a segment list.
pub fn count_corners(segments: &[Segment], tolerance: f64) -> usize {
    if segments.len() < 2 {
        return 0;
    }
    segments
        .windows(2)
        .filter(|w| !same_direction(&w[0], &w[1], tolerance))
        .count()
}

/// Total length (summed in order, like upstream).
pub fn total_length(segments: &[Segment]) -> f64 {
    let mut total = 0.0;
    for seg in segments {
        total += segment_length(seg);
    }
    total
}
