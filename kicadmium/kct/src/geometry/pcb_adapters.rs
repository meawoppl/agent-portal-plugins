//! [`FootprintGeometry`] / [`GraphicGeometry`] for the board model, plus
//! conversions from `geo` results into the shapely-kernel [`Geom`].

use super::courtyard::{FootprintGeometry, GraphicGeometry, Point};
use super::shapely::{Geom, Poly};
use crate::schema::pcb::{Footprint, FootprintGraphic};

impl GraphicGeometry for FootprintGraphic {
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

impl FootprintGeometry for Footprint {
    type Graphic = FootprintGraphic;
    fn position(&self) -> Point {
        self.position
    }
    fn rotation(&self) -> f64 {
        self.rotation
    }
    fn layer(&self) -> &str {
        &self.layer
    }
    fn graphics(&self) -> &[FootprintGraphic] {
        &self.graphics
    }
}

/// `geo` multipolygon -> kernel geometry (rings kept as given).
pub fn from_geo_multi(mp: &geo::MultiPolygon<f64>) -> Geom {
    let ring = |r: &geo::LineString<f64>| r.0.iter().map(|c| (c.x, c.y)).collect::<Vec<_>>();
    let polys: Vec<Poly> =
        mp.0.iter()
            .map(|p| Poly::with_holes(ring(p.exterior()), p.interiors().iter().map(ring).collect()))
            .collect();
    match polys.len() {
        0 => Geom::Empty,
        1 => Geom::Poly(polys.into_iter().next().unwrap()),
        _ => Geom::Multi(polys),
    }
}

/// Board-frame courtyard polygon on `side` as a kernel geometry.
pub fn courtyard_geom(fp: &Footprint, side: &str) -> Option<Geom> {
    super::courtyard::courtyard_polygon(fp, side).map(|mp| from_geo_multi(&mp))
}
