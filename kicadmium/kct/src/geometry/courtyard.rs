//! Checker-agnostic courtyard-polygon geometry (port of
//! `kicad_tools.geometry.courtyard`, Issue #4182).
//!
//! The real `F.CrtYd` / `B.CrtYd` polygon of a placed footprint in board
//! coordinates, from (in priority order) an `fp_rect`, an `fp_poly`, or a
//! closed chain of `fp_line` segments. Unresolvable courtyards are `None`.
//!
//! Footprints are accessed through the [`FootprintGeometry`] /
//! [`GraphicGeometry`] traits so any footprint model (e.g. a
//! `schema::pcb::Footprint`) can be used; [`SimpleFootprint`] /
//! [`SimpleGraphic`] are plain implementations, with `from_sexp` parsers
//! for raw `(footprint ...)` nodes.

use geo::{Area, BooleanOps, Coord, LineString, MultiPolygon, Polygon, Validation};

use crate::core::types::Layer;
use crate::sexp::SExp;

pub type Point = (f64, f64);

/// Endpoint-matching tolerance (mm) when chaining `fp_line` segments.
pub const CHAIN_EPSILON_MM: f64 = 1e-3;

/// Footprint graphic accessors (mirrors `schema.pcb.FootprintGraphic`).
pub trait GraphicGeometry {
    /// `line`, `rect`, `circle`, `arc`, `poly`.
    fn graphic_type(&self) -> &str;
    fn layer(&self) -> &str;
    fn start(&self) -> Point;
    fn end(&self) -> Point;
    fn mid(&self) -> Option<Point>;
    fn center(&self) -> Option<Point>;
    fn radius(&self) -> Option<f64>;
    fn points(&self) -> &[Point];
}

/// Footprint accessors (mirrors `schema.pcb.Footprint`).
pub trait FootprintGeometry {
    type Graphic: GraphicGeometry;
    fn position(&self) -> Point;
    /// Orientation in degrees.
    fn rotation(&self) -> f64;
    /// Placement layer (`F.Cu` / `B.Cu`).
    fn layer(&self) -> &str;
    fn graphics(&self) -> &[Self::Graphic];
}

/// Plain footprint graphic.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SimpleGraphic {
    pub graphic_type: String,
    pub layer: String,
    pub start: Point,
    pub end: Point,
    pub mid: Option<Point>,
    pub center: Option<Point>,
    pub radius: Option<f64>,
    pub points: Vec<Point>,
}

impl SimpleGraphic {
    /// Parse an `fp_line` / `fp_rect` / `fp_circle` / `fp_arc` / `fp_poly`.
    pub fn from_sexp(node: &SExp) -> Option<Self> {
        let kind = node.tag()?.strip_prefix("fp_")?;
        if !matches!(kind, "line" | "rect" | "circle" | "arc" | "poly") {
            return None;
        }
        let xy = |name: &str| {
            node.get(name)
                .and_then(|c| Some((c.float_at(0)?, c.float_at(1)?)))
        };
        let center = xy("center");
        let mut g = SimpleGraphic {
            graphic_type: kind.to_string(),
            layer: node.child_str("layer").unwrap_or("").to_string(),
            start: xy("start").or(center).unwrap_or((0.0, 0.0)),
            end: xy("end").unwrap_or((0.0, 0.0)),
            mid: xy("mid"),
            center,
            radius: None,
            points: node.points(),
        };
        if kind == "circle" {
            if let Some(c) = center {
                g.radius = Some((g.end.0 - c.0).hypot(g.end.1 - c.1));
            }
        }
        Some(g)
    }
}

impl GraphicGeometry for SimpleGraphic {
    fn graphic_type(&self) -> &str {
        &self.graphic_type
    }
    fn layer(&self) -> &str {
        &self.layer
    }
    fn start(&self) -> Point {
        self.start
    }
    fn end(&self) -> Point {
        self.end
    }
    fn mid(&self) -> Option<Point> {
        self.mid
    }
    fn center(&self) -> Option<Point> {
        self.center
    }
    fn radius(&self) -> Option<f64> {
        self.radius
    }
    fn points(&self) -> &[Point] {
        &self.points
    }
}

/// Plain footprint.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SimpleFootprint {
    pub position: Point,
    pub rotation: f64,
    pub layer: String,
    pub graphics: Vec<SimpleGraphic>,
}

impl SimpleFootprint {
    /// Parse a `(footprint ...)` / `(module ...)` node's placement and graphics.
    pub fn from_sexp(node: &SExp) -> Self {
        let (x, y, rot) = node.at().unwrap_or((0.0, 0.0, 0.0));
        SimpleFootprint {
            position: (x, y),
            rotation: rot,
            layer: node.child_str("layer").unwrap_or("F.Cu").to_string(),
            graphics: node
                .children
                .iter()
                .filter_map(SimpleGraphic::from_sexp)
                .collect(),
        }
    }
}

impl FootprintGeometry for SimpleFootprint {
    type Graphic = SimpleGraphic;
    fn position(&self) -> Point {
        self.position
    }
    fn rotation(&self) -> f64 {
        self.rotation
    }
    fn layer(&self) -> &str {
        &self.layer
    }
    fn graphics(&self) -> &[SimpleGraphic] {
        &self.graphics
    }
}

/// Local->board transform (KiCad negates the orientation, #3739).
pub fn fp_transform<F: FootprintGeometry + ?Sized>(footprint: &F) -> impl Fn(Point) -> Point {
    let (fx, fy) = footprint.position();
    let rot = (-footprint.rotation()).to_radians();
    let (sin_r, cos_r) = rot.sin_cos();
    move |(lx, ly)| {
        (
            fx + (lx * cos_r - ly * sin_r),
            fy + (lx * sin_r + ly * cos_r),
        )
    }
}

/// `"F"` / `"B"` for a courtyard layer.
pub fn courtyard_side(layer: &str) -> Option<&'static str> {
    if layer == Layer::FCrtYd.as_str() {
        Some("F")
    } else if layer == Layer::BCrtYd.as_str() {
        Some("B")
    } else {
        None
    }
}

fn courtyard_layer(side: &str) -> &'static str {
    if side == "F" {
        Layer::FCrtYd.as_str()
    } else {
        Layer::BCrtYd.as_str()
    }
}

/// Closed 5-point ring of an `fp_rect` (local space).
pub fn rect_ring<G: GraphicGeometry + ?Sized>(graphic: &G) -> Vec<Point> {
    let (sx, sy) = graphic.start();
    let (ex, ey) = graphic.end();
    vec![(sx, sy), (ex, sy), (ex, ey), (sx, ey), (sx, sy)]
}

/// Chain segments into a single closed ring by endpoint matching; `None`
/// for open chains, branches, or multiple components.
pub fn chain_lines(segments: &[(Point, Point)]) -> Option<Vec<Point>> {
    let close = |a: Point, b: Point| (a.0 - b.0).hypot(a.1 - b.1) <= CHAIN_EPSILON_MM;
    let mut remaining: Vec<(Point, Point)> = segments.to_vec();
    if remaining.is_empty() {
        return None;
    }
    let (start, mut current) = remaining.remove(0);
    let mut ring = vec![start, current];
    while !remaining.is_empty() {
        let mut found = None;
        for (idx, (a, b)) in remaining.iter().enumerate() {
            if close(*a, current) {
                found = Some((idx, *b));
                break;
            }
            if close(*b, current) {
                found = Some((idx, *a));
                break;
            }
        }
        let (idx, next) = found?;
        current = next;
        ring.push(current);
        remaining.remove(idx);
    }
    if !close(ring[0], *ring.last().unwrap()) {
        return None;
    }
    Some(ring)
}

/// Polygon from a ring; invalid (self-intersecting) rings are repaired by a
/// boolean self-union (shapely's `buffer(0)`). `None` when empty/zero-area.
pub(crate) fn valid_polygon(ring: Vec<Point>) -> Option<MultiPolygon<f64>> {
    let coords: Vec<Coord> = ring.into_iter().map(|(x, y)| Coord { x, y }).collect();
    let polygon = Polygon::new(LineString::from(coords), vec![]);
    let geometry = if polygon.is_valid() {
        MultiPolygon::new(vec![polygon])
    } else {
        let mp = MultiPolygon::new(vec![polygon]);
        mp.union(&MultiPolygon::new(vec![]))
    };
    if geometry.0.is_empty() || geometry.unsigned_area() <= 0.0 {
        return None;
    }
    Some(geometry)
}

/// Board-frame courtyard polygon on `side` (`"F"` / `"B"`).
pub fn courtyard_polygon<F: FootprintGeometry + ?Sized>(
    footprint: &F,
    side: &str,
) -> Option<MultiPolygon<f64>> {
    let target = courtyard_layer(side);
    let transform = fp_transform(footprint);
    let mut rects = Vec::new();
    let mut polys = Vec::new();
    let mut lines = Vec::new();
    for g in footprint.graphics() {
        if g.layer() != target {
            continue;
        }
        match g.graphic_type() {
            "rect" => rects.push(g),
            "poly" => polys.push(g),
            "line" => lines.push((g.start(), g.end())),
            _ => {}
        }
    }
    let ring = if let Some(rect) = rects.first() {
        Some(rect_ring(*rect))
    } else if let Some(poly) = polys.first().filter(|p| p.points().len() >= 3) {
        Some(poly.points().to_vec())
    } else if !lines.is_empty() {
        chain_lines(&lines)
    } else {
        None
    };
    let ring = ring.filter(|r| r.len() >= 3)?;
    valid_polygon(ring.into_iter().map(&transform).collect())
}

/// Whether the footprint has any F/B courtyard graphic.
pub fn has_courtyard_geometry<F: FootprintGeometry + ?Sized>(footprint: &F) -> bool {
    footprint
        .graphics()
        .iter()
        .any(|g| courtyard_side(g.layer()).is_some())
}

/// Whether the footprint has a courtyard graphic on `side`.
pub fn side_has_geometry<F: FootprintGeometry + ?Sized>(footprint: &F, side: &str) -> bool {
    let target = courtyard_layer(side);
    footprint.graphics().iter().any(|g| g.layer() == target)
}
