//! Checker-agnostic trace-copper geometry (port of
//! `kicad_tools.geometry.copper`, Issue #4176/#5060).
//!
//! A trace segment's copper is its centerline buffered by `width / 2`
//! (round caps): a capsule. [`segment_copper_polygon`] builds that shape as
//! a `geo` polygon (faceted like shapely's `buffer()`, 16 segments per
//! quarter circle); [`segments_copper_touch`] decides capsule contact
//! exactly and far more cheaply.

use geo::{Coord, Geometry, LineString, Point, Polygon};

/// Segments per quarter circle (shapely's default `quad_segs`).
pub const QUAD_SEGS: usize = 16;

fn arc(center: (f64, f64), r: f64, from: f64, sweep: f64, n: usize) -> impl Iterator<Item = Coord> {
    (0..=n).map(move |i| {
        let a = from + sweep * i as f64 / n as f64;
        Coord {
            x: center.0 + r * a.cos(),
            y: center.1 + r * a.sin(),
        }
    })
}

/// Round disk of radius `r` (`Point.buffer(r)`).
pub fn disk_polygon(center: (f64, f64), r: f64) -> Polygon<f64> {
    let n = 4 * QUAD_SEGS;
    let mut coords: Vec<Coord> = arc(center, r, 0.0, std::f64::consts::TAU, n).collect();
    coords.pop();
    Polygon::new(LineString::from(coords), vec![])
}

/// Capsule around `start-end` of radius `r` (`LineString.buffer(r)`).
pub fn capsule_polygon(start: (f64, f64), end: (f64, f64), r: f64) -> Polygon<f64> {
    let theta = (end.1 - start.1).atan2(end.0 - start.0);
    let half = std::f64::consts::FRAC_PI_2;
    let n = 2 * QUAD_SEGS;
    let coords: Vec<Coord> = arc(end, r, theta - half, std::f64::consts::PI, n)
        .chain(arc(start, r, theta + half, std::f64::consts::PI, n))
        .collect();
    Polygon::new(LineString::from(coords), vec![])
}

/// Board-frame geometry of a trace's copper.
///
/// * `width <= 0`: the bare centerline (a `LineString`, or a `Point` when
///   the segment is also zero-length) -- contact without spurious area.
/// * zero length with positive width: the round end-cap disk.
pub fn segment_copper_polygon(start: (f64, f64), end: (f64, f64), width: f64) -> Geometry<f64> {
    if start == end {
        if width <= 0.0 {
            return Geometry::Point(Point::new(start.0, start.1));
        }
        return Geometry::Polygon(disk_polygon(start, width / 2.0));
    }
    if width <= 0.0 {
        return Geometry::LineString(LineString::from(vec![start, end]));
    }
    Geometry::Polygon(capsule_polygon(start, end, width / 2.0))
}

/// Shortest distance from `point` to the closed segment `seg_start-seg_end`.
pub fn point_segment_distance(
    point: (f64, f64),
    seg_start: (f64, f64),
    seg_end: (f64, f64),
) -> f64 {
    let (px, py) = point;
    let (ax, ay) = seg_start;
    let (bx, by) = seg_end;
    let (dx, dy) = (bx - ax, by - ay);
    let length_sq = dx * dx + dy * dy;
    if length_sq <= 0.0 {
        return (px - ax).hypot(py - ay);
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / length_sq).clamp(0.0, 1.0);
    (px - (ax + t * dx)).hypot(py - (ay + t * dy))
}

fn orientation(p: (f64, f64), q: (f64, f64), r: (f64, f64)) -> f64 {
    (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)
}

/// Exact minimum distance between two closed segments (0 when they cross
/// or touch); the analytic `LineString(a).distance(LineString(b))`.
pub fn segment_centerline_distance(
    a_start: (f64, f64),
    a_end: (f64, f64),
    b_start: (f64, f64),
    b_end: (f64, f64),
) -> f64 {
    let o1 = orientation(a_start, a_end, b_start);
    let o2 = orientation(a_start, a_end, b_end);
    let o3 = orientation(b_start, b_end, a_start);
    let o4 = orientation(b_start, b_end, a_end);
    if o1 * o2 < 0.0 && o3 * o4 < 0.0 {
        return 0.0;
    }
    point_segment_distance(b_start, a_start, a_end)
        .min(point_segment_distance(b_end, a_start, a_end))
        .min(point_segment_distance(a_start, b_start, b_end))
        .min(point_segment_distance(a_end, b_start, b_end))
}

/// True iff two same-layer traces' swept copper (capsules) touch: centerline
/// distance <= sum of half-widths (negative widths clamp to zero). Invariant
/// under collinear splitting of either segment.
pub fn segments_copper_touch(
    a_start: (f64, f64),
    a_end: (f64, f64),
    a_width: f64,
    b_start: (f64, f64),
    b_end: (f64, f64),
    b_width: f64,
) -> bool {
    let reach = a_width.max(0.0) / 2.0 + b_width.max(0.0) / 2.0;
    let axis = |p: (f64, f64), i: usize| if i == 0 { p.0 } else { p.1 };
    for i in 0..2 {
        let (a0, a1) = (axis(a_start, i), axis(a_end, i));
        let (b0, b1) = (axis(b_start, i), axis(b_end, i));
        if a0.min(a1) > b0.max(b1) + reach || b0.min(b1) > a0.max(a1) + reach {
            return false;
        }
    }
    segment_centerline_distance(a_start, a_end, b_start, b_end) <= reach
}
