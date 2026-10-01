//! Pin-1 / polarity silkscreen-marker rule (port of
//! `kicad_tools.validate.rules.pin1_marker`).

use std::sync::LazyLock;

use regex::Regex;

use super::clearance::pad_polygon;
use super::silkscreen::{fp_transform, silk_side, text_bbox_geometry, SilkGraphic};
use crate::geometry::package_body::{footprint_side, package_body_polygon};
use crate::geometry::pcb_adapters::from_geo_multi;
use crate::geometry::shapely::{self as sh, Geom, Poly};
use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::schema::pcb::{Footprint, Pad, Pcb};
use crate::utils::pyfmt::g;
use crate::utils::pymath;
use crate::validate::violations::{DRCResults, DRCViolation};

pub const PIN1_MARKER_MISSING_RULE_ID: &str = "pin1_marker_missing";
pub const PIN1_MARKER_OBSCURED_RULE_ID: &str = "pin1_marker_obscured";
pub const PIN1_PAD_NUMBERS: &[&str] = &["1", "A1"];
pub const DEFAULT_SEARCH_RADIUS_MM: f64 = 2.5;
const ASYMMETRY_MARGIN_MM: f64 = 1e-3;
const MIN_VISIBLE_AREA_MM2: f64 = 1e-4;
const ARC_SEGMENTS: usize = 8;

static POLARIZED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(Diode_|LED_)|:(D|LED|CP)_|Capacitor_Tantalum").unwrap());
static EXCLUDE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"MountingHole|TestPoint|Fiducial|NetTie|Jumper|Button_Switch|^Resistor_|^Inductor_|^Fuse|:R_Array|:R_Pack|:C_|USB_C|Connector_Coaxial|BNC|SMA_|U\.FL",
    )
    .unwrap()
});

fn has_copper(p: &Pad) -> bool {
    p.layers.iter().any(|l| l.ends_with(".Cu") || l == "*.Cu")
}

/// `_arc_points`.
fn arc_points(s: (f64, f64), m: (f64, f64), e: (f64, f64)) -> Vec<(f64, f64)> {
    let ((x1, y1), (x2, y2), (x3, y3)) = (s, m, e);
    let d = 2.0 * (x1 * (y2 - y3) + x2 * (y3 - y1) + x3 * (y1 - y2));
    if d.abs() < 1e-12 {
        return vec![s, m, e];
    }
    let ux = ((x1 * x1 + y1 * y1) * (y2 - y3)
        + (x2 * x2 + y2 * y2) * (y3 - y1)
        + (x3 * x3 + y3 * y3) * (y1 - y2))
        / d;
    let uy = ((x1 * x1 + y1 * y1) * (x3 - x2)
        + (x2 * x2 + y2 * y2) * (x1 - x3)
        + (x3 * x3 + y3 * y3) * (x2 - x1))
        / d;
    let r = pymath::hypot(x1 - ux, y1 - uy);
    let a1 = (y1 - uy).atan2(x1 - ux);
    let a2 = (y2 - uy).atan2(x2 - ux);
    let a3 = (y3 - uy).atan2(x3 - ux);
    let tau = 2.0 * std::f64::consts::PI;
    let ccw = |a: f64, b: f64| (b - a).rem_euclid(tau);
    let mut sweep = ccw(a1, a3);
    if ccw(a1, a2) > sweep {
        sweep -= tau;
    }
    (0..=ARC_SEGMENTS)
        .map(|i| {
            let a = a1 + sweep * i as f64 / ARC_SEGMENTS as f64;
            (ux + r * a.cos(), uy + r * a.sin())
        })
        .collect()
}

/// `_graphic_geometry` (stroke-buffered silk graphic).
fn graphic_geometry(gr: &SilkGraphic, t: Option<&dyn Fn((f64, f64)) -> (f64, f64)>) -> Option<Geom> {
    let xf = |p: (f64, f64)| match t {
        Some(f) => f(p),
        None => p,
    };
    let half = gr.stroke_width.max(0.0) / 2.0;
    let line = match gr.graphic_type {
        "line" => {
            if gr.start == gr.end {
                return Some(sh::point_buffer(xf(gr.start), half.max(0.05)));
            }
            vec![xf(gr.start), xf(gr.end)]
        }
        "rect" => {
            let (sx, sy) = gr.start;
            let (ex, ey) = gr.end;
            [(sx, sy), (ex, sy), (ex, ey), (sx, ey), (sx, sy)]
                .into_iter()
                .map(xf)
                .collect()
        }
        "circle" => {
            let c = gr.center?;
            let r = match gr.radius {
                Some(r) if r != 0.0 => r,
                _ => pymath::dist(c, gr.end),
            };
            return Some(sh::point_buffer(xf(c), r + half));
        }
        "arc" => {
            let m = gr.mid?;
            arc_points(gr.start, m, gr.end).into_iter().map(xf).collect()
        }
        "poly" => {
            if gr.points.len() < 3 {
                return None;
            }
            let poly = Poly::new(gr.points.iter().map(|&p| xf(p)).collect());
            let g = if sh::is_valid(&poly) {
                Geom::Poly(poly)
            } else {
                sh::make_valid(&poly)
            };
            return Some(if half > 0.0 {
                sh::buffer_polygon(&g, half)
            } else {
                g
            });
        }
        _ => return None,
    };
    Some(sh::buffer_line(&line, half.max(1e-3)))
}

fn union_all(geoms: &[Geom]) -> Geom {
    if geoms.len() == 1 {
        return geoms[0].clone();
    }
    sh::unary_union(geoms)
}

#[derive(Debug, Clone)]
pub struct Pin1MarkerRule {
    pub min_pads: Option<usize>,
    pub include_references: Vec<String>,
    pub exclude_references: Vec<String>,
    pub search_radius_mm: f64,
    pub require_asymmetry: bool,
    pub include_board_silk: bool,
    pub severity: &'static str,
}

impl Default for Pin1MarkerRule {
    fn default() -> Self {
        Pin1MarkerRule {
            min_pads: Some(3),
            include_references: vec![],
            exclude_references: vec![],
            search_radius_mm: DEFAULT_SEARCH_RADIUS_MM,
            require_asymmetry: true,
            include_board_silk: true,
            severity: "warning",
        }
    }
}

impl Pin1MarkerRule {
    pub fn pin1_pads(fp: &Footprint) -> Vec<&Pad> {
        for n in PIN1_PAD_NUMBERS {
            let pads: Vec<&Pad> = fp
                .pads
                .iter()
                .filter(|p| p.number == *n && has_copper(p))
                .collect();
            if !pads.is_empty() {
                return pads;
            }
        }
        vec![]
    }

    pub fn selects(&self, fp: &Footprint) -> bool {
        if self.exclude_references.contains(&fp.reference) || Self::pin1_pads(fp).is_empty() {
            return false;
        }
        if self.include_references.contains(&fp.reference) {
            return true;
        }
        if EXCLUDE.is_match(&fp.name) {
            return false;
        }
        if POLARIZED.is_match(&fp.name) {
            return true;
        }
        let Some(min) = self.min_pads else {
            return false;
        };
        fp.pads
            .iter()
            .filter(|p| !p.number.is_empty() && has_copper(p))
            .count()
            >= min
    }

    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        let selected: Vec<&Footprint> = pcb.footprints().iter().filter(|f| self.selects(f)).collect();
        if selected.is_empty() {
            return results;
        }
        let mut board_f = Vec::new();
        let mut board_b = Vec::new();
        if self.include_board_silk {
            for gr in pcb.graphics() {
                let Some(side) = silk_side(&gr.layer) else {
                    continue;
                };
                if let Some(g) = graphic_geometry(&SilkGraphic::from(gr), None) {
                    if !g.is_empty() {
                        if side == "F" {
                            board_f.push(g);
                        } else {
                            board_b.push(g);
                        }
                    }
                }
            }
        }
        for fp in selected {
            let silk = if footprint_side(fp) == "F" {
                &board_f
            } else {
                &board_b
            };
            if let Some(v) = self.check_footprint(fp, silk) {
                results.add(v);
            }
        }
        results
    }

    fn check_footprint(&self, fp: &Footprint, board_silk: &[Geom]) -> Option<DRCViolation> {
        let pin1_numbers: Vec<String> = Self::pin1_pads(fp).iter().map(|p| p.number.clone()).collect();
        let mut pin1_geoms = Vec::new();
        let mut other_geoms = Vec::new();
        for pad in &fp.pads {
            if !has_copper(pad) {
                continue;
            }
            let Some(poly) = pad_polygon(pad, fp) else {
                continue;
            };
            if pin1_numbers.contains(&pad.number) {
                pin1_geoms.push(poly);
            } else if !pad.number.is_empty() {
                other_geoms.push(poly);
            }
        }
        if pin1_geoms.is_empty() {
            return None;
        }
        let pin1 = union_all(&pin1_geoms);
        let others = (!other_geoms.is_empty()).then(|| union_all(&other_geoms));
        let mut all = pin1_geoms.clone();
        all.extend(other_geoms.iter().cloned());
        let all_pads = union_all(&all);
        let body = package_body_polygon(fp, true).map(|(b, _)| from_geo_multi(&b));
        let own = self.silk_geometries(fp);
        let near_board: Vec<&Geom> = board_silk
            .iter()
            .filter(|g| sh::distance(g, &pin1) <= self.search_radius_mm)
            .collect();
        let mut candidates = 0usize;
        for geom in own.iter().chain(near_board.iter().copied()) {
            let d1 = sh::distance(geom, &pin1);
            if d1 > self.search_radius_mm {
                continue;
            }
            if self.require_asymmetry {
                if let Some(o) = &others {
                    if sh::distance(geom, o) <= d1 + ASYMMETRY_MARGIN_MM {
                        continue;
                    }
                }
            }
            candidates += 1;
            if is_visible(geom, &all_pads, body.as_ref()) {
                return None;
            }
        }
        if self.require_asymmetry {
            if let Some(o) = &others {
                for comp in bent_components(&own) {
                    if sh::distance(&comp, &pin1) > self.search_radius_mm {
                        continue;
                    }
                    let c = Geom::Point(sh::centroid(&comp));
                    if sh::distance(&c, o) <= sh::distance(&c, &pin1) + ASYMMETRY_MARGIN_MM {
                        continue;
                    }
                    candidates += 1;
                    if is_visible(&comp, &all_pads, body.as_ref()) {
                        return None;
                    }
                }
            }
        }
        Some(self.violation(fp, &pin1, candidates > 0))
    }

    fn silk_geometries(&self, fp: &Footprint) -> Vec<Geom> {
        let side = footprint_side(fp);
        let t = fp_transform(fp);
        let mut out = Vec::new();
        for gr in &fp.graphics {
            if silk_side(&gr.layer) != Some(side) {
                continue;
            }
            if let Some(g) = graphic_geometry(&SilkGraphic::from(gr), Some(&t)) {
                if !g.is_empty() {
                    out.push(g);
                }
            }
        }
        for text in &fp.texts {
            if text.text_type != "user" || text.hidden || text.text.trim().is_empty() {
                continue;
            }
            if silk_side(&text.layer) != Some(side) {
                continue;
            }
            if let Some(g) =
                text_bbox_geometry(&text.text, text.font_size, text.font_thickness, t(text.position))
            {
                out.push(g);
            }
        }
        out
    }

    fn violation(&self, fp: &Footprint, pin1: &Geom, obscured: bool) -> DRCViolation {
        let c = sh::centroid(pin1);
        let (rule, detail) = if obscured {
            (
                PIN1_MARKER_OBSCURED_RULE_ID,
                "has pin-1 silkscreen marks only under the package body or on pad copper, so none \
                 is visible after assembly"
                    .to_string(),
            )
        } else {
            (
                PIN1_MARKER_MISSING_RULE_ID,
                format!(
                    "has no silkscreen pin-1 / polarity marker within {}mm of pad 1",
                    g(self.search_radius_mm)
                ),
            )
        };
        DRCViolation::new(
            rule,
            self.severity,
            format!(
                "{} ({}) {detail} -- add a dot, triangle or bar next to pad 1 outside the body, \
                 or waive via .kct_waivers.json",
                fp.reference, fp.name
            ),
        )
        .at(py_round(c.0, 3), py_round(c.1, 3))
        .layer(fp.layer.clone())
        .required(self.search_radius_mm)
        .items([fp.reference.clone()])
    }
}

fn is_visible(geom: &Geom, all_pads: &Geom, body: Option<&Geom>) -> bool {
    let mut v = sh::difference(geom, all_pads);
    if let Some(b) = body {
        v = sh::difference(&v, b);
    }
    v.area() > MIN_VISIBLE_AREA_MM2
}

fn bent_components(geoms: &[Geom]) -> Vec<Geom> {
    if geoms.len() < 2 {
        return vec![];
    }
    let merged = sh::unary_union(geoms);
    let parts: Vec<Geom> = match merged {
        Geom::Multi(ps) => ps.into_iter().map(Geom::Poly).collect(),
        other => vec![other],
    };
    parts
        .into_iter()
        .filter(|p| !p.is_empty() && p.area() > 0.0 && p.area() < 0.5 * sh::convex_hull_area(p))
        .collect()
}
