//! Layer-aware bounds and routing chains of top-level `Edge.Cuts` graphics
//! (port of `kicad_tools.core.board_outline`).
//!
//! Circles are tessellated into inscribed chains whose chord sagitta is
//! certified (`OutlineSegments::max_error_mm`); distance consumers subtract
//! it and keepout consumers add it, which is conservative regardless of
//! whether a circle is an outer boundary or a cutout.

use std::ops::{Deref, DerefMut};

use super::outline_tessellation::{frac, tessellate_arc, tessellate_cubic, DEFAULT_MAX_ERROR_MM};
use crate::exceptions::ValueError;
use crate::sexp::SExp;

pub type Point = (f64, f64);
/// `(min_x, min_y, max_x, max_y)`.
pub type Bounds = (f64, f64, f64, f64);

/// Hausdorff bound (mm) for tessellated `gr_circle` outlines.
pub const CIRCLE_TESSELLATION_MAX_ERROR_MM: f64 = 1e-5;
/// Aesthetic floor on circle chord count.
pub const CIRCLE_MIN_SEGMENTS: usize = 12;
/// Resource ceiling on circle chord count; exceeding it refuses rather than
/// silently weakening the bound.
pub const CIRCLE_MAX_SEGMENTS: usize = 25000;

const TAU: f64 = std::f64::consts::TAU;

fn err(msg: impl Into<String>) -> ValueError {
    ValueError::new(msg)
}

/// Rotate `point` about `center` by `angle_deg` (CCW-positive).
fn rotate_point(point: Point, center: Point, angle_deg: f64) -> Point {
    let theta = angle_deg.to_radians();
    let (dx, dy) = (point.0 - center.0, point.1 - center.1);
    let (sin_t, cos_t) = theta.sin_cos();
    (
        center.0 + dx * cos_t - dy * sin_t,
        center.1 + dx * sin_t + dy * cos_t,
    )
}

/// Normalize a legacy center/endpoint/signed-sweep arc into start/mid/end.
pub fn legacy_arc_points(point: Point, center: Point, angle_deg: f64) -> (Point, Point, Point) {
    (
        point,
        rotate_point(point, center, angle_deg / 2.0),
        rotate_point(point, center, angle_deg),
    )
}

fn coordinate(child: &SExp, context: &str) -> Result<Point, ValueError> {
    if child.children.len() != 2 {
        return Err(err(format!(
            "Malformed Edge.Cuts {context}: expected two coordinates"
        )));
    }
    match (child.float_at(0), child.float_at(1)) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok((x, y)),
        _ => Err(err(format!(
            "Malformed Edge.Cuts {context}: invalid coordinate"
        ))),
    }
}

fn tag(node: &SExp) -> &str {
    node.tag().unwrap_or("None")
}

fn point(node: &SExp, name: &str) -> Result<Point, ValueError> {
    let child = node.find_child(name).ok_or_else(|| {
        err(format!(
            "Malformed Edge.Cuts {}: missing {name} coordinate",
            tag(node)
        ))
    })?;
    coordinate(child, &format!("{} {name}", tag(node)))
}

fn pts_chain(node: &SExp, context: &str) -> Result<Vec<Point>, ValueError> {
    match node.find_child("pts") {
        Some(pts) => pts
            .find_children("xy")
            .into_iter()
            .map(|xy| coordinate(xy, context))
            .collect(),
        None => Ok(Vec::new()),
    }
}

fn arc_points(start: Point, mid: Point, end: Point) -> Result<Vec<Point>, ValueError> {
    let (bx, by) = (mid.0 - start.0, mid.1 - start.1);
    let (cx, cy) = (end.0 - start.0, end.1 - start.1);
    let det = 2.0 * (bx * cy - by * cx);
    if det.abs() < 1e-12 {
        return Err(err("Malformed Edge.Cuts gr_arc: collinear arc points"));
    }
    let (b2, c2) = (bx * bx + by * by, cx * cx + cy * cy);
    let ox = start.0 + (cy * b2 - by * c2) / det;
    let oy = start.1 + (bx * c2 - cx * b2) / det;
    let radius = (start.0 - ox).hypot(start.1 - oy);
    let ang = |p: Point| (p.1 - oy).atan2(p.0 - ox);
    let (a, m, b) = (ang(start), ang(mid), ang(end));
    let span = (b - a).rem_euclid(TAU);
    let ccw = (m - a).rem_euclid(TAU) <= span;
    let mut points = vec![start, mid, end];
    for angle in [
        0.0,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    ] {
        let on_arc = if ccw {
            (angle - a).rem_euclid(TAU) <= span + 1e-12
        } else {
            (a - angle).rem_euclid(TAU) <= (a - b).rem_euclid(TAU) + 1e-12
        };
        if on_arc {
            points.push((ox + radius * angle.cos(), oy + radius * angle.sin()));
        }
    }
    Ok(points)
}

/// Whether a cubic closes along one exact line (encloses no board area).
/// Exact rational arithmetic; no tolerance.
pub fn is_degenerate_closed_curve(points: &[Point]) -> bool {
    if points.len() != 4 || points[0] != points[3] {
        return false;
    }
    let mut distinct: Vec<Point> = Vec::new();
    for p in points {
        if !distinct.contains(p) {
            distinct.push(*p);
        }
    }
    if distinct.len() <= 2 {
        return true;
    }
    let (origin, first, second) = (points[0], points[1], points[2]);
    let ax = frac(first.0) - frac(origin.0);
    let ay = frac(first.1) - frac(origin.1);
    let bx = frac(second.0) - frac(origin.0);
    let by = frac(second.1) - frac(origin.1);
    ax * by == ay * bx
}

fn curve_points(points: &[Point]) -> Result<Vec<Point>, ValueError> {
    if points.len() != 4 {
        return Err(err(
            "Malformed Edge.Cuts gr_curve: expected four cubic control points",
        ));
    }
    let mut extrema: Vec<f64> = vec![0.0, 1.0];
    for axis in 0..2 {
        let c = |i: usize| if axis == 0 { points[i].0 } else { points[i].1 };
        let (p0, p1, p2, p3) = (c(0), c(1), c(2), c(3));
        let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
        let b = 2.0 * (p0 - 2.0 * p1 + p2);
        let cc = p1 - p0;
        let roots: Vec<f64> = if a.abs() < 1e-12 {
            if b.abs() >= 1e-12 {
                vec![-cc / b]
            } else {
                vec![]
            }
        } else {
            let disc = b * b - 4.0 * a * cc;
            if disc >= 0.0 {
                [-1.0, 1.0]
                    .iter()
                    .map(|s| (-b + s * disc.sqrt()) / (2.0 * a))
                    .collect()
            } else {
                vec![]
            }
        };
        for t in roots {
            if 0.0 < t && t < 1.0 && !extrema.contains(&t) {
                extrema.push(t);
            }
        }
    }
    Ok(extrema
        .into_iter()
        .map(|t| {
            let u = 1.0 - t;
            let w = [u.powi(3), 3.0 * u * u * t, 3.0 * u * t * t, t.powi(3)];
            let x = w.iter().zip(points).map(|(w, p)| w * p.0).sum();
            let y = w.iter().zip(points).map(|(w, p)| w * p.1).sum();
            (x, y)
        })
        .collect())
}

/// Exact worst-case chord-to-arc gap for an inscribed `segments`-gon,
/// `r * (1 - cos(pi/n))` via the cancellation-free half-angle identity.
pub fn circle_sagitta(radius: f64, segments: usize) -> Result<f64, ValueError> {
    if segments < 3 {
        return Err(err("circle_sagitta: segments must be at least 3"));
    }
    Ok(2.0 * radius * (std::f64::consts::PI / (2 * segments) as f64).sin().powi(2))
}

/// Equal chords needed to keep the sagitta within `max_error`; refuses
/// (never clamps) beyond [`CIRCLE_MAX_SEGMENTS`].
pub fn circle_segment_count(radius: f64, max_error: f64) -> Result<usize, ValueError> {
    if radius <= 0.0 || !radius.is_finite() {
        return Err(err(
            "Malformed Edge.Cuts gr_circle: radius must be finite and positive",
        ));
    }
    if max_error <= 0.0 || !max_error.is_finite() {
        return Err(err(
            "circle_segment_count: max_error must be finite and positive",
        ));
    }
    let sin_half = (max_error / (2.0 * radius)).sqrt();
    if sin_half >= 1.0 {
        return Ok(CIRCLE_MIN_SEGMENTS);
    }
    let estimate = (std::f64::consts::PI / (2.0 * sin_half.asin())).ceil();
    let mut segments = if estimate.is_finite() && estimate < (CIRCLE_MAX_SEGMENTS + 1) as f64 {
        (estimate as usize).max(CIRCLE_MIN_SEGMENTS)
    } else {
        CIRCLE_MAX_SEGMENTS + 1
    };
    while segments <= CIRCLE_MAX_SEGMENTS && circle_sagitta(radius, segments)? > max_error {
        segments += 1;
    }
    if segments > CIRCLE_MAX_SEGMENTS {
        return Err(err(format!(
            "Edge.Cuts gr_circle radius {}mm needs more than {CIRCLE_MAX_SEGMENTS} chords to \
             stay within {}mm of the true circle; refusing to tessellate rather than silently \
             exceed the documented error bound",
            crate::utils::pyrepr::py_float_repr(radius),
            crate::utils::pyrepr::py_float_repr(max_error),
        )));
    }
    Ok(segments)
}

/// Ordered on-circle vertices of a closed chord chain.
pub fn circle_tessellation_points(
    center: Point,
    radius: f64,
    max_error: f64,
) -> Result<Vec<Point>, ValueError> {
    let n = circle_segment_count(radius, max_error)?;
    let (cx, cy) = center;
    Ok((0..n)
        .map(|i| {
            let a = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            (cx + radius * a.cos(), cy + radius * a.sin())
        })
        .collect())
}

/// Direct children that are explicitly `Edge.Cuts` `gr_*` graphics.
pub fn outline_graphics(root: &SExp) -> impl Iterator<Item = &SExp> {
    root.children.iter().filter(|node| {
        node.is_list()
            && node.tag().is_some_and(|t| t.starts_with("gr_"))
            && node
                .find_child("layer")
                .is_some_and(|l| l.string_at(0) == Some("Edge.Cuts"))
    })
}

/// Straight-edge outline chain plus its certified approximation error
/// (`0.0` when every element was already straight). Derefs to the segment
/// list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutlineSegments {
    pub segments: Vec<(Point, Point)>,
    pub max_error_mm: f64,
}

impl OutlineSegments {
    pub fn new(segments: Vec<(Point, Point)>, max_error_mm: f64) -> Self {
        Self {
            segments,
            max_error_mm,
        }
    }
}

impl Deref for OutlineSegments {
    type Target = Vec<(Point, Point)>;
    fn deref(&self) -> &Self::Target {
        &self.segments
    }
}

impl DerefMut for OutlineSegments {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.segments
    }
}

fn legacy_sweep(node: &SExp) -> Result<f64, ValueError> {
    let angle = node.find_child("angle");
    match angle {
        Some(a) if a.children.len() == 1 && a.float_at(0).is_some_and(f64::is_finite) => {
            Ok(a.float_at(0).unwrap())
        }
        _ => Err(err(
            "Malformed Edge.Cuts gr_arc: missing mid or invalid legacy angle",
        )),
    }
}

/// Authored cubic/arc path for routing (`_routing_curved_chain`). Legacy
/// arcs require `0 < |sweep| < 360`.
pub fn routing_curved_chain(node: &SExp) -> Result<Vec<Point>, ValueError> {
    if node.has_tag("gr_curve") {
        let chain = pts_chain(node, "gr_curve xy")?;
        return tessellate_cubic(&chain);
    }
    if !node.has_tag("gr_arc") {
        return Err(err(format!(
            "Unsupported curved routing Edge.Cuts geometry: {}",
            tag(node)
        )));
    }
    let (mut start, mut end) = (point(node, "start")?, point(node, "end")?);
    let mid = if node.find_child("mid").is_some() {
        point(node, "mid")?
    } else {
        let sweep = legacy_sweep(node)?;
        if !(0.0 < sweep.abs() && sweep.abs() < 360.0) {
            return Err(err(
                "Unsupported routing Edge.Cuts legacy arc: require 0 < abs(sweep) < 360",
            ));
        }
        let (s, m, e) = legacy_arc_points(end, start, sweep);
        start = s;
        end = e;
        m
    };
    tessellate_arc(start, mid, end)
}

fn ring_segments(ring: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    ring.iter()
        .copied()
        .zip(ring.iter().copied().cycle().skip(1))
}

/// Endpoint-preserving outline chains with a certified Hausdorff allowance.
/// Every authored graphic stays a separate component; no closure is guessed.
pub fn board_outline_segments(root: &SExp) -> Result<OutlineSegments, ValueError> {
    let mut segments = OutlineSegments::default();
    for node in outline_graphics(root) {
        match node.tag().unwrap_or("") {
            "gr_line" => segments.push((point(node, "start")?, point(node, "end")?)),
            "gr_rect" => {
                let (a, b) = (point(node, "start")?, point(node, "end")?);
                let corners = [a, (b.0, a.1), b, (a.0, b.1)];
                segments.extend(ring_segments(&corners));
            }
            "gr_poly" => {
                let chain = pts_chain(node, "gr_poly xy")?;
                if chain.len() < 3 {
                    return Err(err("Malformed Edge.Cuts gr_poly: missing points"));
                }
                segments.extend(ring_segments(&chain));
            }
            "gr_curve" | "gr_arc" => {
                let chain = routing_curved_chain(node)?;
                segments.extend(chain.windows(2).map(|w| (w[0], w[1])));
                segments.max_error_mm = segments.max_error_mm.max(DEFAULT_MAX_ERROR_MM);
            }
            "gr_circle" => {
                let (center, end) = (point(node, "center")?, point(node, "end")?);
                let radius = (center.0 - end.0).hypot(center.1 - end.1);
                if radius <= 0.0 {
                    return Err(err(
                        "Malformed Edge.Cuts gr_circle: zero-radius circle has no boundary",
                    ));
                }
                let ring =
                    circle_tessellation_points(center, radius, CIRCLE_TESSELLATION_MAX_ERROR_MM)?;
                segments.extend(ring_segments(&ring));
                segments.max_error_mm = segments
                    .max_error_mm
                    .max(circle_sagitta(radius, ring.len())?);
            }
            other => {
                return Err(err(format!(
                    "Unsupported routing Edge.Cuts geometry: {other}; straight edges are required"
                )))
            }
        }
    }
    Ok(segments)
}

/// Sheet-absolute bounds of the outline (stroke width excluded; closed
/// collinear cubics ignored). `None` when there is no outline geometry;
/// malformed or unsupported graphics are errors. Does not validate closure.
pub fn board_outline_bounds(root: &SExp) -> Result<Option<Bounds>, ValueError> {
    let mut points: Vec<Point> = Vec::new();
    for node in outline_graphics(root) {
        let t = node.tag().unwrap_or("");
        match t {
            "gr_rect" | "gr_line" => {
                points.push(point(node, "start")?);
                points.push(point(node, "end")?);
            }
            "gr_arc" => {
                let (mut start, mut end) = (point(node, "start")?, point(node, "end")?);
                let mid = if node.find_child("mid").is_some() {
                    point(node, "mid")?
                } else {
                    let sweep = legacy_sweep(node)?;
                    let (s, m, e) = legacy_arc_points(end, start, sweep);
                    start = s;
                    end = e;
                    m
                };
                points.extend(arc_points(start, mid, end)?);
            }
            "gr_circle" => {
                let (center, end) = (point(node, "center")?, point(node, "end")?);
                let r = (center.0 - end.0).hypot(center.1 - end.1);
                points.push((center.0 - r, center.1 - r));
                points.push((center.0 + r, center.1 + r));
            }
            "gr_poly" | "gr_curve" => {
                let chain = pts_chain(node, &format!("{t} xy"))?;
                if chain.len() < 3 {
                    return Err(err(format!("Malformed Edge.Cuts {t}: missing points")));
                }
                if t == "gr_curve" {
                    if is_degenerate_closed_curve(&chain) {
                        continue;
                    }
                    points.extend(curve_points(&chain)?);
                } else {
                    points.extend(chain);
                }
            }
            other => {
                return Err(err(format!("Unsupported Edge.Cuts geometry: {other}")));
            }
        }
    }
    if points.is_empty() {
        return Ok(None);
    }
    let fold = |f: fn(f64, f64) -> f64, init: f64, sel: fn(&Point) -> f64| {
        points.iter().map(sel).fold(init, f)
    };
    Ok(Some((
        fold(f64::min, f64::INFINITY, |p| p.0),
        fold(f64::min, f64::INFINITY, |p| p.1),
        fold(f64::max, f64::NEG_INFINITY, |p| p.0),
        fold(f64::max, f64::NEG_INFINITY, |p| p.1),
    )))
}
