//! Port of `kicad_tools.silkscreen.place_refs`: the silkscreen
//! reference-designator placement solver.
//!
//! Moves (optionally rotates) **visible** reference text just far enough to
//! clear pad/via mask apertures, other silk, component courtyards and the
//! board edge, preserving visibility, height and stroke. Only the reference
//! text's own `(at x y [angle])` is written; a reference with no clear
//! candidate is left in place and reported (`unplaceable` /
//! `under_component_fallback`). Geometry comes from the same helpers the
//! silk DRC rules use; text is an oriented approximate envelope.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use super::silk_defaults::{
    DEFAULT_CLEARANCE_MM, DEFAULT_MAX_OFFSET_MM, DEFAULT_STEP_MM, SILK_EDGE_CLEARANCE_MM,
};
use crate::geometry::courtyard::side_has_geometry;
use crate::geometry::pcb_adapters::courtyard_geom;
use crate::geometry::shapely::{self as sh, Geom, Poly};
use crate::pyjson::{py_round, Json};
use crate::schema::pcb::{
    chain_segment_indices, group_segments_by_connectivity, is_footprint_tag, Footprint, Pcb,
};
use crate::sexp::{SExp, Value};
use crate::utils::pyfmt::format_g;
use crate::validate::rules::silkscreen::{
    fp_transform, pad_aperture_geometry, silk_side, stroke_geometry, text_bbox_geometry,
    via_aperture_geometry, via_is_tented, SilkGraphic,
};

const EPSILON_MM: f64 = 1e-4;

type Labeled = Vec<(Geom, String)>;
type OutlineSegs = Vec<((f64, f64), (f64, f64))>;

fn side_index(side: &str) -> usize {
    usize::from(side != "F")
}

/// Oriented text envelope: rotate the box by `-angle` about its centre.
pub fn oriented_text_geometry(
    text: &str,
    font_size: (f64, f64),
    thickness: f64,
    center: (f64, f64),
    angle: f64,
) -> Option<Geom> {
    text_bbox_geometry(text, font_size, thickness, center)
        .map(|g| sh::rotate_about(&g, -angle, center))
}

/// Index (into the footprint node's children) of the reference text node.
fn find_reference_child(fp_node: &SExp) -> Option<usize> {
    let find = |tag: &str, key: &str| {
        fp_node.children.iter().position(|c| {
            !c.is_atom() && c.has_tag(tag) && !c.children.is_empty() && {
                let atoms: Vec<_> = c.atoms().collect();
                atoms.len() >= 2 && atoms[0].to_string() == key
            }
        })
    };
    find("fp_text", "reference").or_else(|| find("property", "Reference"))
}

/// Third token of `(at x y [angle])`, else 0.
fn read_text_angle(ref_node: &SExp) -> f64 {
    let Some(at) = ref_node.find("at") else {
        return 0.0;
    };
    let atoms: Vec<&Value> = at.atoms().collect();
    if atoms.len() >= 3 {
        atoms[2].as_f64().unwrap_or(0.0)
    } else {
        0.0
    }
}

/// `_set_text_at`: coordinate-only write of `(at x y [angle])`.
fn set_text_at(ref_node: &mut SExp, x: f64, y: f64, angle: f64) -> Result<()> {
    let Some(at) = ref_node.children.iter_mut().find(|c| c.has_tag("at")) else {
        bail!("Reference text node has no (at ...) child");
    };
    at.set_value(0, Value::Float(py_round(x, 6)));
    at.set_value(1, Value::Float(py_round(y, 6)));
    if at.atoms().count() >= 3 {
        at.set_value(2, Value::Float(py_round(angle, 6)));
    } else if angle != 0.0 {
        at.push(SExp::atom(Value::Float(py_round(angle, 6))));
    }
    Ok(())
}

/// Board -> footprint-local transform (inverse of `fp_transform`).
fn inverse_fp_transform(fp: &Footprint) -> impl Fn((f64, f64)) -> (f64, f64) {
    let (fx, fy) = fp.position;
    let theta = (-fp.rotation).to_radians();
    let (s, c) = theta.sin_cos();
    move |(x, y)| {
        let (dx, dy) = (x - fx, y - fy);
        (dx * c + dy * s, -dx * s + dy * c)
    }
}

/// `(side, geom, label)` for every silk element that never moves.
fn iter_static_silk_obstacles(pcb: &Pcb) -> Vec<(&'static str, Geom, String)> {
    let mut out = Vec::new();
    for fp in pcb.footprints() {
        let t = fp_transform(fp);
        for text in &fp.texts {
            if text.text_type == "reference" {
                continue;
            }
            let Some(side) = silk_side(&text.layer) else {
                continue;
            };
            if text.hidden {
                continue;
            }
            let center = t(text.position);
            if let Some(g) = oriented_text_geometry(
                &text.text,
                text.font_size,
                text.font_thickness,
                center,
                text.rotation,
            ) {
                out.push((side, g, format!("{} ({})", fp.reference, text.text_type)));
            }
        }
        for gr in &fp.graphics {
            let Some(side) = silk_side(&gr.layer) else {
                continue;
            };
            if let Some(g) = stroke_geometry(&SilkGraphic::from(gr), Some(&t)) {
                out.push((
                    side,
                    g,
                    format!("{} (fp_{})", fp.reference, gr.graphic_type),
                ));
            }
        }
    }
    for text in pcb.texts() {
        let Some(side) = silk_side(&text.layer) else {
            continue;
        };
        if text.hidden {
            continue;
        }
        if let Some(g) = oriented_text_geometry(
            &text.text,
            text.font_size,
            text.font_thickness,
            text.position,
            text.rotation,
        ) {
            let label = if text.text.is_empty() {
                "gr_text".to_string()
            } else {
                text.text.chars().take(20).collect()
            };
            out.push((side, g, label));
        }
    }
    for gr in pcb.graphics() {
        let Some(side) = silk_side(&gr.layer) else {
            continue;
        };
        if let Some(g) = stroke_geometry(&SilkGraphic::from(gr), None) {
            out.push((side, g, format!("gr_{}", gr.graphic_type)));
        }
    }
    out
}

/// Body bbox: courtyard, else pad extents, else a 2 mm box at the anchor.
fn component_bbox(fp: &Footprint, side: &str) -> (f64, f64, f64, f64) {
    if let Some(b) = courtyard_geom(fp, side).and_then(|g| g.bounds()) {
        return b;
    }
    let t = fp_transform(fp);
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    for pad in &fp.pads {
        let (cx, cy) = t(pad.position);
        let (hw, hh) = (pad.size.0 / 2.0, pad.size.1 / 2.0);
        xs.extend([cx - hw, cx + hw]);
        ys.extend([cy - hh, cy + hh]);
    }
    if !xs.is_empty() {
        let mn = |v: &[f64]| v.iter().cloned().fold(f64::INFINITY, f64::min);
        let mx = |v: &[f64]| v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        return (mn(&xs), mn(&ys), mx(&xs), mx(&ys));
    }
    let (fx, fy) = fp.position;
    (fx - 1.0, fy - 1.0, fx + 1.0, fy + 1.0)
}

/// Candidate centres on rings of increasing distance around `bbox`.
pub fn candidate_points(
    bbox: (f64, f64, f64, f64),
    text_w: f64,
    text_h: f64,
    max_offset_mm: f64,
    step_mm: f64,
) -> Vec<(f64, f64)> {
    let (x0, y0, x1, y1) = bbox;
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hw, hh) = (text_w / 2.0, text_h / 2.0);
    let rings = (max_offset_mm / step_mm).floor() as usize;
    let mut pts = Vec::with_capacity((rings + 1) * 8);
    for ring in 0..=rings {
        let off = ring as f64 * step_mm;
        let (top, bottom) = (y0 - off - hh, y1 + off + hh);
        let (left, right) = (x0 - off - hw, x1 + off + hw);
        pts.extend([
            (cx, top),
            (cx, bottom),
            (left, cy),
            (right, cy),
            (left, top),
            (right, top),
            (left, bottom),
            (right, bottom),
        ]);
    }
    pts
}

fn ring_line(ring: &[(f64, f64)]) -> Geom {
    let mut r = ring.to_vec();
    if r.first() != r.last() {
        if let Some(&p) = r.first() {
            r.push(p);
        }
    }
    Geom::Line(r)
}

fn sym_diff(a: &Geom, b: &Geom) -> Geom {
    if a.is_empty() {
        return b.clone();
    }
    sh::difference(&sh::union(a, b), &sh::intersection(a, b))
}

/// Closed outline contours and board material (`None` + reason when the
/// outline is unusable).
fn board_material(pcb: &Pcb) -> (OutlineSegs, Option<Geom>, String) {
    if !pcb.outline_error.is_empty() {
        return (
            Vec::new(),
            None,
            format!("board outline unavailable: {}", pcb.outline_error),
        );
    }
    let mut segments = pcb.get_board_outline_segments();
    let (ox, oy) = pcb.board_origin();
    let fail = |segs: Vec<_>, why: &str| (segs, None, format!("board outline unavailable: {why}"));
    for node in &pcb.sexp().children {
        if node.is_atom() {
            continue;
        }
        if is_footprint_tag(node.tag()) {
            let local_edge = node.iter_all().any(|e| {
                e.find("layer").and_then(|l| l.text_at(0)).as_deref() == Some("Edge.Cuts")
            });
            if local_edge {
                return fail(segments, "footprint-local Edge.Cuts");
            }
        }
        if node.find("layer").and_then(|l| l.text_at(0)).as_deref() != Some("Edge.Cuts") {
            continue;
        }
        let tag = node.tag().unwrap_or("");
        if !matches!(tag, "gr_line" | "gr_rect" | "gr_poly") {
            return fail(segments, &format!("unsupported {tag}"));
        }
        if tag != "gr_poly" {
            for ep in ["start", "end"] {
                let Some(p) = node.find(ep) else {
                    return fail(segments, "malformed endpoint");
                };
                if p.atoms().count() != 2
                    || [p.float_at(0), p.float_at(1)]
                        .iter()
                        .any(|c| !c.is_some_and(f64::is_finite))
                {
                    return fail(segments, "malformed endpoint");
                }
            }
        } else {
            let mut verts = Vec::new();
            if let Some(pts) = node.find("pts") {
                for xy in &pts.children {
                    if xy.is_atom() || !xy.has_tag("xy") {
                        return fail(segments, "malformed polygon");
                    }
                    match (xy.float_at(0), xy.float_at(1)) {
                        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => {
                            verts.push((x - ox, y - oy))
                        }
                        _ => return fail(segments, "malformed polygon"),
                    }
                }
            }
            if verts.len() > 1 && verts.last() == verts.first() {
                verts.pop();
            }
            if verts.len() < 3 {
                return fail(segments, "malformed polygon");
            }
            for i in 0..verts.len() {
                segments.push((verts[i], verts[(i + 1) % verts.len()]));
            }
        }
    }
    if segments.is_empty() {
        return fail(segments, "missing Edge.Cuts");
    }
    if segments
        .iter()
        .any(|(a, b)| ![a.0, a.1, b.0, b.1].iter().all(|v| v.is_finite()))
    {
        return fail(segments, "non-finite coordinates");
    }
    let mut contours: Vec<(Geom, Vec<(f64, f64)>)> = Vec::new();
    for idx in group_segments_by_connectivity(&segments, 0.01) {
        let ring = chain_segment_indices(&segments, &idx, 0.01);
        if ring.is_empty() {
            return fail(segments, "open or branched contour");
        }
        let poly = Poly::new(ring.clone());
        if !sh::is_valid(&poly) || poly.is_empty() || poly.area() <= 0.0 {
            return fail(segments, "invalid contour");
        }
        let boundary = ring_line(&ring);
        if contours
            .iter()
            .any(|(_, r)| sh::intersects(&boundary, &ring_line(r)))
        {
            return fail(segments, "intersecting contours");
        }
        contours.push((Geom::Poly(poly), ring));
    }
    let mut material = Geom::Empty;
    for (g, _) in &contours {
        material = sym_diff(&material, g);
    }
    (segments, Some(material), String::new())
}

/// Shapely `material.covers(geom)` for polygonal operands.
fn covers(material: &Geom, geom: &Geom) -> bool {
    if geom.is_empty() {
        return false;
    }
    let outside = sh::difference(geom, material).area();
    outside <= 1e-9 * geom.area().max(1.0)
}

struct Obstacles<'a> {
    apertures: &'a [(Geom, String)],
    silk: Vec<&'a (Geom, String)>,
    courtyards: &'a [(Geom, String)],
    outline: Option<&'a Geom>,
    material: Option<&'a Geom>,
    outline_error: &'a str,
    clearance: f64,
    edge_clearance: f64,
}

impl Obstacles<'_> {
    /// `_candidate_clear`.
    fn clears(&self, geom: &Geom) -> (bool, String) {
        if !self.outline_error.is_empty() {
            return (false, self.outline_error.to_string());
        }
        if !self.material.is_some_and(|m| covers(m, geom)) {
            return (false, "outside board material or inside a cutout".into());
        }
        let expanded = if self.clearance > 0.0 {
            sh::buffer_polygon(geom, self.clearance)
        } else {
            geom.clone()
        };
        for (g, label) in self.apertures {
            if sh::intersects(&expanded, g) {
                return (false, format!("pad aperture {label}"));
            }
        }
        for (g, label) in &self.silk {
            if sh::intersects(&expanded, g) {
                return (false, format!("silkscreen {label}"));
            }
        }
        for (g, label) in self.courtyards {
            if sh::intersects(geom, g) {
                return (false, format!("component body {label}"));
            }
        }
        if let Some(o) = self.outline {
            if sh::distance(geom, o) < self.edge_clearance - EPSILON_MM {
                return (false, "board edge".into());
            }
        }
        (true, String::new())
    }
}

/// The plan (or outcome) for one reference designator.
#[derive(Debug, Clone, PartialEq)]
pub struct RefPlacement {
    pub footprint_ref: String,
    pub layer: String,
    pub old_position: (f64, f64),
    pub new_position: (f64, f64),
    pub old_rotation: f64,
    pub new_rotation: f64,
    /// `unchanged`, `moved`, `unplaceable` or `under_component_fallback`.
    pub status: String,
    pub reason: String,
    /// Footprint-local coordinates to write (moved only).
    pub new_local_position: Option<(f64, f64)>,
}

impl RefPlacement {
    pub fn moved(&self) -> bool {
        self.status == "moved"
    }

    pub fn to_dict(&self) -> Json {
        let pt = |p: (f64, f64)| Json::Arr(vec![Json::Float(p.0), Json::Float(p.1)]);
        let mut d = Json::obj();
        d.set("footprint_ref", self.footprint_ref.as_str());
        d.set("layer", self.layer.as_str());
        d.set("old_position_mm", pt(self.old_position));
        d.set("new_position_mm", pt(self.new_position));
        d.set("old_rotation_deg", Json::Float(self.old_rotation));
        d.set("new_rotation_deg", Json::Float(self.new_rotation));
        d.set("status", self.status.as_str());
        d.set("reason", self.reason.as_str());
        d
    }
}

/// Aggregate plan result.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlaceSilkRefsResult {
    pub clearance_mm: f64,
    pub placements: Vec<RefPlacement>,
}

impl PlaceSilkRefsResult {
    fn with(&self, status: &str) -> Vec<&RefPlacement> {
        self.placements
            .iter()
            .filter(|p| p.status == status)
            .collect()
    }
    pub fn moved(&self) -> Vec<&RefPlacement> {
        self.with("moved")
    }
    pub fn unchanged(&self) -> Vec<&RefPlacement> {
        self.with("unchanged")
    }
    pub fn unplaceable(&self) -> Vec<&RefPlacement> {
        self.with("unplaceable")
    }
    pub fn under_component_fallback(&self) -> Vec<&RefPlacement> {
        self.with("under_component_fallback")
    }
    pub fn total_moved(&self) -> usize {
        self.moved().len()
    }
    pub fn total_unplaceable(&self) -> usize {
        self.unplaceable().len() + self.under_component_fallback().len()
    }
}

/// Planning options (upstream `plan()` keywords and defaults).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanOptions {
    pub clearance_mm: f64,
    pub edge_clearance_mm: f64,
    pub mask_clearance_mm: f64,
    pub max_offset_mm: f64,
    pub step_mm: f64,
    pub allow_rotate: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        PlanOptions {
            clearance_mm: DEFAULT_CLEARANCE_MM,
            edge_clearance_mm: SILK_EDGE_CLEARANCE_MM,
            mask_clearance_mm: 0.05,
            max_offset_mm: DEFAULT_MAX_OFFSET_MM,
            step_mm: DEFAULT_STEP_MM,
            allow_rotate: false,
        }
    }
}

#[derive(Default)]
struct RenderContext {
    apertures: [Labeled; 2],
    static_silk: [Labeled; 2],
    courtyards: [Labeled; 2],
    outline_segments: OutlineSegs,
}

/// Plans and applies collision-free reference placement.
pub struct SilkRefPlacer {
    pub path: PathBuf,
    pub pcb: Pcb,
    /// Reference -> footprint index (populated by `plan`).
    ref_nodes: HashMap<String, usize>,
    render: Option<RenderContext>,
}

impl SilkRefPlacer {
    pub fn new(pcb_path: impl AsRef<Path>) -> Result<Self> {
        let path = pcb_path.as_ref().to_path_buf();
        let pcb = Pcb::load(&path)?;
        Ok(SilkRefPlacer {
            path,
            pcb,
            ref_nodes: HashMap::new(),
            render: None,
        })
    }

    /// Compute the per-reference move plan without touching the tree.
    pub fn plan(&mut self, o: &PlanOptions) -> Result<PlaceSilkRefsResult> {
        for (name, v) in [
            ("clearance_mm", o.clearance_mm),
            ("edge_clearance_mm", o.edge_clearance_mm),
            ("mask_clearance_mm", o.mask_clearance_mm),
            ("max_offset_mm", o.max_offset_mm),
        ] {
            if !v.is_finite() || v < 0.0 {
                bail!("{name} must be finite and nonnegative");
            }
        }
        if !o.step_mm.is_finite() || o.step_mm <= 0.0 {
            bail!("step_mm must be finite and positive");
        }
        if o.max_offset_mm / o.step_mm >= 4096.0 {
            bail!("search exceeds 4096 rings; increase step_mm or reduce max_offset_mm");
        }
        let pcb = &self.pcb;
        let mut seen = std::collections::HashSet::new();
        for fp in pcb.footprints() {
            if !seen.insert(fp.reference.as_str()) {
                bail!(
                    "Duplicate footprint reference {}; assign unique references before placing silkscreen",
                    crate::pyjson::py_repr_str(&fp.reference)
                );
            }
        }
        let mut result = PlaceSilkRefsResult {
            clearance_mm: o.clearance_mm,
            placements: Vec::new(),
        };
        self.ref_nodes.clear();

        let mut ctx = RenderContext::default();
        for fp in pcb.footprints() {
            let t = fp_transform(fp);
            for pad in &fp.pads {
                let sides: &[&str] = match pad.pad_type.as_str() {
                    "smd" => {
                        if fp.layer == "F.Cu" {
                            &["F"]
                        } else {
                            &["B"]
                        }
                    }
                    "thru_hole" => &["F", "B"],
                    _ => continue,
                };
                let g = pad_aperture_geometry(pad, t(pad.position), o.mask_clearance_mm);
                for s in sides {
                    ctx.apertures[side_index(s)]
                        .push((g.clone(), format!("{} pad {}", fp.reference, pad.number)));
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
                ctx.apertures[side_index(side)]
                    .push((via_aperture_geometry(via, o.mask_clearance_mm), label));
            }
        }
        for (side, g, label) in iter_static_silk_obstacles(pcb) {
            ctx.static_silk[side_index(side)].push((g, label));
        }
        let mut own_courtyard: HashMap<(usize, usize), Geom> = HashMap::new();
        for (fi, fp) in pcb.footprints().iter().enumerate() {
            for side in ["F", "B"] {
                if !side_has_geometry(fp, side) {
                    continue;
                }
                if let Some(poly) = courtyard_geom(fp, side) {
                    ctx.courtyards[side_index(side)].push((poly.clone(), fp.reference.clone()));
                    own_courtyard.insert((fi, side_index(side)), poly);
                }
            }
        }
        let (segments, material, outline_error) = board_material(pcb);
        let outline = (!segments.is_empty() && outline_error.is_empty())
            .then(|| Geom::Lines(segments.iter().map(|(a, b)| vec![*a, *b]).collect()));
        ctx.outline_segments = segments;

        struct Entry {
            fi: usize,
            side: usize,
            original_center: (f64, f64),
            old_rotation: f64,
            geom0: Geom,
        }
        let mut entries: Vec<Entry> = Vec::new();
        let mut dynamic: [Vec<(Geom, String)>; 2] = [Vec::new(), Vec::new()];
        for (fi, fp) in pcb.footprints().iter().enumerate() {
            let Some(rt) = fp.texts.iter().find(|t| t.text_type == "reference") else {
                continue;
            };
            if rt.hidden {
                continue;
            }
            let Some(side) = silk_side(&rt.layer) else {
                continue;
            };
            let Some(node) = pcb
                .footprint_node_index(fi)
                .map(|ni| &pcb.sexp().children[ni])
            else {
                continue;
            };
            let Some(ri) = find_reference_child(node) else {
                continue;
            };
            let old_rotation = read_text_angle(&node.children[ri]);
            let center = fp_transform(fp)(rt.position);
            let Some(geom0) = oriented_text_geometry(
                &rt.text,
                rt.font_size,
                rt.font_thickness,
                center,
                old_rotation,
            ) else {
                continue;
            };
            self.ref_nodes.insert(fp.reference.clone(), fi);
            dynamic[side_index(side)].push((geom0.clone(), fp.reference.clone()));
            entries.push(Entry {
                fi,
                side: side_index(side),
                original_center: center,
                old_rotation,
                geom0,
            });
        }
        entries.sort_by(|a, b| {
            pcb.footprints()[a.fi]
                .reference
                .cmp(&pcb.footprints()[b.fi].reference)
        });

        for e in &entries {
            let fp = &pcb.footprints()[e.fi];
            let rt = fp
                .texts
                .iter()
                .find(|t| t.text_type == "reference")
                .expect("has ref");
            let r = fp.reference.clone();
            if let Some(i) = dynamic[e.side].iter().position(|(_, l)| *l == r) {
                dynamic[e.side].remove(i);
            }
            let mut silk: Vec<&(Geom, String)> = ctx.static_silk[e.side].iter().collect();
            silk.extend(dynamic[e.side].iter());
            let obs = Obstacles {
                apertures: &ctx.apertures[e.side],
                silk,
                courtyards: &ctx.courtyards[e.side],
                outline: outline.as_ref(),
                material: material.as_ref(),
                outline_error: &outline_error,
                clearance: o.clearance_mm,
                edge_clearance: o.edge_clearance_mm,
            };
            let side_name = if e.side == 0 { "F" } else { "B" };
            let bbox = component_bbox(fp, side_name);
            let mut cands = candidate_points(
                bbox,
                rt.font_size.0,
                rt.font_size.1,
                o.max_offset_mm,
                o.step_mm,
            );
            let oc = e.original_center;
            cands.sort_by(|a, b| {
                (a.0 - oc.0)
                    .hypot(a.1 - oc.1)
                    .total_cmp(&(b.0 - oc.0).hypot(b.1 - oc.1))
            });
            let mut chosen: Option<((f64, f64), f64, Geom)> = None;
            let mut last_reason = String::new();
            for p in std::iter::once(oc).chain(cands) {
                let geom = oriented_text_geometry(
                    &rt.text,
                    rt.font_size,
                    rt.font_thickness,
                    p,
                    e.old_rotation,
                )
                .expect("non-empty text");
                let (ok, why) = obs.clears(&geom);
                if ok {
                    chosen = Some((p, e.old_rotation, geom));
                    break;
                }
                last_reason = why;
                if o.allow_rotate {
                    let rot = e.old_rotation + 90.0;
                    let g =
                        oriented_text_geometry(&rt.text, rt.font_size, rt.font_thickness, p, rot)
                            .expect("non-empty text");
                    let (ok, why) = obs.clears(&g);
                    if ok {
                        chosen = Some((p, rot, g));
                        break;
                    }
                    last_reason = why;
                }
            }
            drop(obs);
            let Some((point, rotation, geom)) = chosen else {
                dynamic[e.side].push((e.geom0.clone(), r.clone()));
                let fallback = own_courtyard
                    .get(&(e.fi, e.side))
                    .is_some_and(|own| sh::intersects(&e.geom0, own));
                result.placements.push(RefPlacement {
                    footprint_ref: r,
                    layer: rt.layer.clone(),
                    old_position: oc,
                    new_position: oc,
                    old_rotation: e.old_rotation,
                    new_rotation: e.old_rotation,
                    status: if fallback {
                        "under_component_fallback"
                    } else {
                        "unplaceable"
                    }
                    .into(),
                    reason: if last_reason.is_empty() {
                        "no collision-free candidate within search radius".into()
                    } else {
                        last_reason
                    },
                    new_local_position: None,
                });
                continue;
            };
            dynamic[e.side].push((geom, r.clone()));
            let moved = (point.0 - oc.0).abs() > EPSILON_MM
                || (point.1 - oc.1).abs() > EPSILON_MM
                || rotation != e.old_rotation;
            result.placements.push(if moved {
                RefPlacement {
                    footprint_ref: r,
                    layer: rt.layer.clone(),
                    old_position: oc,
                    new_position: point,
                    old_rotation: e.old_rotation,
                    new_rotation: rotation,
                    status: "moved".into(),
                    reason: String::new(),
                    new_local_position: Some(inverse_fp_transform(fp)(point)),
                }
            } else {
                RefPlacement {
                    footprint_ref: r,
                    layer: rt.layer.clone(),
                    old_position: oc,
                    new_position: oc,
                    old_rotation: e.old_rotation,
                    new_rotation: e.old_rotation,
                    status: "unchanged".into(),
                    reason: String::new(),
                    new_local_position: None,
                }
            });
        }
        self.render = Some(ctx);
        Ok(result)
    }

    /// Write every moved placement's `(at ...)`; returns the count.
    pub fn apply(&mut self, result: &PlaceSilkRefsResult) -> Result<usize> {
        let mut applied = 0;
        for p in result.moved() {
            let (Some(&fi), Some((lx, ly))) =
                (self.ref_nodes.get(&p.footprint_ref), p.new_local_position)
            else {
                continue;
            };
            let Some(ni) = self.pcb.footprint_node_index(fi) else {
                continue;
            };
            let node = &mut self.pcb.sexp_mut().children[ni];
            let Some(ri) = find_reference_child(node) else {
                continue;
            };
            set_text_at(&mut node.children[ri], lx, ly, p.new_rotation)?;
            applied += 1;
        }
        Ok(applied)
    }

    /// Save the tree (to `output_path`, or back to the source).
    pub fn save(&self, output_path: Option<&Path>) -> Result<()> {
        crate::core::sexp_file::save_pcb(self.pcb.sexp(), output_path.unwrap_or(&self.path))
    }

    /// Write the SVG review artifact (requires [`Self::plan`]).
    pub fn render_svg(&self, result: &PlaceSilkRefsResult, output_path: &Path) -> Result<PathBuf> {
        let Some(ctx) = &self.render else {
            bail!("render_svg() requires plan() to be called first");
        };
        render_svg(&self.pcb, result, ctx, output_path)
    }
}

fn svg_escape(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn render_svg(
    pcb: &Pcb,
    result: &PlaceSilkRefsResult,
    ctx: &RenderContext,
    out: &Path,
) -> Result<PathBuf> {
    let margin = 3.0;
    let (mut xs, mut ys): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());
    let mut add = |x0: f64, y0: f64, x1: f64, y1: f64| {
        xs.extend([x0, x1]);
        ys.extend([y0, y1]);
    };
    for (a, b) in &ctx.outline_segments {
        add(a.0, a.1, b.0, b.1);
    }
    for side in &ctx.courtyards {
        for (g, _) in side {
            if let Some(b) = g.bounds() {
                add(b.0, b.1, b.2, b.3);
            }
        }
    }
    for side in &ctx.apertures {
        for (g, _) in side {
            if let Some(b) = g.bounds() {
                add(b.0, b.1, b.2, b.3);
            }
        }
    }
    for p in &result.placements {
        for (x, y) in [p.old_position, p.new_position] {
            add(x - 2.0, y - 2.0, x + 2.0, y + 2.0);
        }
    }
    if xs.is_empty() || ys.is_empty() {
        xs = vec![0.0, 10.0];
        ys = vec![0.0, 10.0];
    }
    let fmin = |v: &[f64]| v.iter().cloned().fold(f64::INFINITY, f64::min);
    let fmax = |v: &[f64]| v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let (min_x, max_x) = (fmin(&xs) - margin, fmax(&xs) + margin);
    let (min_y, max_y) = (fmin(&ys) - margin, fmax(&ys) + margin);
    let width = (max_x - min_x).max(1.0);
    let height = (max_y - min_y).max(1.0);
    let s = 10.0;
    let mut parts = vec![
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{:.2} {:.2} {:.2} {:.2}\" width=\"{:.0}\" height=\"{:.0}\">",
            min_x * s,
            min_y * s,
            width * s,
            height * s,
            width * s,
            height * s
        ),
        format!(
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"white\"/>",
            min_x * s,
            min_y * s,
            width * s,
            height * s
        ),
        "<!-- kct place-silk-refs review artifact (issue #5030) -->".to_string(),
        "<!-- legend: green=unchanged, blue(+dashed old)=moved, red=unplaceable/under_component_fallback -->".to_string(),
    ];
    let pts = |coords: &[(f64, f64)]| {
        coords
            .iter()
            .map(|(x, y)| format!("{:.2},{:.2}", x * s, y * s))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let exterior = |g: &Geom| -> Option<Vec<(f64, f64)>> {
        match g {
            // Shapely: `.exterior` (polygons) or `.coords` (lines); multi
            // geometries have neither and are skipped upstream.
            Geom::Poly(p) => Some(p.shell.clone()),
            Geom::Line(l) => Some(l.clone()),
            _ => None,
        }
    };
    for (a, b) in &ctx.outline_segments {
        parts.push(format!(
            "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" stroke=\"black\" stroke-width=\"1\"/>",
            a.0 * s,
            a.1 * s,
            b.0 * s,
            b.1 * s
        ));
    }
    for side in &ctx.courtyards {
        for (g, label) in side {
            if let Some(c) = exterior(g) {
                parts.push(format!(
                    "<polygon points=\"{}\" fill=\"none\" stroke=\"#999999\" stroke-width=\"0.5\" stroke-dasharray=\"2,2\"><title>{} courtyard</title></polygon>",
                    pts(&c),
                    svg_escape(label)
                ));
            }
        }
    }
    for side in &ctx.apertures {
        for (g, label) in side {
            if let Some(b) = g.bounds() {
                parts.push(format!(
                    "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"#ffcc66\" fill-opacity=\"0.5\"><title>{}</title></rect>",
                    b.0 * s,
                    b.1 * s,
                    (b.2 - b.0) * s,
                    (b.3 - b.1) * s,
                    svg_escape(label)
                ));
            }
        }
    }
    for side in &ctx.static_silk {
        for (g, label) in side {
            for p in g.polys() {
                parts.push(format!(
                    "<polygon points=\"{}\" fill=\"#666666\" fill-opacity=\"0.4\"><title>{}</title></polygon>",
                    pts(&p.shell),
                    svg_escape(label)
                ));
            }
        }
    }
    let mut placements: Vec<&RefPlacement> = result.placements.iter().collect();
    placements.sort_by(|a, b| a.footprint_ref.cmp(&b.footprint_ref));
    for p in placements {
        let rt = pcb
            .footprints()
            .iter()
            .rev()
            .find(|f| f.reference == p.footprint_ref)
            .and_then(|f| f.texts.iter().find(|t| t.text_type == "reference"));
        let font = rt.map_or((1.0, 1.0), |t| t.font_size);
        let thick = rt.map_or(0.15, |t| t.font_thickness);
        let text = rt.map_or(p.footprint_ref.as_str(), |t| t.text.as_str());
        let color = match p.status.as_str() {
            "moved" => {
                if let Some(g) =
                    oriented_text_geometry(text, font, thick, p.old_position, p.old_rotation)
                {
                    if let Some(c) = exterior(&g) {
                        parts.push(format!(
                            "<polygon points=\"{}\" fill=\"none\" stroke=\"#999999\" stroke-width=\"0.5\" stroke-dasharray=\"1,1\"/>",
                            pts(&c)
                        ));
                    }
                }
                parts.push(format!(
                    "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" stroke=\"#3366cc\" stroke-width=\"0.3\" stroke-dasharray=\"1,1\"/>",
                    p.old_position.0 * s,
                    p.old_position.1 * s,
                    p.new_position.0 * s,
                    p.new_position.1 * s
                ));
                "#3366cc"
            }
            "unchanged" => "#2e8b57",
            _ => "#cc3333",
        };
        if let Some(g) = oriented_text_geometry(text, font, thick, p.new_position, p.new_rotation) {
            if let Some(c) = exterior(&g) {
                let reason = if p.reason.is_empty() {
                    String::new()
                } else {
                    format!(" -- {}", svg_escape(&p.reason))
                };
                parts.push(format!(
                    "<polygon points=\"{}\" fill=\"{color}\" fill-opacity=\"0.35\" stroke=\"{color}\" stroke-width=\"0.4\"><title>{}: {}{reason}</title></polygon>",
                    pts(&c),
                    svg_escape(&p.footprint_ref),
                    svg_escape(&p.status)
                ));
            }
        }
        let (nx, ny) = p.new_position;
        parts.push(format!(
            "<text x=\"{:.2}\" y=\"{:.2}\" font-size=\"{:.1}\" transform=\"rotate({} {:.2} {:.2})\" fill=\"{color}\" text-anchor=\"middle\" dominant-baseline=\"middle\">{}</text>",
            nx * s,
            ny * s,
            (font.1 * s * 0.6).max(4.0),
            format_g(-p.new_rotation, 6),
            nx * s,
            ny * s,
            svg_escape(&p.footprint_ref)
        ));
    }
    parts.push("</svg>".into());
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out, parts.join("\n"))?;
    Ok(out.to_path_buf())
}
