//! Port of `kicad_tools.core.geometry`: vector primitives on raw scalars.

/// Minimum distance from point `(px, py)` to segment `(x1, y1)-(x2, y2)`.
///
/// Computed from coordinate differences only, so the result is invariant
/// under rigid translation of all three points.
pub fn point_to_segment_distance(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let seg_len_sq = dx * dx + dy * dy;
    let apx = px - x1;
    let apy = py - y1;
    if seg_len_sq == 0.0 {
        return (apx * apx + apy * apy).sqrt();
    }
    let t = ((apx * dx + apy * dy) / seg_len_sq).clamp(0.0, 1.0);
    let rx = apx - t * dx;
    let ry = apy - t * dy;
    (rx * rx + ry * ry).sqrt()
}

/// Whether two segments properly intersect (shared endpoints and collinear
/// overlap do not count).
#[allow(clippy::too_many_arguments)]
pub fn segments_intersect(
    ax1: f64,
    ay1: f64,
    ax2: f64,
    ay2: f64,
    bx1: f64,
    by1: f64,
    bx2: f64,
    by2: f64,
) -> bool {
    let cross = |ox: f64, oy: f64, px: f64, py: f64, qx: f64, qy: f64| {
        (px - ox) * (qy - oy) - (py - oy) * (qx - ox)
    };
    let d1 = cross(bx1, by1, bx2, by2, ax1, ay1);
    let d2 = cross(bx1, by1, bx2, by2, ax2, ay2);
    let d3 = cross(ax1, ay1, ax2, ay2, bx1, by1);
    let d4 = cross(ax1, ay1, ax2, ay2, bx2, by2);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// Minimum distance between two segments (0 when they properly intersect).
#[allow(clippy::too_many_arguments)]
pub fn segment_to_segment_distance(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    x3: f64,
    y3: f64,
    x4: f64,
    y4: f64,
) -> f64 {
    if segments_intersect(x1, y1, x2, y2, x3, y3, x4, y4) {
        return 0.0;
    }
    let d1 = point_to_segment_distance(x1, y1, x3, y3, x4, y4);
    let d2 = point_to_segment_distance(x2, y2, x3, y3, x4, y4);
    let d3 = point_to_segment_distance(x3, y3, x1, y1, x2, y2);
    let d4 = point_to_segment_distance(x4, y4, x1, y1, x2, y2);
    d1.min(d2).min(d3).min(d4)
}

/// Edge-to-edge clearance between two traces (negative means overlap).
#[allow(clippy::too_many_arguments)]
pub fn segment_clearance(
    ax1: f64,
    ay1: f64,
    ax2: f64,
    ay2: f64,
    width_a: f64,
    bx1: f64,
    by1: f64,
    bx2: f64,
    by2: f64,
    width_b: f64,
) -> f64 {
    segment_to_segment_distance(ax1, ay1, ax2, ay2, bx1, by1, bx2, by2)
        - width_a / 2.0
        - width_b / 2.0
}

/// Rotate a pad's footprint-local offset into the board frame.
///
/// KiCad applies the footprint orientation as a *negated* angle relative to
/// the standard CCW convention: a pad at local `(2, 0)` on a footprint at 90
/// degrees lands at offset `(0, -2)`. Call with `-rotation_deg` for the
/// inverse (board -> local) transform.
pub fn rotate_pad_offset(local_x: f64, local_y: f64, rotation_deg: f64) -> (f64, f64) {
    let angle = (-rotation_deg).to_radians();
    let (sin_a, cos_a) = angle.sin_cos();
    (
        local_x * cos_a - local_y * sin_a,
        local_x * sin_a + local_y * cos_a,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn pad_rotation_matches_pcbnew() {
        assert!(close(rotate_pad_offset(2.0, 0.0, 0.0), (2.0, 0.0)));
        assert!(close(rotate_pad_offset(2.0, 0.0, 90.0), (0.0, -2.0)));
        assert!(close(rotate_pad_offset(2.0, 0.0, 180.0), (-2.0, 0.0)));
        assert!(close(rotate_pad_offset(2.0, 0.0, 270.0), (0.0, 2.0)));
    }

    #[test]
    fn distances() {
        assert_eq!(
            point_to_segment_distance(0.0, 1.0, -1.0, 0.0, 1.0, 0.0),
            1.0
        );
        assert!(segments_intersect(0.0, 0.0, 2.0, 2.0, 0.0, 2.0, 2.0, 0.0));
        assert_eq!(
            segment_to_segment_distance(0.0, 0.0, 2.0, 2.0, 0.0, 2.0, 2.0, 0.0),
            0.0
        );
        let c = segment_clearance(0.0, 0.0, 1.0, 0.0, 0.2, 0.0, 1.0, 1.0, 1.0, 0.2);
        assert!((c - 0.8).abs() < 1e-12);
    }
}
