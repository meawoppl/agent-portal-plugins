//! Copper clearance rules (port of `kicad_tools.validate.rules.clearance`):
//! same-layer element spacing, net-0 bridges, and segment / via / pad
//! clearance to foreign-net zone fills.

use std::collections::HashMap;

use crate::core::geometry::{
    point_to_segment_distance, rotate_pad_offset, segment_to_segment_distance, segments_intersect,
};
use crate::core::layers::via_spans_layer;
use crate::geometry::shapely::{self as sh, Geom, Poly};
use crate::geometry::strtree::StrTree;
use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::schema::pcb::{Footprint, Pad, Pcb, Segment, Via};
use crate::validate::rules::DRC_TOLERANCE;
use crate::validate::spatial::candidate_pairs;
use crate::validate::violations::{DRCResults, DRCViolation};

const COLOCATION_EPSILON_MM: f64 = 1e-4;
const COUPLING_MAX_GAP_MM: f64 = 0.5;
const COUPLING_MIN_PARALLEL_LEN_MM: f64 = 1.0;
const COUPLING_ANGLE_TOL_DEG: f64 = 20.0;
const POLYGON_DIVERGENT_SHAPES: &[&str] = &["roundrect", "oval", "obround"];

/// `seg.uuid[:8]` (first 8 characters).
pub fn uuid8(uuid: &str) -> String {
    uuid.chars().take(8).collect()
}

/// A copper element for clearance checking (upstream `CopperElement`).
#[derive(Debug, Clone)]
pub struct CopperElement<'a> {
    pub element_type: &'static str,
    pub layer: String,
    pub net_number: i64,
    /// Segments: `[x1, y1, x2, y2, w]`; pads/vias: `[cx, cy, w, h]`.
    pub geometry: Vec<f64>,
    pub reference: String,
    pub net_name: String,
    pub explicit_layers: Vec<String>,
    pub polygon: Option<Geom>,
    pub pad_type: String,
    pub source_pad: Option<&'a Pad>,
    pub source_footprint: Option<&'a Footprint>,
    pub pad_shape: String,
    seg_geom: std::cell::OnceCell<Geom>,
    pad_geom: std::cell::OnceCell<Option<Geom>>,
}

impl<'a> CopperElement<'a> {
    fn base(element_type: &'static str, layer: &str, net: i64, geometry: Vec<f64>, reference: String, net_name: String) -> Self {
        CopperElement {
            element_type,
            layer: layer.to_string(),
            net_number: net,
            geometry,
            reference,
            net_name,
            explicit_layers: Vec::new(),
            polygon: None,
            pad_type: String::new(),
            source_pad: None,
            source_footprint: None,
            pad_shape: String::new(),
            seg_geom: Default::default(),
            pad_geom: Default::default(),
        }
    }

    pub fn from_segment(seg: &Segment) -> Self {
        Self::base(
            "segment",
            &seg.layer,
            seg.net_number,
            vec![seg.start.0, seg.start.1, seg.end.0, seg.end.1, seg.width],
            if seg.uuid.is_empty() {
                "Trace".into()
            } else {
                format!("Trace-{}", uuid8(&seg.uuid))
            },
            if seg.net_number != 0 {
                seg.net_name.clone()
            } else {
                String::new()
            },
        )
    }

    pub fn from_pad(pad: &'a Pad, fp: &'a Footprint) -> Self {
        let (x, y) = transform_pad_position(pad, fp);
        let (w, h) = transform_pad_dimensions(pad);
        let mut e = Self::base(
            "pad",
            "*",
            pad.net_number,
            vec![x, y, w, h],
            format!("{}-{}", fp.reference, pad.number),
            if pad.net_number != 0 {
                pad.net_name.clone()
            } else {
                String::new()
            },
        );
        e.polygon = pad_polygon(pad, fp);
        e.pad_type = pad.pad_type.clone();
        e.source_pad = Some(pad);
        e.source_footprint = Some(fp);
        e.pad_shape = pad.shape.clone();
        e
    }

    pub fn from_via(via: &Via) -> Self {
        let mut e = Self::base(
            "via",
            "*",
            via.net_number,
            vec![via.position.0, via.position.1, via.size, via.size],
            if via.uuid.is_empty() {
                "Via".into()
            } else {
                format!("Via-{}", uuid8(&via.uuid))
            },
            if via.net_number != 0 {
                via.net_name.clone()
            } else {
                String::new()
            },
        );
        e.explicit_layers = via.layers.clone();
        e
    }

    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let g = &self.geometry;
        if self.element_type == "segment" {
            let r = g[4] / 2.0;
            return (
                g[0].min(g[2]) - r,
                g[1].min(g[3]) - r,
                g[0].max(g[2]) + r,
                g[1].max(g[3]) + r,
            );
        }
        let (x, y, mut w, mut h) = (g[0], g[1], g[2], g[3]);
        if self.polygon.is_none() || (w - h).abs() < 0.001 {
            let m = if w > h { w } else { h };
            w = m;
            h = m;
        }
        let mut b = (x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0);
        if let Some(p) = &self.polygon {
            if let Some(pb) = p.bounds() {
                b = (b.0.min(pb.0), b.1.min(pb.1), b.2.max(pb.2), b.3.max(pb.3));
            }
        }
        b
    }

    fn same_footprint(&self, other: &CopperElement) -> bool {
        match (self.source_footprint, other.source_footprint) {
            (Some(a), Some(b)) => std::ptr::eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    /// `_segment_copper_geom`.
    fn segment_geom(&self) -> &Geom {
        self.seg_geom.get_or_init(|| {
            let g = &self.geometry;
            let half = g[4] / 2.0;
            let (a, b) = ((g[0], g[1]), (g[2], g[3]));
            if half > 0.0 {
                sh::segment_buffer(a, b, half)
            } else if a == b {
                Geom::Point(a)
            } else {
                Geom::Line(vec![a, b])
            }
        })
    }

    /// `_element_to_shapely_geom`.
    pub fn copper_geom(&self) -> Option<&Geom> {
        self.pad_geom
            .get_or_init(|| {
                if let Some(p) = &self.polygon {
                    return Some(p.clone());
                }
                let g = &self.geometry;
                let r = (if g[2] > g[3] { g[2] } else { g[3] }) / 2.0;
                if r <= 0.0 {
                    return None;
                }
                Some(sh::point_buffer((g[0], g[1]), r))
            })
            .as_ref()
    }
}

/// `_transform_pad_position`.
pub fn transform_pad_position(pad: &Pad, fp: &Footprint) -> (f64, f64) {
    let (rx, ry) = rotate_pad_offset(pad.position.0, pad.position.1, fp.rotation);
    (fp.position.0 + rx, fp.position.1 + ry)
}

/// `_transform_pad_dimensions` (board-frame AABB size).
pub fn transform_pad_dimensions(pad: &Pad) -> (f64, f64) {
    let (w, h) = pad.size;
    let rot = pad.rotation.rem_euclid(360.0);
    if (rot - 90.0).abs() < 0.001 || (rot - 270.0).abs() < 0.001 {
        return (h, w);
    }
    if rot.abs() < 0.001 || (rot - 180.0).abs() < 0.001 {
        return (w, h);
    }
    let a = rot.to_radians();
    let (c, s) = (a.cos().abs(), a.sin().abs());
    (w * c + h * s, w * s + h * c)
}

/// `_pad_polygon`: true copper outline in board coordinates.
pub fn pad_polygon(pad: &Pad, fp: &Footprint) -> Option<Geom> {
    let (cx, cy) = transform_pad_position(pad, fp);
    let (w, h) = pad.size;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let rot = pad.rotation;
    let shape = pad.shape.as_str();
    let min = if h < w { h } else { w };
    if shape == "circle" || (matches!(shape, "oval" | "obround") && (w - h).abs() < 1e-6) {
        return Some(sh::point_buffer((cx, cy), min / 2.0));
    }
    let poly = if matches!(shape, "oval" | "obround") {
        let r = min / 2.0;
        if w >= h {
            sh::box_buffer(-(w / 2.0 - r), 0.0, w / 2.0 - r, 0.0, r)
        } else {
            sh::box_buffer(0.0, -(h / 2.0 - r), 0.0, h / 2.0 - r, r)
        }
    } else if shape == "roundrect" {
        let r = pad.roundrect_rratio * min;
        if r <= 0.0 {
            sh::box_poly(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0)
        } else {
            sh::box_buffer(-(w / 2.0 - r), -(h / 2.0 - r), w / 2.0 - r, h / 2.0 - r, r)
        }
    } else {
        sh::box_poly(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0)
    };
    let poly = sh::rotate(&poly, -rot);
    Some(sh::translate(&poly, cx, cy))
}

/// `_pad_on_layer`.
pub fn pad_on_layer(pad: &Pad, layer: &str) -> bool {
    pad.layers.iter().any(|l| l == "*.Cu" || l == layer)
}

fn segments_are_coupled(a: &Segment, b: &Segment) -> f64 {
    let (ax1, ay1) = a.start;
    let (ax2, ay2) = a.end;
    let (bx1, by1) = b.start;
    let (bx2, by2) = b.end;
    let (adx, ady) = (ax2 - ax1, ay2 - ay1);
    let (bdx, bdy) = (bx2 - bx1, by2 - by1);
    let al = adx.hypot(ady);
    let bl = bdx.hypot(bdy);
    if al < 1e-9 || bl < 1e-9 {
        return 0.0;
    }
    let cos = ((adx * bdx + ady * bdy).abs() / (al * bl)).clamp(-1.0, 1.0);
    if cos.acos().to_degrees() > COUPLING_ANGLE_TOL_DEG {
        return 0.0;
    }
    let center = segment_to_segment_distance(ax1, ay1, ax2, ay2, bx1, by1, bx2, by2);
    if center - (a.width + b.width) / 2.0 > COUPLING_MAX_GAP_MM {
        return 0.0;
    }
    let (ux, uy) = (adx / al, ady / al);
    let t1 = (bx1 - ax1) * ux + (by1 - ay1) * uy;
    let t2 = (bx2 - ax1) * ux + (by2 - ay1) * uy;
    let lo = 0.0f64.max(t1.min(t2));
    let hi = al.min(t1.max(t2));
    let o = hi - lo;
    if o > 0.0 {
        o
    } else {
        0.0
    }
}

fn next_down(x: f64) -> f64 {
    if x.is_nan() || x == f64::NEG_INFINITY {
        return x;
    }
    if x == 0.0 {
        return -f64::from_bits(1);
    }
    let b = x.to_bits();
    f64::from_bits(if x > 0.0 { b - 1 } else { b + 1 })
}

fn next_up(x: f64) -> f64 {
    -next_down(-x)
}

fn pair_is_geometrically_coupled(pcb: &Pcb, net_a: i64, net_b: i64) -> bool {
    let mut coupled = 0.0;
    for layer in pcb.copper_layers() {
        let mut sa = Vec::new();
        let mut sb = Vec::new();
        for s in pcb.segments_on_layer(&layer.name) {
            if s.net_number == net_a {
                sa.push(s);
            } else if s.net_number == net_b {
                sb.push(s);
            }
        }
        if sa.is_empty() || sb.is_empty() {
            continue;
        }
        let segs: Vec<&Segment> = sa.iter().chain(sb.iter()).copied().collect();
        let bounds: Vec<(f64, f64, f64, f64)> = segs
            .iter()
            .map(|s| {
                (
                    next_down(s.start.0.min(s.end.0) - s.width / 2.0),
                    next_down(s.start.1.min(s.end.1) - s.width / 2.0),
                    next_up(s.start.0.max(s.end.0) + s.width / 2.0),
                    next_up(s.start.1.max(s.end.1) + s.width / 2.0),
                )
            })
            .collect();
        let malformed = segs.iter().any(|s| {
            s.width < 0.0
                || ![s.start.0, s.start.1, s.end.0, s.end.1, s.width]
                    .iter()
                    .all(|v| v.is_finite())
        }) || bounds
            .iter()
            .any(|b| ![b.0, b.1, b.2, b.3].iter().all(|v| v.is_finite()));
        let split = sa.len();
        let pairs: Vec<(&Segment, &Segment)> = if malformed {
            sa.iter()
                .flat_map(|a| sb.iter().map(move |b| (*a, *b)))
                .collect()
        } else {
            candidate_pairs(&bounds, next_up(COUPLING_MAX_GAP_MM))
                .into_iter()
                .filter(|&(i, j)| i < split && split <= j)
                .map(|(i, j)| (segs[i], segs[j]))
                .collect()
        };
        for (a, b) in pairs {
            coupled += segments_are_coupled(a, b);
            if coupled >= COUPLING_MIN_PARALLEL_LEN_MM {
                return true;
            }
        }
    }
    false
}

/// `_build_diff_pair_set`.
pub fn build_diff_pair_set(pcb: &Pcb) -> Vec<(i64, i64)> {
    let names: Vec<(i64, String)> = pcb.nets().iter().map(|n| (n.number, n.name.clone())).collect();
    let mut out = Vec::new();
    for dp in crate::router::diffpair::detect_differential_pairs(&names) {
        let (p, n) = (dp.positive.net_id, dp.negative.net_id);
        if p == 0 || n == 0 {
            continue;
        }
        let key = if p <= n { (p, n) } else { (n, p) };
        if pair_is_geometrically_coupled(pcb, p, n) && !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

fn calculate_clearance(e1: &CopperElement, e2: &CopperElement) -> (f64, f64, f64) {
    match (e1.element_type, e2.element_type) {
        ("segment", "segment") => segment_segment_clearance(e1, e2),
        ("segment", _) => segment_circle_clearance(e1, e2),
        (_, "segment") => segment_circle_clearance(e2, e1),
        _ => circle_circle_clearance(e1, e2),
    }
}

fn segment_segment_clearance(a: &CopperElement, b: &CopperElement) -> (f64, f64, f64) {
    let g = &a.geometry;
    let h = &b.geometry;
    let center = segment_to_segment_distance(g[0], g[1], g[2], g[3], h[0], h[1], h[2], h[3]);
    let clearance = center - g[4] / 2.0 - h[4] / 2.0;
    (
        clearance,
        (g[0] + g[2] + h[0] + h[2]) / 4.0,
        (g[1] + g[3] + h[1] + h[3]) / 4.0,
    )
}

fn segment_circle_clearance(seg: &CopperElement, circle: &CopperElement) -> (f64, f64, f64) {
    let s = &seg.geometry;
    let (x1, y1, x2, y2, sw) = (s[0], s[1], s[2], s[3], s[4]);
    let c = &circle.geometry;
    let (cx, cy, w, h) = (c[0], c[1], c[2], c[3]);
    let half = sw / 2.0;
    let mut poly_clearance = None;
    if circle.polygon.is_some() && POLYGON_DIVERGENT_SHAPES.contains(&circle.pad_shape.as_str()) {
        poly_clearance = segment_polygon_clearance(seg, circle);
    }
    let clearance = if let Some(pc) = poly_clearance {
        pc
    } else if circle.element_type == "via" || (w - h).abs() < 0.001 {
        let radius = (if w > h { w } else { h }) / 2.0;
        point_to_segment_distance(cx, cy, x1, y1, x2, y2) - half - radius
    } else {
        rect_segment_centerline_distance(cx, cy, w, h, x1, y1, x2, y2) - half
    };
    (clearance, cx, cy)
}

fn segment_polygon_clearance(seg: &CopperElement, circle: &CopperElement) -> Option<f64> {
    let pad = circle.polygon.as_ref()?;
    let sg = seg.segment_geom();
    let inter = if sh::intersects(sg, pad) {
        sh::intersection(sg, pad)
    } else {
        Geom::Empty
    };
    if !inter.is_empty() && inter.area() > 0.0 {
        let (a, b, c, d) = inter.bounds()?;
        let w = c - a;
        let h = d - b;
        return Some(-(if w > h { w } else { h }));
    }
    Some(sh::distance(sg, pad))
}

#[allow(clippy::too_many_arguments)]
fn rect_segment_centerline_distance(
    cx: f64,
    cy: f64,
    w: f64,
    h: f64,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
) -> f64 {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let (left, right, bot, top) = (cx - hw, cx + hw, cy - hh, cy + hh);
    let inside = |px: f64, py: f64| left <= px && px <= right && bot <= py && py <= top;
    let (p1, p2) = (inside(x1, y1), inside(x2, y2));
    if p1 && p2 {
        let depth = |px: f64, py: f64| {
            let gx = (px - right).max(left - px);
            let gy = (py - top).max(bot - py);
            if gy > gx {
                gy
            } else {
                gx
            }
        };
        let d1 = depth(x1, y1);
        let d2 = depth(x2, y2);
        let mut deepest = if d2 < d1 { d2 } else { d1 };
        let steps = 32;
        let (dx, dy) = (x2 - x1, y2 - y1);
        for i in 1..steps {
            let t = i as f64 / steps as f64;
            let d = depth(x1 + t * dx, y1 + t * dy);
            if d < deepest {
                deepest = d;
            }
        }
        return deepest;
    }
    if p1 != p2 {
        return 0.0;
    }
    for (ex1, ey1, ex2, ey2) in [
        (left, bot, right, bot),
        (right, bot, right, top),
        (right, top, left, top),
        (left, top, left, bot),
    ] {
        if segments_intersect(x1, y1, x2, y2, ex1, ey1, ex2, ey2) {
            return 0.0;
        }
    }
    let to_rect = |px: f64, py: f64| {
        let qx = left.max(px.min(right));
        let qy = bot.max(py.min(top));
        ((px - qx) * (px - qx) + (py - qy) * (py - qy)).sqrt()
    };
    let mut cands = vec![to_rect(x1, y1), to_rect(x2, y2)];
    for (ox, oy) in [(left, bot), (right, bot), (right, top), (left, top)] {
        cands.push(point_to_segment_distance(ox, oy, x1, y1, x2, y2));
    }
    py_min(&cands)
}

/// Python `min(list)`: first minimum, NaN-unaware like CPython's `<`.
pub fn py_min(v: &[f64]) -> f64 {
    let mut m = v[0];
    for &x in &v[1..] {
        if x < m {
            m = x;
        }
    }
    m
}

fn polygon_pair_clearance(c1: &CopperElement, c2: &CopperElement) -> Option<(f64, f64, f64)> {
    let g1 = c1.copper_geom()?;
    let g2 = c2.copper_geom()?;
    let inter = if sh::intersects(g1, g2) {
        sh::intersection(g1, g2)
    } else {
        Geom::Empty
    };
    if !inter.is_empty() && inter.area() > 0.0 {
        let (a, b, c, d) = inter.bounds()?;
        let (w, h) = (c - a, d - b);
        let depth = if w > h { w } else { h };
        return Some((-depth, (a + c) / 2.0, (b + d) / 2.0));
    }
    let (dist, p1, p2) = sh::distance_points(g1, g2)?;
    Some((dist, (p1.0 + p2.0) / 2.0, (p1.1 + p2.1) / 2.0))
}

fn circle_circle_clearance(c1: &CopperElement, c2: &CopperElement) -> (f64, f64, f64) {
    let (x1, y1, w1, h1) = (c1.geometry[0], c1.geometry[1], c1.geometry[2], c1.geometry[3]);
    let (x2, y2, w2, h2) = (c2.geometry[0], c2.geometry[1], c2.geometry[2], c2.geometry[3]);
    if c1.polygon.is_some() || c2.polygon.is_some() {
        if let Some(r) = polygon_pair_clearance(c1, c2) {
            return r;
        }
    }
    let circ1 = c1.element_type == "via" || (w1 - h1).abs() < 0.001;
    let circ2 = c2.element_type == "via" || (w2 - h2).abs() < 0.001;
    let clearance = if circ1 && circ2 {
        ((x2 - x1) * (x2 - x1) + (y2 - y1) * (y2 - y1)).sqrt() - w1 / 2.0 - w2 / 2.0
    } else if !circ1 && !circ2 {
        rect_rect_clearance(x1, y1, w1, h1, x2, y2, w2, h2)
    } else if circ1 {
        rect_circle_clearance(x2, y2, w2, h2, x1, y1, w1 / 2.0)
    } else {
        rect_circle_clearance(x1, y1, w1, h1, x2, y2, w2 / 2.0)
    };
    (clearance, (x1 + x2) / 2.0, (y1 + y2) / 2.0)
}

#[allow(clippy::too_many_arguments)]
fn rect_rect_clearance(cx1: f64, cy1: f64, w1: f64, h1: f64, cx2: f64, cy2: f64, w2: f64, h2: f64) -> f64 {
    let gx = (cx2 - cx1).abs() - (w1 + w2) / 2.0;
    let gy = (cy2 - cy1).abs() - (h1 + h2) / 2.0;
    if gx >= 0.0 && gy >= 0.0 {
        (gx * gx + gy * gy).sqrt()
    } else if gx >= 0.0 {
        gx
    } else if gy >= 0.0 || gy > gx {
        gy
    } else {
        gx
    }
}

fn rect_circle_clearance(cx: f64, cy: f64, w: f64, h: f64, px: f64, py: f64, r: f64) -> f64 {
    let qx = (cx - w / 2.0).max(px.min(cx + w / 2.0));
    let qy = (cy - h / 2.0).max(py.min(cy + h / 2.0));
    ((px - qx) * (px - qx) + (py - qy) * (py - qy)).sqrt() - r
}

fn colocated(a: &CopperElement, b: &CopperElement) -> bool {
    let (seg, via) = match (a.element_type, b.element_type) {
        ("segment", "via") => (a, b),
        ("via", "segment") => (b, a),
        _ => return false,
    };
    let s = &seg.geometry;
    let (vx, vy) = (via.geometry[0], via.geometry[1]);
    (s[0] - vx).hypot(s[1] - vy) < COLOCATION_EPSILON_MM
        || (s[2] - vx).hypot(s[3] - vy) < COLOCATION_EPSILON_MM
}

#[derive(Debug, Clone, Default)]
pub struct ClearanceRule;

impl ClearanceRule {
    pub fn check(&self, pcb: &Pcb, rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        let min = rules.min_clearance_mm;
        let dps = build_diff_pair_set(pcb);
        let layers = pcb.copper_layers();
        for layer in &layers {
            for v in self.check_layer(pcb, &layer.name, min, &dps, rules.min_smd_pad_clearance_mm) {
                results.add(v);
            }
            for v in self.check_net0_bridges(pcb, &layer.name, min) {
                results.add(v);
            }
        }
        results.merge(super::factory_clearance::check_pth_hole_clearance(pcb, rules));
        results.rules_checked += layers.len() as i64;
        results
    }

    pub fn collect_elements<'a>(&self, pcb: &'a Pcb, layer: &str) -> Vec<CopperElement<'a>> {
        let mut out = Vec::new();
        for s in pcb.segments_on_layer(layer) {
            out.push(CopperElement::from_segment(s));
        }
        for fp in pcb.footprints() {
            for pad in &fp.pads {
                if pad.layers.iter().any(|l| l == layer || l == "*.Cu") {
                    out.push(CopperElement::from_pad(pad, fp));
                }
            }
        }
        for v in pcb.vias() {
            if via_spans_layer(&v.layers, layer) {
                out.push(CopperElement::from_via(v));
            }
        }
        out
    }

    fn check_layer(
        &self,
        pcb: &Pcb,
        layer: &str,
        min: f64,
        dps: &[(i64, i64)],
        min_smd: Option<f64>,
    ) -> Vec<DRCViolation> {
        let mut out = Vec::new();
        let els = self.collect_elements(pcb, layer);
        let bounds: Vec<_> = els.iter().map(CopperElement::bounds).collect();
        let search = 0.0f64.max(min).max(min_smd.unwrap_or(0.0));
        for (i, j) in candidate_pairs(&bounds, search + DRC_TOLERANCE) {
            let (e1, e2) = (&els[i], &els[j]);
            if e1.net_number == e2.net_number || e1.net_number == 0 || e2.net_number == 0 {
                continue;
            }
            if e1.element_type != "segment" && e2.element_type != "segment" {
                let off = |e: &CopperElement| {
                    e.element_type == "via"
                        && !e.explicit_layers.is_empty()
                        && !e.explicit_layers.iter().any(|l| l == layer)
                };
                if off(e1) || off(e2) {
                    continue;
                }
            }
            if e1.element_type == "segment" && e2.element_type == "segment" && !dps.is_empty() {
                let key = if e1.net_number <= e2.net_number {
                    (e1.net_number, e2.net_number)
                } else {
                    (e2.net_number, e1.net_number)
                };
                if dps.contains(&key) {
                    continue;
                }
            }
            if colocated(e1, e2) {
                continue;
            }
            let (clearance, lx, ly) = calculate_clearance(e1, e2);
            let mut required = min;
            if e1.pad_type == "smd" && e2.pad_type == "smd" && !e1.same_footprint(e2) {
                let m = min_smd.unwrap_or(0.0);
                if m > required {
                    required = m;
                }
            }
            if clearance + DRC_TOLERANCE < required {
                out.push(create_violation(e1, e2, clearance, required, layer, lx, ly));
            }
        }
        out
    }

    fn elements_touch(a: &CopperElement, b: &CopperElement) -> bool {
        // Broad phase: copper clearance is never below the envelope gap.
        let (x, y) = (a.bounds(), b.bounds());
        let m = 2.0 * COLOCATION_EPSILON_MM;
        if x.0 - m > y.2 || y.0 - m > x.2 || x.1 - m > y.3 || y.1 - m > x.3 {
            return false;
        }
        if colocated(a, b) {
            return false;
        }
        calculate_clearance(a, b).0 <= COLOCATION_EPSILON_MM
    }

    fn check_net0_bridges(&self, pcb: &Pcb, layer: &str, min: f64) -> Vec<DRCViolation> {
        let els = self.collect_elements(pcb, layer);
        let net0: Vec<&CopperElement> = els.iter().filter(|e| e.net_number == 0).collect();
        let assigned: Vec<&CopperElement> = els.iter().filter(|e| e.net_number != 0).collect();
        if net0.is_empty() || assigned.is_empty() {
            return vec![];
        }
        let mut parent: Vec<usize> = (0..net0.len()).collect();
        fn find(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        for i in 0..net0.len() {
            for j in i + 1..net0.len() {
                if Self::elements_touch(net0[i], net0[j]) {
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    if ri != rj {
                        parent[ri] = rj;
                    }
                }
            }
        }
        let mut islands: Vec<(usize, Vec<usize>)> = Vec::new();
        for i in 0..net0.len() {
            let r = find(&mut parent, i);
            match islands.iter_mut().find(|(k, _)| *k == r) {
                Some(e) => e.1.push(i),
                None => islands.push((r, vec![i])),
            }
        }
        let mut out = Vec::new();
        for (_, members) in islands {
            let mut touched: Vec<(i64, &CopperElement)> = Vec::new();
            let mut contact: Option<&CopperElement> = None;
            for &idx in &members {
                let n0 = net0[idx];
                for a in &assigned {
                    if touched.iter().any(|(n, _)| *n == a.net_number) {
                        continue;
                    }
                    if Self::elements_touch(n0, a) {
                        touched.push((a.net_number, a));
                        if contact.is_none() {
                            contact = Some(n0);
                        }
                    }
                }
            }
            if touched.len() >= 2 {
                let n0 = contact.unwrap_or(net0[members[0]]);
                let mut bridged: Vec<&CopperElement> = touched.iter().map(|(_, e)| *e).collect();
                bridged.sort_by_key(|e| e.net_number);
                let (a, b) = (bridged[0], bridged[1]);
                let na = if a.net_name.is_empty() {
                    format!("net{}", a.net_number)
                } else {
                    a.net_name.clone()
                };
                let nb = if b.net_name.is_empty() {
                    format!("net{}", b.net_number)
                } else {
                    b.net_name.clone()
                };
                let g = &n0.geometry;
                let (lx, ly) = if n0.element_type == "segment" {
                    ((g[0] + g[2]) / 2.0, (g[1] + g[3]) / 2.0)
                } else {
                    (g[0], g[1])
                };
                out.push(
                    DRCViolation::new(
                        "clearance_net0_bridge",
                        "error",
                        format!(
                            "Net-0 copper ({}) bridges nets {na} and {nb} (stray-copper short)",
                            n0.reference
                        ),
                    )
                    .at(py_round(lx, 3), py_round(ly, 3))
                    .layer(layer)
                    .actual(0.0)
                    .required(min)
                    .items([n0.reference.clone(), a.reference.clone(), b.reference.clone()])
                    .nets([na, nb]),
                );
            }
        }
        out
    }
}

fn create_violation(
    e1: &CopperElement,
    e2: &CopperElement,
    actual: f64,
    required: f64,
    layer: &str,
    lx: f64,
    ly: f64,
) -> DRCViolation {
    let mut types = [e1.element_type, e2.element_type];
    types.sort();
    let (na, nb) = (&e1.net_name, &e2.net_name);
    let message = if actual < 0.0 && !na.is_empty() && !nb.is_empty() && na != nb {
        format!(
            "SHORT: '{na}' and '{nb}' copper overlaps by {:.3}mm ({} to {})",
            -actual, e1.element_type, e2.element_type
        )
    } else {
        format!(
            "{} to {} clearance {actual:.3}mm < minimum {required:.3}mm",
            crate::pyjson::py_title(e1.element_type),
            e2.element_type
        )
    };
    DRCViolation::new(format!("clearance_{}_{}", types[0], types[1]), "error", message)
        .at(py_round(lx, 3), py_round(ly, 3))
        .layer(layer)
        .actual(py_round(actual, 4))
        .required(required)
        .items([e1.reference.clone(), e2.reference.clone()])
        .nets([na.clone(), nb.clone()])
}

// ------------------------------------------------------------- zone fills

/// One filled polygon of a zone (upstream `_ZoneFill`).
#[derive(Debug, Clone)]
pub struct ZoneFill {
    pub net_number: i64,
    pub net_name: String,
    pub layer: String,
    pub polygon: Geom,
    pub prep: std::sync::Arc<sh::Prepared>,
    pub source_clearance: f64,
    pub source_zone_id: String,
    pub source_zone_index: usize,
}

/// `_repair_fill_polygon` (`make_valid`, polygonal parts only).
pub fn repair_fill_polygon(p: &Poly) -> Geom {
    sh::make_valid(p)
}

/// `_collect_zone_fills`: `{layer: [fill]}` in first-seen layer order.
pub fn collect_zone_fills(pcb: &Pcb, include_unassigned: bool) -> Vec<(String, Vec<ZoneFill>)> {
    let name_to_number: HashMap<&str, i64> = pcb
        .nets()
        .iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| (n.name.as_str(), n.number))
        .collect();
    let mut out: Vec<(String, Vec<ZoneFill>)> = Vec::new();
    for (zi, zone) in pcb.zones().iter().enumerate() {
        let mut net = zone.net_number;
        if net == 0 && !zone.net_name.is_empty() {
            net = name_to_number.get(zone.net_name.as_str()).copied().unwrap_or(0);
        }
        if net == 0 && !include_unassigned {
            continue;
        }
        let net_name = if zone.net_name.is_empty() {
            pcb.get_net(net).map(|n| n.name.clone()).unwrap_or_default()
        } else {
            zone.net_name.clone()
        };
        for (i, pts) in zone.filled_polygons.iter().enumerate() {
            if pts.len() < 3 {
                continue;
            }
            let poly = Poly::new(pts.clone());
            let geom = if sh::is_valid(&poly) {
                Geom::Poly(poly)
            } else {
                repair_fill_polygon(&poly)
            };
            if geom.is_empty() {
                continue;
            }
            let layer = zone.filled_polygon_layer(i);
            if layer.is_empty() {
                continue;
            }
            let fill = ZoneFill {
                net_number: net,
                net_name: net_name.clone(),
                layer: layer.to_string(),
                prep: std::sync::Arc::new(sh::Prepared::new(geom.clone())),
                polygon: geom,
                source_clearance: zone.clearance,
                source_zone_id: zone.uuid.clone(),
                source_zone_index: zi,
            };
            match out.iter_mut().find(|(l, _)| l == layer) {
                Some(e) => e.1.push(fill),
                None => out.push((layer.to_string(), vec![fill])),
            }
        }
    }
    out
}

fn zone_ref(fill: &ZoneFill) -> String {
    if fill.net_name.is_empty() {
        "ZoneFill".into()
    } else {
        format!("ZoneFill-{}", fill.net_name)
    }
}

fn tree_for(fills: &[ZoneFill]) -> StrTree {
    StrTree::new(&fills.iter().map(|f| f.polygon.bounds()).collect::<Vec<_>>())
}

#[derive(Debug, Clone, Default)]
pub struct SegmentZoneClearanceRule;

impl SegmentZoneClearanceRule {
    pub fn check(&self, pcb: &Pcb, rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        let fills_by_layer = collect_zone_fills(pcb, false);
        if fills_by_layer.is_empty() {
            return results;
        }
        let min = rules.min_clearance_mm;
        for (layer, fills) in &fills_by_layer {
            let tree = tree_for(fills);
            for seg in pcb.segments_on_layer(layer) {
                if seg.net_number == 0 {
                    continue;
                }
                let half = seg.width / 2.0;
                let line = Geom::Line(vec![seg.start, seg.end]);
                let margin = half + min;
                let (x0, y0, x1, y1) = line.bounds().unwrap();
                for idx in tree.query((x0 - margin, y0 - margin, x1 + margin, y1 + margin)) {
                    let fill = &fills[idx];
                    if fill.net_number == seg.net_number {
                        continue;
                    }
                    if let Some(v) = self.check_pair(seg, &line, half, fill, min, layer) {
                        results.add(v);
                    }
                }
            }
        }
        results
    }

    fn check_pair(
        &self,
        seg: &Segment,
        line: &Geom,
        half: f64,
        fill: &ZoneFill,
        min: f64,
        layer: &str,
    ) -> Option<DRCViolation> {
        let poly = &fill.polygon;
        let (cdist, pl, pp) = sh::distance_points_prep(line, &fill.prep)?;
        let seg_net = if seg.net_number != 0 {
            seg.net_name.clone()
        } else {
            String::new()
        };
        let seg_ref = if seg.uuid.is_empty() {
            "Trace".to_string()
        } else {
            format!("Trace-{}", uuid8(&seg.uuid))
        };
        let zref = zone_ref(fill);
        if cdist == 0.0 {
            let pieces = sh::clip_segment(seg.start, seg.end, poly);
            let (rep, depth) = if pieces.is_empty() {
                (pl, 0.0)
            } else {
                let g = if pieces.len() == 1 {
                    Geom::Line(vec![pieces[0].0, pieces[0].1])
                } else {
                    Geom::Lines(pieces.iter().map(|p| vec![p.0, p.1]).collect())
                };
                let rep = line_interior_point(&g);
                (rep, sh::boundary_distance(poly, rep))
            };
            let clearance = -(depth + half);
            return Some(
                DRCViolation::new(
                    "clearance_segment_zone",
                    "error",
                    format!(
                        "Short: segment on net '{seg_net}' overlaps zone fill of net '{}' on \
                         {layer} (overlap depth {:.3}mm)",
                        fill.net_name,
                        depth + half
                    ),
                )
                .at(py_round(rep.0, 3), py_round(rep.1, 3))
                .layer(layer)
                .actual(py_round(clearance, 4))
                .required(min)
                .items([seg_ref, zref])
                .nets([seg_net, fill.net_name.clone()]),
            );
        }
        let clearance = cdist - half;
        if clearance + DRC_TOLERANCE >= min {
            return None;
        }
        let (lx, ly) = ((pl.0 + pp.0) / 2.0, (pl.1 + pp.1) / 2.0);
        let message = if clearance < 0.0 {
            format!(
                "Short: segment on net '{seg_net}' copper overlaps zone fill of net '{}' on \
                 {layer} by {:.3}mm",
                fill.net_name, -clearance
            )
        } else {
            format!(
                "Segment to zone fill clearance {clearance:.3}mm < minimum {min:.3}mm (net \
                 '{seg_net}' vs zone net '{}')",
                fill.net_name
            )
        };
        Some(
            DRCViolation::new("clearance_segment_zone", "error", message)
                .at(py_round(lx, 3), py_round(ly, 3))
                .layer(layer)
                .actual(py_round(clearance, 4))
                .required(min)
                .items([seg_ref, zref])
                .nets([seg_net, fill.net_name.clone()]),
        )
    }
}

/// GEOS `InteriorPointLine` (centroid-nearest interior vertex, else
/// endpoint).
pub fn line_interior_point(g: &Geom) -> (f64, f64) {
    let lines = g.lines();
    let (mut sx, mut sy, mut tl) = (0.0, 0.0, 0.0);
    for l in &lines {
        for w in l.windows(2) {
            let len = sh::dist(w[0], w[1]);
            let mx = (w[0].0 + w[1].0) / 2.0;
            let my = (w[0].1 + w[1].1) / 2.0;
            sx += len * mx;
            sy += len * my;
            tl += len;
        }
    }
    let c = if tl > 0.0 {
        (sx / tl, sy / tl)
    } else {
        lines.first().map(|l| l[0]).unwrap_or((0.0, 0.0))
    };
    let mut best = None;
    let mut min = f64::MAX;
    for l in &lines {
        for &p in &l[1..l.len().saturating_sub(1)] {
            let d = sh::dist(p, c);
            if d < min {
                min = d;
                best = Some(p);
            }
        }
    }
    if let Some(b) = best {
        return b;
    }
    for l in &lines {
        for p in [l[0], l[l.len() - 1]] {
            let d = sh::dist(p, c);
            if d < min {
                min = d;
                best = Some(p);
            }
        }
    }
    best.unwrap_or(c)
}

#[derive(Debug, Clone, Default)]
pub struct ViaZoneClearanceRule;

impl ViaZoneClearanceRule {
    pub fn check(&self, pcb: &Pcb, rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        let fills_by_layer = collect_zone_fills(pcb, false);
        if fills_by_layer.is_empty() {
            return results;
        }
        let min = rules.min_clearance_mm;
        for (layer, fills) in &fills_by_layer {
            let tree = tree_for(fills);
            for via in pcb.vias() {
                if via.net_number == 0 || !via_spans_layer(&via.layers, layer) {
                    continue;
                }
                let r = via.size / 2.0;
                if r <= 0.0 {
                    continue;
                }
                let shape = sh::point_buffer(via.position, r);
                let rf = if via.uuid.is_empty() {
                    "Via".to_string()
                } else {
                    format!("Via-{}", uuid8(&via.uuid))
                };
                query_and_check(
                    &tree, fills, &shape, r, via.net_number, &via.net_name, &rf, min, layer,
                    &mut results, "clearance_via_zone", "via",
                );
            }
            for fp in pcb.footprints() {
                for pad in &fp.pads {
                    if pad.net_number == 0 || !pad_on_layer(pad, layer) {
                        continue;
                    }
                    let Some(shape) = pad_polygon(pad, fp) else {
                        continue;
                    };
                    let rf = format!("{}-{}", fp.reference, pad.number);
                    query_and_check(
                        &tree, fills, &shape, 0.0, pad.net_number, &pad.net_name, &rf, min, layer,
                        &mut results, "clearance_pad_zone", "pad",
                    );
                }
            }
        }
        results
    }
}

#[allow(clippy::too_many_arguments)]
fn query_and_check(
    tree: &StrTree,
    fills: &[ZoneFill],
    shape: &Geom,
    inflate: f64,
    net_number: i64,
    net_name: &str,
    rf: &str,
    min: f64,
    layer: &str,
    results: &mut DRCResults,
    rule_id: &str,
    kind: &str,
) {
    let Some((x0, y0, x1, y1)) = shape.bounds() else {
        return;
    };
    let m = inflate + min;
    for idx in tree.query((x0 - m, y0 - m, x1 + m, y1 + m)) {
        let fill = &fills[idx];
        if fill.net_number == net_number {
            continue;
        }
        if let Some(v) = check_shape(shape, net_number, net_name, rf, fill, min, layer, rule_id, kind) {
            results.add(v);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_shape(
    shape: &Geom,
    net_number: i64,
    net_name: &str,
    rf: &str,
    fill: &ZoneFill,
    min: f64,
    layer: &str,
    rule_id: &str,
    kind: &str,
) -> Option<DRCViolation> {
    let poly = &fill.polygon;
    let label = if net_number != 0 { net_name } else { "" };
    let zref = zone_ref(fill);
    let (d, ps, pp) = sh::distance_points_prep(shape, &fill.prep)?;
    if d > 0.0 {
        if d + DRC_TOLERANCE >= min {
            return None;
        }
        let (lx, ly) = ((ps.0 + pp.0) / 2.0, (ps.1 + pp.1) / 2.0);
        return Some(
            DRCViolation::new(
                rule_id,
                "error",
                format!(
                    "{} to zone fill clearance {d:.3}mm < minimum {min:.3}mm (net '{label}' vs \
                     zone net '{}')",
                    crate::pyjson::py_title(kind),
                    fill.net_name
                ),
            )
            .at(py_round(lx, 3), py_round(ly, 3))
            .layer(layer)
            .actual(py_round(d, 4))
            .required(min)
            .items([rf.to_string(), zref])
            .nets([label.to_string(), fill.net_name.clone()]),
        );
    }
    let inter = sh::intersection(shape, poly);
    let (rep, depth) = if inter.is_empty() {
        (ps, 0.0)
    } else {
        let rep = sh::representative_point(&inter).unwrap_or(ps);
        (rep, sh::boundary_distance(poly, rep))
    };
    Some(
        DRCViolation::new(
            rule_id,
            "error",
            format!(
                "Short: {kind} on net '{label}' overlaps zone fill of net '{}' on {layer} \
                 (overlap depth {depth:.3}mm)",
                fill.net_name
            ),
        )
        .at(py_round(rep.0, 3), py_round(rep.1, 3))
        .layer(layer)
        .actual(py_round(-depth, 4))
        .required(min)
        .items([rf.to_string(), zref])
        .nets([label.to_string(), fill.net_name.clone()]),
    )
}
