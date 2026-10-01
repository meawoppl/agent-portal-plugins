//! Copper-geometry helpers of `kicad_tools.validate.connectivity`'s
//! `ConnectivityValidator` shared with the net-status analyzer (pad / via /
//! pour geometry). The full `ConnectivityValidator` report is not ported.

use crate::geometry::shapely::{self as sh, Geom, Poly};
use crate::schema::pcb::{Footprint, Pad, Via};

/// Inward erosion applied to pad / via copper before pour bonding.
pub const POUR_PAD_ERODE: f64 = 0.1;

/// `_transform_pad_position(pad_local, fp_x, fp_y, rotation)`.
pub fn transform_pad_position(local: (f64, f64), fp_pos: (f64, f64), rotation: f64) -> (f64, f64) {
    let a = (-rotation).to_radians();
    let (c, s) = (a.cos(), a.sin());
    let (px, py) = local;
    (fp_pos.0 + (px * c - py * s), fp_pos.1 + (px * s + py * c))
}

/// `_pad_copper_polygon(fp, pad, shape_aware)`: rotated size box (or the
/// pad's circle / stadium when `shape_aware`), eroded by
/// [`POUR_PAD_ERODE`]; the un-eroded outline when erosion empties it.
pub fn pad_copper_polygon(fp: &Footprint, pad: &Pad, shape_aware: bool) -> Option<Geom> {
    let (cx, cy) = transform_pad_position(pad.position, fp.position, fp.rotation);
    let (w, h) = pad.size;
    if w <= 0.0 || h <= 0.0 {
        return Some(Geom::Point((cx, cy)));
    }
    let a = (-fp.rotation).to_radians();
    let (c, s) = (a.cos(), a.sin());
    let to_board = |ox: f64, oy: f64| (cx + ox * c - oy * s, cy + ox * s + oy * c);
    let shape = if shape_aware {
        pad.shape.to_lowercase()
    } else {
        String::new()
    };
    let base = if shape == "circle" || shape == "oval" {
        let r = (if h < w { h } else { w }) / 2.0;
        let half = (w - h).abs() / 2.0;
        if half <= 0.0 {
            sh::point_buffer((cx, cy), r)
        } else {
            let (e0, e1) = if w >= h {
                ((-half, 0.0), (half, 0.0))
            } else {
                ((0.0, -half), (0.0, half))
            };
            sh::segment_buffer(to_board(e0.0, e0.1), to_board(e1.0, e1.1), r)
        }
    } else {
        Geom::Poly(Poly::new(vec![
            to_board(-w / 2.0, -h / 2.0),
            to_board(w / 2.0, -h / 2.0),
            to_board(w / 2.0, h / 2.0),
            to_board(-w / 2.0, h / 2.0),
        ]))
    };
    let eroded = sh::erode_convex(&base, POUR_PAD_ERODE);
    if !eroded.is_empty() {
        return Some(eroded);
    }
    Some(base)
}

/// `_fill_solid_region`: the fill polygon, `buffer(0)`-repaired when invalid.
pub fn fill_solid_region(points: &[(f64, f64)]) -> Option<Geom> {
    if points.len() < 3 {
        return None;
    }
    // `Polygon(points)` then `buffer(0)` when invalid.
    let g = sh::buffer0(&Poly::new(points.to_vec()));
    (!g.is_empty()).then_some(g)
}

/// `_via_copper_geom`: eroded via disc (full disc when erosion empties it,
/// a point for degenerate vias).
pub fn via_copper_geom(pos: (f64, f64), radius: f64) -> Geom {
    if radius <= 0.0 {
        return Geom::Point(pos);
    }
    let circle = sh::point_buffer(pos, radius);
    let eroded = sh::erode_convex(&circle, POUR_PAD_ERODE);
    if !eroded.is_empty() {
        return eroded;
    }
    circle
}

/// `_physical_via_annulus`: raw annular copper (quad 64).
pub fn physical_via_annulus(via: &Via) -> Geom {
    let r = via.size.max(0.0) / 2.0;
    if r <= 0.0 {
        return Geom::Empty;
    }
    sh::annulus(via.position, r, via.drill / 2.0, 64)
}
