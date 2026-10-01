//! Package-body outline geometry for footprints (port of
//! `kicad_tools.geometry.package_body`).
//!
//! The physical body of a placed footprint in board coordinates: the largest
//! closed shape on the footprint's own-side Fab layer (`fp_rect`, `fp_poly`,
//! `fp_circle`, or a loop of `fp_line` / `fp_arc` polygonized like shapely's
//! `polygonize`), falling back to the courtyard polygon.

use std::collections::HashMap;

use geo::{Area, Contains, Coord, LineString, MultiPolygon, Polygon};

use super::courtyard::{
    courtyard_polygon, fp_transform, valid_polygon, FootprintGeometry, GraphicGeometry, Point,
};
use crate::core::types::Layer;

pub const BODY_SOURCE_FAB: &str = "fab";
pub const BODY_SOURCE_COURTYARD: &str = "courtyard";

/// Segments used to approximate an `fp_circle` body outline (shapely
/// `buffer(radius, 16)`: 16 per quarter).
pub const CIRCLE_RESOLUTION: usize = 16;

/// `"B"` for a back-side footprint, else `"F"`.
pub fn footprint_side<F: FootprintGeometry + ?Sized>(footprint: &F) -> &'static str {
    if footprint.layer().starts_with("B.") {
        "B"
    } else {
        "F"
    }
}

fn circle_ring(center: Point, radius: f64) -> Vec<Point> {
    let n = 4 * CIRCLE_RESOLUTION;
    (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            (center.0 + radius * a.cos(), center.1 + radius * a.sin())
        })
        .collect()
}

/// Largest closed body shape on the footprint's Fab layer.
pub fn fab_body_polygon<F: FootprintGeometry + ?Sized>(
    footprint: &F,
    side: &str,
) -> Option<MultiPolygon<f64>> {
    let target = if side == "F" {
        Layer::FFab.as_str()
    } else {
        Layer::BFab.as_str()
    };
    let transform = fp_transform(footprint);
    let mut candidates: Vec<MultiPolygon<f64>> = Vec::new();
    let mut lines: Vec<Vec<Point>> = Vec::new();
    for g in footprint.graphics() {
        if g.layer() != target {
            continue;
        }
        match g.graphic_type() {
            "rect" => {
                let ((sx, sy), (ex, ey)) = (g.start(), g.end());
                let ring = [(sx, sy), (ex, sy), (ex, ey), (sx, ey)];
                candidates.extend(valid_polygon(ring.iter().map(|p| transform(*p)).collect()));
            }
            "poly" if g.points().len() >= 3 => {
                candidates.extend(valid_polygon(
                    g.points().iter().map(|p| transform(*p)).collect(),
                ));
            }
            "circle" => {
                if let Some(center) = g.center() {
                    let radius = g.radius().unwrap_or_else(|| {
                        let e = g.end();
                        (e.0 - center.0).hypot(e.1 - center.1)
                    });
                    if radius > 0.0 {
                        candidates.extend(valid_polygon(circle_ring(transform(center), radius)));
                    }
                }
            }
            "line" => {
                if g.start() != g.end() {
                    lines.push(vec![transform(g.start()), transform(g.end())]);
                }
            }
            "arc" => {
                let mut pts = vec![g.start()];
                pts.extend(g.mid());
                pts.push(g.end());
                lines.push(pts.into_iter().map(&transform).collect());
            }
            _ => {}
        }
    }
    if !lines.is_empty() {
        candidates.extend(
            polygonize(&lines)
                .into_iter()
                .map(|p| MultiPolygon::new(vec![p])),
        );
    }
    let mut best: Option<MultiPolygon<f64>> = None;
    for candidate in candidates {
        let area = candidate.unsigned_area();
        if area <= 0.0 {
            continue;
        }
        if best.as_ref().is_none_or(|b| area > b.unsigned_area()) {
            best = Some(candidate);
        }
    }
    best
}

/// `(polygon, source)` for a footprint's package body; `None` when neither
/// a Fab outline nor (with `fallback_to_courtyard`) a courtyard resolves.
pub fn package_body_polygon<F: FootprintGeometry + ?Sized>(
    footprint: &F,
    fallback_to_courtyard: bool,
) -> Option<(MultiPolygon<f64>, &'static str)> {
    let side = footprint_side(footprint);
    if let Some(p) = fab_body_polygon(footprint, side) {
        return Some((p, BODY_SOURCE_FAB));
    }
    if fallback_to_courtyard {
        if let Some(p) = courtyard_polygon(footprint, side) {
            return Some((p, BODY_SOURCE_COURTYARD));
        }
    }
    None
}

// ---------------------------------------------------------------- polygonize

fn key(p: Point) -> (u64, u64) {
    // Normalize -0.0 so exactly-equal coordinates share a node.
    let n = |v: f64| if v == 0.0 { 0.0f64 } else { v };
    (n(p.0).to_bits(), n(p.1).to_bits())
}

fn signed_area(ring: &[Point]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f64>()
        / 2.0
}

/// Shapely-style `polygonize` of noded linework: each polyline is an edge
/// between its (exactly matching) endpoints; dangles are removed and the
/// bounded faces of the planar graph become polygons, with rings of nested
/// disconnected components as holes.
pub fn polygonize(lines: &[Vec<Point>]) -> Vec<Polygon<f64>> {
    // Nodes and edges.
    let mut node_ids: HashMap<(u64, u64), usize> = HashMap::new();
    let mut node_pts: Vec<Point> = Vec::new();
    let mut node = |p: Point| -> usize {
        *node_ids.entry(key(p)).or_insert_with(|| {
            node_pts.push(p);
            node_pts.len() - 1
        })
    };
    struct Edge {
        a: usize,
        b: usize,
        pts: Vec<Point>,
    }
    let mut edges: Vec<Edge> = Vec::new();
    for line in lines {
        if line.len() < 2 {
            continue;
        }
        let a = node(line[0]);
        let b = node(*line.last().unwrap());
        if a == b && line.len() < 4 {
            continue;
        }
        edges.push(Edge {
            a,
            b,
            pts: line.clone(),
        });
    }
    // Iteratively drop dangling edges.
    let mut alive = vec![true; edges.len()];
    loop {
        let mut degree = vec![0usize; node_pts.len()];
        for (i, e) in edges.iter().enumerate() {
            if alive[i] {
                degree[e.a] += 1;
                degree[e.b] += 1;
            }
        }
        let mut changed = false;
        for (i, e) in edges.iter().enumerate() {
            if alive[i] && e.a != e.b && (degree[e.a] < 2 || degree[e.b] < 2) {
                alive[i] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Half-edges: (edge, forward). Outgoing lists sorted by angle.
    let half_pts = |e: &Edge, fwd: bool| -> Vec<Point> {
        if fwd {
            e.pts.clone()
        } else {
            e.pts.iter().rev().copied().collect()
        }
    };
    let mut outgoing: Vec<Vec<(f64, usize, bool)>> = vec![Vec::new(); node_pts.len()];
    for (i, e) in edges.iter().enumerate() {
        if !alive[i] {
            continue;
        }
        for fwd in [true, false] {
            let pts = half_pts(e, fwd);
            let from = if fwd { e.a } else { e.b };
            let (p, q) = (pts[0], pts[1]);
            outgoing[from].push(((q.1 - p.1).atan2(q.0 - p.0), i, fwd));
        }
    }
    for list in &mut outgoing {
        list.sort_by(|x, y| x.0.total_cmp(&y.0));
    }
    let mut used: HashMap<(usize, bool), bool> = HashMap::new();
    let mut faces: Vec<(Vec<Point>, usize)> = Vec::new(); // ring, component seed edge
    let mut component = vec![usize::MAX; edges.len()];
    // Connected components (by edges) for hole assignment.
    {
        let mut comp_id = 0;
        for start in 0..edges.len() {
            if !alive[start] || component[start] != usize::MAX {
                continue;
            }
            let mut stack = vec![start];
            component[start] = comp_id;
            while let Some(e) = stack.pop() {
                for (j, other) in edges.iter().enumerate() {
                    if alive[j]
                        && component[j] == usize::MAX
                        && [other.a, other.b]
                            .iter()
                            .any(|n| *n == edges[e].a || *n == edges[e].b)
                    {
                        component[j] = comp_id;
                        stack.push(j);
                    }
                }
            }
            comp_id += 1;
        }
    }
    for (i, _) in edges.iter().enumerate() {
        if !alive[i] {
            continue;
        }
        for fwd in [true, false] {
            if used.contains_key(&(i, fwd)) {
                continue;
            }
            let mut ring: Vec<Point> = Vec::new();
            let (mut cur, mut dir) = (i, fwd);
            let mut guard = 0;
            loop {
                used.insert((cur, dir), true);
                let pts = half_pts(&edges[cur], dir);
                ring.extend(&pts[..pts.len() - 1]);
                let to = if dir { edges[cur].b } else { edges[cur].a };
                // Arrival direction reversed = angle of the twin half-edge.
                let back = {
                    let n = pts.len();
                    let (p, q) = (pts[n - 1], pts[n - 2]);
                    (q.1 - p.1).atan2(q.0 - p.0)
                };
                let list = &outgoing[to];
                // Next half-edge clockwise from the twin: the largest angle
                // strictly below `back` (wrapping), which traces faces
                // counter-clockwise.
                let pos = list
                    .iter()
                    .position(|(a, e, d)| *a == back && *e == cur && *d != dir);
                let idx = match pos {
                    Some(p) => (p + list.len() - 1) % list.len(),
                    None => break,
                };
                let (_, ne, nd) = list[idx];
                if (ne, nd) == (i, fwd) {
                    break;
                }
                cur = ne;
                dir = nd;
                guard += 1;
                if guard > 4 * edges.len() + 4 {
                    ring.clear();
                    break;
                }
            }
            if ring.len() >= 3 {
                faces.push((ring, component[i]));
            }
        }
    }
    // Bounded faces are CCW (positive area); outer faces are CW.
    let mut shells: Vec<(Polygon<f64>, usize, f64)> = Vec::new();
    let mut outers: Vec<(Vec<Point>, usize)> = Vec::new();
    for (ring, comp) in faces {
        let area = signed_area(&ring);
        if area > 0.0 {
            let poly = Polygon::new(to_ls(&ring), vec![]);
            shells.push((poly, comp, area));
        } else if area < 0.0 {
            outers.push((ring, comp));
        }
    }
    // Holes: another component's outer boundary inside a shell (smallest
    // containing shell wins).
    let mut holes: Vec<Vec<LineString<f64>>> = vec![Vec::new(); shells.len()];
    for (ring, comp) in &outers {
        let probe = geo::Point::new(ring[0].0, ring[0].1);
        let host = shells
            .iter()
            .enumerate()
            .filter(|(_, (poly, c, _))| c != comp && poly.contains(&probe))
            .min_by(|a, b| a.1 .2.total_cmp(&b.1 .2))
            .map(|(i, _)| i);
        if let Some(h) = host {
            holes[h].push(to_ls(ring));
        }
    }
    shells
        .into_iter()
        .zip(holes)
        .map(|((poly, _, _), holes)| Polygon::new(poly.exterior().clone(), holes))
        .collect()
}

fn to_ls(ring: &[Point]) -> LineString<f64> {
    LineString::from(
        ring.iter()
            .map(|(x, y)| Coord { x: *x, y: *y })
            .collect::<Vec<_>>(),
    )
}
