//! Silkscreen rules (port of `kicad_tools.validate.rules.silkscreen`):
//! line width, text height, legacy silk-over-pad, and the geometric
//! silk-over-copper / silk-overlap / silk-edge-clearance / coverage checks.

use std::collections::HashMap;

use crate::geometry::shapely::{self as sh, Geom, Poly};
use crate::geometry::strtree::StrTree;
use crate::manufacturers::DesignRules;
use crate::pyjson::py_repr_str;
use crate::schema::pcb::{BoardGraphic, Footprint, FootprintGraphic, Pcb, Setup, Via};
use crate::utils::pymath;
use crate::validate::violations::{DRCResults, DRCViolation};

pub const SILKSCREEN_LAYERS: &[&str] = &["F.SilkS", "B.SilkS", "F.Silkscreen", "B.Silkscreen"];
pub const FRONT_SILK_LAYERS: &[&str] = &["F.SilkS", "F.Silkscreen"];
pub const BACK_SILK_LAYERS: &[&str] = &["B.SilkS", "B.Silkscreen"];
/// `kicad_tools.silkscreen._silk_defaults.SILK_EDGE_CLEARANCE_MM`.
pub const SILK_EDGE_CLEARANCE_MM: f64 = 0.2;
const TEXT_CHAR_WIDTH_FACTOR: f64 = 0.7;
const CLEARANCE_EPSILON_MM: f64 = 1e-4;
const MIN_OVERLAP_AREA_MM2: f64 = 0.05;
const MIN_SILK_OVERLAP_AREA_MM2: f64 = 0.005;
pub const MODELED_SILK_GRAPHIC_TYPES: &[&str] = &["line", "rect", "poly"];
pub const SILK_GEOMETRY_UNMODELED_RULE_ID: &str = "silk_geometry_unmodeled";

pub fn is_silkscreen_layer(layer: &str) -> bool {
    SILKSCREEN_LAYERS.contains(&layer)
}

pub fn is_library_footprint(fp: &Footprint) -> bool {
    fp.name.contains(':')
}

/// `_silk_side`.
pub fn silk_side(layer: &str) -> Option<&'static str> {
    if FRONT_SILK_LAYERS.contains(&layer) {
        Some("F")
    } else if BACK_SILK_LAYERS.contains(&layer) {
        Some("B")
    } else {
        None
    }
}

/// `_fp_transform`.
pub fn fp_transform(fp: &Footprint) -> impl Fn((f64, f64)) -> (f64, f64) {
    let (fx, fy) = fp.position;
    let r = (-fp.rotation).to_radians();
    let (c, s) = (r.cos(), r.sin());
    move |(lx, ly)| (fx + (lx * c - ly * s), fy + (lx * s + ly * c))
}

/// Python `text[:20]`.
fn prefix20(s: &str) -> String {
    s.chars().take(20).collect()
}

pub fn check_silkscreen_line_width(
    pcb: &Pcb,
    rules: &DesignRules,
    suppress_library: bool,
) -> DRCResults {
    let mut results = DRCResults::with_rules_checked(1);
    let min = rules.min_silkscreen_width_mm;
    for g in pcb.graphics() {
        if !is_silkscreen_layer(&g.layer) {
            continue;
        }
        if g.stroke_width < min && g.stroke_width > 0.0 {
            results.add(
                DRCViolation::new(
                    "silkscreen_line_width",
                    "warning",
                    format!(
                        "Silkscreen line width {:.2}mm < minimum {min:.2}mm",
                        g.stroke_width
                    ),
                )
                .at(g.start.0, g.start.1)
                .layer(g.layer.clone())
                .actual(g.stroke_width)
                .required(min)
                .items([format!("gr_{}", g.graphic_type)]),
            );
        }
    }
    for fp in pcb.footprints() {
        if suppress_library && is_library_footprint(fp) {
            for g in &fp.graphics {
                if is_silkscreen_layer(&g.layer) && g.stroke_width < min && g.stroke_width > 0.0 {
                    results.suppressed_count += 1;
                }
            }
            continue;
        }
        for g in &fp.graphics {
            if !is_silkscreen_layer(&g.layer) {
                continue;
            }
            if g.stroke_width < min && g.stroke_width > 0.0 {
                results.add(
                    DRCViolation::new(
                        "silkscreen_line_width",
                        "warning",
                        format!(
                            "Silkscreen line width {:.2}mm < minimum {min:.2}mm on {}",
                            g.stroke_width, fp.reference
                        ),
                    )
                    .at(fp.position.0, fp.position.1)
                    .layer(g.layer.clone())
                    .actual(g.stroke_width)
                    .required(min)
                    .items([fp.reference.clone(), format!("fp_{}", g.graphic_type)]),
                );
            }
        }
    }
    results
}

pub fn check_silkscreen_text_height(
    pcb: &Pcb,
    rules: &DesignRules,
    suppress_library: bool,
) -> DRCResults {
    let mut results = DRCResults::with_rules_checked(1);
    let min = rules.min_silkscreen_height_mm;
    for t in pcb.texts() {
        if !is_silkscreen_layer(&t.layer) || t.hidden {
            continue;
        }
        if t.font_height() < min {
            results.add(
                DRCViolation::new(
                    "silkscreen_text_height",
                    "warning",
                    format!(
                        "Silkscreen text height {:.2}mm < minimum {min:.2}mm",
                        t.font_height()
                    ),
                )
                .at(t.position.0, t.position.1)
                .layer(t.layer.clone())
                .actual(t.font_height())
                .required(min)
                .items([if t.text.is_empty() {
                    "gr_text".to_string()
                } else {
                    prefix20(&t.text)
                }]),
            );
        }
    }
    for fp in pcb.footprints() {
        if suppress_library && is_library_footprint(fp) {
            for t in &fp.texts {
                if is_silkscreen_layer(&t.layer) && !t.hidden && t.font_height() < min {
                    results.suppressed_count += 1;
                }
            }
            continue;
        }
        for t in &fp.texts {
            if !is_silkscreen_layer(&t.layer) || t.hidden {
                continue;
            }
            if t.font_height() < min {
                let item = format!("{} ({})", fp.reference, t.text_type);
                results.add(
                    DRCViolation::new(
                        "silkscreen_text_height",
                        "warning",
                        format!(
                            "Silkscreen text height {:.2}mm < minimum {min:.2}mm on {}",
                            t.font_height(),
                            fp.reference
                        ),
                    )
                    .at(fp.position.0, fp.position.1)
                    .layer(t.layer.clone())
                    .actual(t.font_height())
                    .required(min)
                    .items([item]),
                );
            }
        }
    }
    results
}

pub fn check_silkscreen_over_pads(pcb: &Pcb) -> DRCResults {
    let mut results = DRCResults::with_rules_checked(1);
    for fp in pcb.footprints() {
        let exposed: Vec<_> = fp.pads.iter().filter(|p| p.pad_type == "smd").collect();
        if exposed.is_empty() {
            continue;
        }
        let silk: &[&str] = if fp.layer == "F.Cu" {
            FRONT_SILK_LAYERS
        } else {
            BACK_SILK_LAYERS
        };
        for t in &fp.texts {
            if !silk.contains(&t.layer.as_str()) || t.hidden {
                continue;
            }
            for pad in &exposed {
                let dx = t.position.0 - pad.position.0;
                let dy = t.position.1 - pad.position.1;
                if dx.abs() < pad.size.0 / 2.0 && dy.abs() < pad.size.1 / 2.0 {
                    results.add(
                        DRCViolation::new(
                            "silkscreen_over_pad",
                            "warning",
                            format!(
                                "Silkscreen text may overlap exposed pad on {}",
                                fp.reference
                            ),
                        )
                        .at(fp.position.0, fp.position.1)
                        .layer(t.layer.clone())
                        .items([fp.reference.clone(), format!("pad {}", pad.number)]),
                    );
                    break;
                }
            }
        }
    }
    results
}

type Ends = ((f64, f64), (f64, f64));

fn shared_endpoint(a: Ends, b: Ends) -> Option<(f64, f64)> {
    let mut matched = Vec::new();
    let mut used = [false; 2];
    for pa in [a.0, a.1] {
        for (j, pb) in [b.0, b.1].into_iter().enumerate() {
            if used[j] {
                continue;
            }
            if pymath::dist(pa, pb) <= CLEARANCE_EPSILON_MM {
                matched.push(pa);
                used[j] = true;
                break;
            }
        }
    }
    (matched.len() == 1).then(|| matched[0])
}

/// `_text_bbox_geometry`.
pub fn text_bbox_geometry(
    text: &str,
    font_size: (f64, f64),
    thickness: f64,
    center: (f64, f64),
) -> Option<Geom> {
    let n = text.chars().count();
    if n == 0 {
        return None;
    }
    let w = font_size.0 * n as f64 * TEXT_CHAR_WIDTH_FACTOR + thickness;
    let h = font_size.1 + thickness;
    let (cx, cy) = center;
    Some(sh::box_poly(
        cx - w / 2.0,
        cy - h / 2.0,
        cx + w / 2.0,
        cy + h / 2.0,
    ))
}

/// A silk primitive viewed uniformly (footprint or board graphic).
pub struct SilkGraphic<'a> {
    pub graphic_type: &'a str,
    pub layer: &'a str,
    pub stroke_width: f64,
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub center: Option<(f64, f64)>,
    pub mid: Option<(f64, f64)>,
    pub radius: Option<f64>,
    pub points: &'a [(f64, f64)],
    pub filled: bool,
    pub uuid: &'a str,
}

impl<'a> From<&'a FootprintGraphic> for SilkGraphic<'a> {
    fn from(g: &'a FootprintGraphic) -> Self {
        SilkGraphic {
            graphic_type: &g.graphic_type,
            layer: &g.layer,
            stroke_width: g.stroke_width,
            start: g.start,
            end: g.end,
            center: g.center,
            mid: g.mid,
            radius: g.radius,
            points: &g.points,
            filled: g.is_filled(),
            uuid: &g.uuid,
        }
    }
}

impl<'a> From<&'a BoardGraphic> for SilkGraphic<'a> {
    fn from(g: &'a BoardGraphic) -> Self {
        SilkGraphic {
            graphic_type: &g.graphic_type,
            layer: &g.layer,
            stroke_width: g.stroke_width,
            start: g.start,
            end: g.end,
            center: g.center,
            mid: g.mid,
            radius: None,
            points: &g.points,
            filled: g.is_filled(),
            uuid: &g.uuid,
        }
    }
}

fn repaired(poly: Poly) -> Geom {
    if sh::is_valid(&poly) {
        Geom::Poly(poly)
    } else {
        sh::make_valid(&poly)
    }
}

fn poly_geometry(
    g: &SilkGraphic,
    xf: &dyn Fn((f64, f64)) -> (f64, f64),
    width: f64,
) -> Option<Geom> {
    let pts: Vec<(f64, f64)> = g.points.iter().map(|&p| xf(p)).collect();
    if pts.len() < 3 {
        return None;
    }
    if g.filled {
        let mut poly = repaired(Poly::new(pts));
        if width > 0.0 {
            poly = sh::buffer_polygon(&poly, width / 2.0);
        }
        return (!poly.is_empty()).then_some(poly);
    }
    if width <= 0.0 {
        return None;
    }
    let mut ring = pts.clone();
    ring.push(pts[0]);
    Some(sh::buffer_line(&ring, width / 2.0))
}

/// Optional local -> board point transform.
pub type PointTransform<'a> = Option<&'a dyn Fn((f64, f64)) -> (f64, f64)>;

/// `_stroke_geometry`.
pub fn stroke_geometry(g: &SilkGraphic, transform: PointTransform) -> Option<Geom> {
    if !MODELED_SILK_GRAPHIC_TYPES.contains(&g.graphic_type) {
        return None;
    }
    let width = g.stroke_width;
    let ident = |p: (f64, f64)| p;
    let xf: &dyn Fn((f64, f64)) -> (f64, f64) = match transform {
        Some(t) => t,
        None => &ident,
    };
    if g.graphic_type == "poly" {
        return poly_geometry(g, xf, width);
    }
    if width <= 0.0 {
        return None;
    }
    if g.graphic_type == "line" {
        let (a, b) = (xf(g.start), xf(g.end));
        if a == b {
            return None;
        }
        return Some(sh::segment_buffer(a, b, width / 2.0));
    }
    let (sx, sy) = g.start;
    let (ex, ey) = g.end;
    let ring: Vec<(f64, f64)> = [(sx, sy), (ex, sy), (ex, ey), (sx, ey), (sx, sy)]
        .into_iter()
        .map(xf)
        .collect();
    Some(sh::buffer_line(&ring, width / 2.0))
}

pub fn pad_aperture_geometry(
    pad: &crate::schema::pcb::Pad,
    pos: (f64, f64),
    min_mask: f64,
) -> Geom {
    let margin = pad.solder_mask_margin.unwrap_or(min_mask);
    let w = pad.size.0 + 2.0 * margin;
    let h = pad.size.1 + 2.0 * margin;
    let (cx, cy) = pos;
    let ap = sh::box_poly(cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0);
    let rot = pad.rotation;
    if rot.rem_euclid(360.0) != 0.0 {
        return sh::rotate_about(&ap, -rot, (cx, cy));
    }
    ap
}

pub fn via_aperture_geometry(via: &Via, min_mask: f64) -> Geom {
    let d = via.size + 2.0 * min_mask;
    let (cx, cy) = via.position;
    sh::box_poly(cx - d / 2.0, cy - d / 2.0, cx + d / 2.0, cy + d / 2.0)
}

pub fn via_is_tented(via: &Via, setup: Option<&Setup>, side: &str) -> bool {
    let o = if side == "F" {
        via.tenting_front.as_deref()
    } else {
        via.tenting_back.as_deref()
    };
    match o {
        Some("yes") => return true,
        Some("no") => return false,
        _ => {}
    }
    if let Some(s) = setup {
        let d = if side == "F" {
            s.tenting_front
        } else {
            s.tenting_back
        };
        if let Some(d) = d {
            return d;
        }
    }
    true
}

/// One silk element (`_iter_silk_geometries` row).
pub struct SilkItem {
    pub side: &'static str,
    pub geom: Geom,
    pub label: String,
    pub location: (f64, f64),
    pub layer: String,
    pub uuid: String,
    /// `fp_line` endpoints / width (for the silk-overlap joint test).
    pub line: Option<(Ends, f64)>,
}

/// `_iter_silk_geometries`.
pub fn iter_silk_geometries(pcb: &Pcb) -> Vec<SilkItem> {
    let mut out = Vec::new();
    for fp in pcb.footprints() {
        let t = fp_transform(fp);
        for text in &fp.texts {
            let Some(side) = silk_side(&text.layer) else {
                continue;
            };
            if text.hidden {
                continue;
            }
            let center = t(text.position);
            let Some(g) =
                text_bbox_geometry(&text.text, text.font_size, text.font_thickness, center)
            else {
                continue;
            };
            out.push(SilkItem {
                side,
                geom: g,
                label: format!("{} ({})", fp.reference, text.text_type),
                location: center,
                layer: text.layer.clone(),
                uuid: text.uuid.clone(),
                line: None,
            });
        }
        for gr in &fp.graphics {
            let Some(side) = silk_side(&gr.layer) else {
                continue;
            };
            let sg = SilkGraphic::from(gr);
            let Some(g) = stroke_geometry(&sg, Some(&t)) else {
                continue;
            };
            let line =
                (gr.graphic_type == "line").then(|| ((t(gr.start), t(gr.end)), gr.stroke_width));
            out.push(SilkItem {
                side,
                geom: g,
                label: format!("{} (fp_{})", fp.reference, gr.graphic_type),
                location: fp.position,
                layer: gr.layer.clone(),
                uuid: gr.uuid.clone(),
                line,
            });
        }
    }
    for text in pcb.texts() {
        let Some(side) = silk_side(&text.layer) else {
            continue;
        };
        if text.hidden {
            continue;
        }
        let Some(g) = text_bbox_geometry(
            &text.text,
            text.font_size,
            text.font_thickness,
            text.position,
        ) else {
            continue;
        };
        out.push(SilkItem {
            side,
            geom: g,
            label: if text.text.is_empty() {
                "gr_text".into()
            } else {
                prefix20(&text.text)
            },
            location: text.position,
            layer: text.layer.clone(),
            uuid: text.uuid.clone(),
            line: None,
        });
    }
    for gr in pcb.graphics() {
        let Some(side) = silk_side(&gr.layer) else {
            continue;
        };
        let Some(g) = stroke_geometry(&SilkGraphic::from(gr), None) else {
            continue;
        };
        out.push(SilkItem {
            side,
            geom: g,
            label: format!("gr_{}", gr.graphic_type),
            location: gr.start,
            layer: gr.layer.clone(),
            uuid: gr.uuid.clone(),
            line: None,
        });
    }
    out
}

pub fn check_silk_coverage(pcb: &Pcb) -> DRCResults {
    let mut results = DRCResults::with_rules_checked(1);
    type CoverageRow = (String, String, (f64, f64), String, String);
    let mut rows: Vec<CoverageRow> = Vec::new();
    for fp in pcb.footprints() {
        for g in &fp.graphics {
            if silk_side(&g.layer).is_none()
                || MODELED_SILK_GRAPHIC_TYPES.contains(&g.graphic_type.as_str())
            {
                continue;
            }
            rows.push((
                format!("{} (fp_{})", fp.reference, g.graphic_type),
                g.graphic_type.clone(),
                fp.position,
                g.layer.clone(),
                g.uuid.clone(),
            ));
        }
    }
    for g in pcb.graphics() {
        if silk_side(&g.layer).is_none()
            || MODELED_SILK_GRAPHIC_TYPES.contains(&g.graphic_type.as_str())
        {
            continue;
        }
        rows.push((
            format!("gr_{}", g.graphic_type),
            g.graphic_type.clone(),
            g.start,
            g.layer.clone(),
            g.uuid.clone(),
        ));
    }
    for (label, kind, loc, layer, uuid) in rows {
        let item = if uuid.is_empty() {
            label.clone()
        } else {
            format!("{label} {{{uuid}}}")
        };
        results.add(
            DRCViolation::new(
                SILK_GEOMETRY_UNMODELED_RULE_ID,
                "info",
                format!(
                    "Silkscreen {label} is a {} primitive that kct's silk clearance geometry does \
                     not model; it was NOT checked for silk-over-copper / silk-to-pad / \
                     silk-to-edge clearance (native DRC remains authoritative for it)",
                    py_repr_str(&kind)
                ),
            )
            .at(loc.0, loc.1)
            .layer(layer)
            .items([item]),
        );
    }
    results
}

fn tree_of(geoms: &[&Geom]) -> StrTree {
    StrTree::new(&geoms.iter().map(|g| g.bounds()).collect::<Vec<_>>())
}

pub fn check_silk_over_copper(pcb: &Pcb, rules: &DesignRules) -> DRCResults {
    let mut results = DRCResults::with_rules_checked(1);
    let min_mask = rules.min_solder_mask_clearance_mm;
    let mut aps: HashMap<&str, Vec<(Geom, String)>> = HashMap::new();
    for fp in pcb.footprints() {
        let t = fp_transform(fp);
        for pad in &fp.pads {
            let sides: Vec<&str> = match pad.pad_type.as_str() {
                "smd" => vec![if fp.layer == "F.Cu" { "F" } else { "B" }],
                "thru_hole" => vec!["F", "B"],
                _ => continue,
            };
            let g = pad_aperture_geometry(pad, t(pad.position), min_mask);
            let label = format!("{} pad {}", fp.reference, pad.number);
            for s in sides {
                aps.entry(s).or_default().push((g.clone(), label.clone()));
            }
        }
    }
    for via in pcb.vias() {
        if via.size <= 0.0 {
            continue;
        }
        for side in ["F", "B"] {
            if via_is_tented(via, pcb.setup(), side) {
                continue;
            }
            let net = if via.net_name.is_empty() {
                String::new()
            } else {
                format!(" [{}]", via.net_name)
            };
            let label = format!("via{net} at ({:.3}, {:.3})", via.position.0, via.position.1);
            aps.entry(side)
                .or_default()
                .push((via_aperture_geometry(via, min_mask), label));
        }
    }
    let trees: HashMap<&str, StrTree> = aps
        .iter()
        .map(|(s, e)| (*s, tree_of(&e.iter().map(|x| &x.0).collect::<Vec<_>>())))
        .collect();
    let mut found = Vec::new();
    for item in iter_silk_geometries(pcb) {
        let Some(entries) = aps.get(item.side) else {
            continue;
        };
        let Some(b) = item.geom.bounds() else {
            continue;
        };
        for idx in trees[item.side].query(b) {
            let (ag, alabel) = &entries[idx];
            if !sh::intersects(&item.geom, ag) {
                continue;
            }
            if sh::intersection(&item.geom, ag).area() < MIN_OVERLAP_AREA_MM2 {
                continue;
            }
            found.push(
                DRCViolation::new(
                    "silk_over_copper",
                    "warning",
                    format!(
                        "Silkscreen {} overlaps exposed copper of {alabel}",
                        item.label
                    ),
                )
                .at(item.location.0, item.location.1)
                .layer(item.layer.clone())
                .items([item.label.clone(), alabel.clone()]),
            );
        }
    }
    found.sort_by(|a, b| (&a.items[0], &a.items[1]).cmp(&(&b.items[0], &b.items[1])));
    for v in found {
        results.add(v);
    }
    results
}

pub fn check_silk_overlap(pcb: &Pcb) -> DRCResults {
    let mut results = DRCResults::with_rules_checked(1);
    let items = iter_silk_geometries(pcb);
    let mut found = Vec::new();
    for side in ["F", "B"] {
        let entries: Vec<&SilkItem> = items.iter().filter(|i| i.side == side).collect();
        if entries.len() < 2 {
            continue;
        }
        let tree = tree_of(&entries.iter().map(|e| &e.geom).collect::<Vec<_>>());
        for (i, a) in entries.iter().enumerate() {
            let Some(b) = a.geom.bounds() else { continue };
            for j in tree.query(b) {
                if j <= i {
                    continue;
                }
                let o = entries[j];
                if !sh::intersects(&a.geom, &o.geom) {
                    continue;
                }
                let overlap = sh::intersection(&a.geom, &o.geom);
                if overlap.area() < MIN_SILK_OVERLAP_AREA_MM2 {
                    continue;
                }
                if a.label == o.label {
                    if let (Some((ea, wa)), Some((eb, wb))) = (a.line, o.line) {
                        if let Some(jp) = shared_endpoint(ea, eb) {
                            let end_a = if ea.0 == jp { ea.1 } else { ea.0 };
                            let (ob, endb) = if pymath::dist(eb.0, jp) <= CLEARANCE_EPSILON_MM {
                                eb
                            } else {
                                (eb.1, eb.0)
                            };
                            let da = (end_a.0 - jp.0, end_a.1 - jp.1);
                            let db = (endb.0 - ob.0, endb.1 - ob.1);
                            let collinear = da.0 * db.0 + da.1 * db.1 > 0.0
                                && (da.0 * db.1 - da.1 * db.0).abs()
                                    <= 1e-9 * pymath::hypot(da.0, da.1) * pymath::hypot(db.0, db.1);
                            let joint = sh::point_buffer(jp, wa.max(wb));
                            if !collinear && sh::difference(&overlap, &joint).area() < 1e-9 {
                                continue;
                            }
                        }
                    }
                }
                let ia = if a.uuid.is_empty() {
                    a.label.clone()
                } else {
                    format!("{} {{{}}}", a.label, a.uuid)
                };
                let ib = if o.uuid.is_empty() {
                    o.label.clone()
                } else {
                    format!("{} {{{}}}", o.label, o.uuid)
                };
                found.push(
                    DRCViolation::new(
                        "silk_overlap",
                        "warning",
                        format!("Silkscreen {} overlaps silkscreen {}", a.label, o.label),
                    )
                    .at(a.location.0, a.location.1)
                    .layer(a.layer.clone())
                    .items([ia, ib]),
                );
            }
        }
    }
    found.sort_by(|a, b| (&a.items[0], &a.items[1]).cmp(&(&b.items[0], &b.items[1])));
    for v in found {
        results.add(v);
    }
    results
}

pub fn check_silk_edge_clearance(pcb: &Pcb) -> DRCResults {
    let mut results = DRCResults::with_rules_checked(1);
    let segs = pcb.get_board_outline_segments();
    if segs.is_empty() {
        return results;
    }
    let outline = sh::Prepared::new(Geom::Lines(
        segs.iter().map(|(a, b)| vec![*a, *b]).collect(),
    ));
    for item in iter_silk_geometries(pcb) {
        let d = sh::distance_prep(&item.geom, &outline);
        if d < SILK_EDGE_CLEARANCE_MM - CLEARANCE_EPSILON_MM {
            results.add(
                DRCViolation::new(
                    "silk_edge_clearance",
                    "warning",
                    format!(
                        "Silkscreen {} to board edge {d:.3}mm < minimum {SILK_EDGE_CLEARANCE_MM:.2}mm",
                        item.label
                    ),
                )
                .at(item.location.0, item.location.1)
                .layer(item.layer.clone())
                .actual(d)
                .required(SILK_EDGE_CLEARANCE_MM)
                .items([item.label.clone(), "Edge.Cuts".to_string()]),
            );
        }
    }
    results
}

/// `check_all_silkscreen`.
pub fn check_all_silkscreen(pcb: &Pcb, rules: &DesignRules, suppress_library: bool) -> DRCResults {
    let mut results = DRCResults::new();
    results.merge(check_silkscreen_line_width(pcb, rules, suppress_library));
    results.merge(check_silkscreen_text_height(pcb, rules, suppress_library));
    results.merge(check_silkscreen_over_pads(pcb));
    results.merge(check_silk_over_copper(pcb, rules));
    results.merge(super::factory_clearance::check_silk_pad_clearance(
        pcb, rules,
    ));
    results.merge(check_silk_overlap(pcb));
    results.merge(check_silk_edge_clearance(pcb));
    results.merge(check_silk_coverage(pcb));
    results
}
