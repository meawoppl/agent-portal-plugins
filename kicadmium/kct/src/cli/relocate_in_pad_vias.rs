//! Port of `kicad_tools.cli.relocate_in_pad_vias`: the shared via-placement
//! clearance engine (`_check_clearance` and helpers) used by fix-drc's drill
//! repair gate and by `fix-vias --relocate-in-pad`.
//!
//! Coordinates are board-relative (the `Pcb` schema frame).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Result};

use crate::core::geometry::rotate_pad_offset;
use crate::geometry::shapely::{self as sh, Geom};
use crate::schema::pcb::{Footprint, Pad, Pcb, Segment, Via};
use crate::sexp::SExp;
use crate::validate::connectivity::fill_solid_region;
use crate::validate::rules::via_in_pad::pad_absolute_bbox;

pub const COINCIDENT_TOL: f64 = 1e-3;

pub type Bbox = (f64, f64, f64, f64);

/// SMD pads grouped by net: `(footprint index, pad index, bbox)`.
pub type PadsByNet = BTreeMap<i64, Vec<(usize, usize, Bbox)>>;

/// Plated/non-plated through-hole pads: `(fp, pad, bbox, hole center)`.
pub type ThtPads = Vec<(usize, usize, Bbox, (f64, f64))>;

pub fn is_smd_pad(pad: &Pad) -> bool {
    pad.pad_type == "smd"
}

pub fn dist_point_to_aabb(px: f64, py: f64, b: Bbox) -> f64 {
    let ddx = (b.0 - px).max(0.0).max(px - b.2);
    let ddy = (b.1 - py).max(0.0).max(py - b.3);
    ddx.hypot(ddy)
}

pub fn dist_point_to_segment(px: f64, py: f64, seg: &Segment) -> f64 {
    let (x1, y1) = seg.start;
    let (x2, y2) = seg.end;
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        return (px - x1).hypot(py - y1);
    }
    let t = (((px - x1) * dx + (py - y1) * dy) / len_sq).clamp(0.0, 1.0);
    (px - (x1 + t * dx)).hypot(py - (y1 + t * dy))
}

/// Ray from an interior point to the AABB boundary.
pub fn ray_aabb_exit_distance(px: f64, py: f64, dx: f64, dy: f64, b: Bbox) -> f64 {
    let mut ts = Vec::new();
    if dx > 1e-12 {
        ts.push((b.2 - px) / dx);
    } else if dx < -1e-12 {
        ts.push((b.0 - px) / dx);
    }
    if dy > 1e-12 {
        ts.push((b.3 - py) / dy);
    } else if dy < -1e-12 {
        ts.push((b.1 - py) / dy);
    }
    ts.into_iter().filter(|t| *t > 0.0).fold(None, |m: Option<f64>, t| Some(m.map_or(t, |m| m.min(t)))).unwrap_or(0.0)
}

pub fn collect_smd_pads_by_net(pcb: &Pcb) -> PadsByNet {
    let mut out = PadsByNet::new();
    for (fi, fp) in pcb.footprints().iter().enumerate() {
        for (pi, pad) in fp.pads.iter().enumerate() {
            if !is_smd_pad(pad) {
                continue;
            }
            out.entry(pad.net_number)
                .or_default()
                .push((fi, pi, pad_absolute_bbox(pad, fp)));
        }
    }
    out
}

pub fn collect_tht_pads(pcb: &Pcb) -> ThtPads {
    let mut out = Vec::new();
    for (fi, fp) in pcb.footprints().iter().enumerate() {
        for (pi, pad) in fp.pads.iter().enumerate() {
            if pad.pad_type != "thru_hole" && pad.pad_type != "np_thru_hole" {
                continue;
            }
            if pad.drill <= 0.0 {
                continue;
            }
            let b = pad_absolute_bbox(pad, fp);
            out.push((fi, pi, b, ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0)));
        }
    }
    out
}

/// Project drill-to-copper floor (`board.design_settings.rules.min_hole_clearance`,
/// default 0.25 mm), raised to `explicit` when given.
pub fn resolve_hole_clearance(pcb: &Pcb, explicit: Option<f64>) -> Result<f64> {
    let mut value = 0.25;
    if let Some(path) = pcb.path() {
        let project = path.with_extension("kicad_pro");
        if project.exists() {
            let data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&project)?)?;
            let mut cur = &data;
            let mut location = "project".to_string();
            let empty = serde_json::Value::Object(Default::default());
            for key in ["board", "design_settings", "rules"] {
                if !cur.is_object() {
                    bail!("{location} must be an object");
                }
                cur = cur.get(key).unwrap_or(&empty);
                location.push('.');
                location.push_str(key);
            }
            if !cur.is_object() {
                bail!("{location} must be an object");
            }
            match cur.get("min_hole_clearance") {
                None => {}
                Some(v) if v.is_number() => value = v.as_f64().unwrap_or(f64::NAN),
                Some(_) => bail!("min_hole_clearance must be finite and nonnegative"),
            }
        }
    }
    if !value.is_finite() || value < 0.0 {
        bail!("min_hole_clearance must be finite and nonnegative");
    }
    if let Some(e) = explicit {
        if !e.is_finite() || e < 0.0 {
            bail!("min_hole_clearance must be finite and nonnegative");
        }
        value = value.max(e);
    }
    Ok(value)
}

fn has_unmodeled_drill(pad: &Pad) -> bool {
    pad.drill_size.is_some() || pad.drill_offset != (0.0, 0.0)
}

fn point_of(node: Option<&SExp>, transform: &dyn Fn(f64, f64) -> Option<(f64, f64)>) -> Option<(f64, f64)> {
    let n = node?;
    let (x, y) = (n.float_at(0)?, n.float_at(1)?);
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    transform(x, y)
}

/// Conservative stroke-inclusive bounds of a raw graphic item.
fn raw_copper_bounds(item: &SExp, transform: &dyn Fn(f64, f64) -> Option<(f64, f64)>) -> Option<Bbox> {
    let width = match item.get("width") {
        Some(w) => w.float_at(0),
        None => item.get("stroke").and_then(|s| s.get("width")).and_then(|w| w.float_at(0)),
    }?;
    if !width.is_finite() || width < 0.0 {
        return None;
    }
    let name = item.tag().unwrap_or("");
    let kind = name.strip_prefix("gr_").or_else(|| name.strip_prefix("fp_")).unwrap_or(name);
    let points: Vec<(f64, f64)> = match kind {
        "line" | "rect" => {
            let (a, b) = (item.get("start"), item.get("end"));
            let mut pts = vec![point_of(a, transform)?, point_of(b, transform)?];
            if kind == "rect" {
                let (a, b) = (a?, b?);
                pts.push(transform(a.float_at(0)?, b.float_at(1)?)?);
                pts.push(transform(b.float_at(0)?, a.float_at(1)?)?);
            }
            pts
        }
        "arc" => {
            let a = point_of(item.get("start"), transform)?;
            let b = point_of(item.get("mid"), transform)?;
            let c = point_of(item.get("end"), transform)?;
            let (bx, by, cx, cy) = (b.0 - a.0, b.1 - a.1, c.0 - a.0, c.1 - a.1);
            let det = 2.0 * (bx * cy - by * cx);
            if det.abs() < 1e-10 {
                return None;
            }
            let (b2, c2) = (bx * bx + by * by, cx * cx + cy * cy);
            let ux = (cy * b2 - by * c2) / det;
            let uy = (bx * c2 - cx * b2) / det;
            let r = ux.hypot(uy);
            let ctr = (a.0 + ux, a.1 + uy);
            vec![(ctr.0 - r, ctr.1 - r), (ctr.0 + r, ctr.1 + r)]
        }
        "circle" => {
            let ctr = point_of(item.get("center"), transform)?;
            let e = point_of(item.get("end"), transform)?;
            let r = (ctr.0 - e.0).hypot(ctr.1 - e.1);
            vec![(ctr.0 - r, ctr.1 - r), (ctr.0 + r, ctr.1 + r)]
        }
        "poly" | "curve" => {
            let pts = item.get("pts")?;
            if pts.children.iter().any(|c| !c.has_tag("xy")) {
                return None;
            }
            let v: Option<Vec<_>> = pts.children.iter().map(|c| point_of(Some(c), transform)).collect();
            let v = v?;
            if v.len() < 3 || (kind == "curve" && v.len() != 4) {
                return None;
            }
            v
        }
        _ => return None,
    };
    if points.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
        return None;
    }
    let m = width / 2.0 + 1e-7;
    Some((
        points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min) - m,
        points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min) - m,
        points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) + m,
        points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max) + m,
    ))
}

fn check_raw_copper(
    pcb: &Pcb,
    via: &Via,
    dist: &dyn Fn(Bbox) -> f64,
    radius: f64,
    layers: &[String],
    clearance: f64,
    label: &str,
) -> Option<String> {
    let (ox, oy) = pcb.board_origin();
    for node in &pcb.sexp().children {
        if node.is_atom() {
            continue;
        }
        let footprint = matches!(node.tag(), Some("footprint" | "module"));
        let at = node.get("at");
        let transform = |x: f64, y: f64| -> Option<(f64, f64)> {
            if footprint {
                let at = at?;
                let (fx, fy) = (at.float_at(0)?, at.float_at(1)?);
                let (dx, dy) = rotate_pad_offset(x, y, at.float_at(2).unwrap_or(0.0));
                Some((fx + dx - ox, fy + dy - oy))
            } else {
                Some((x - ox, y - oy))
            }
        };
        let items: Vec<&SExp> = if footprint { node.children.iter().collect() } else { vec![node] };
        for item in items {
            let name = item.tag().unwrap_or("");
            if item.is_atom()
                || !(matches!(name, "arc" | "property" | "dimension")
                    || name.starts_with("gr_")
                    || name.starts_with("fp_"))
            {
                continue;
            }
            let layer = item.get("layer").and_then(|l| l.text_at(0)).unwrap_or_default();
            if !layers.contains(&layer) {
                continue;
            }
            if name == "arc" && via.net_number != 0 {
                if let Some(net) = item.get("net") {
                    match net.value_at(0) {
                        Some(crate::sexp::Value::Int(i)) if *i == via.net_number => continue,
                        Some(crate::sexp::Value::Str(s)) if !via.net_name.is_empty() && *s == via.net_name => continue,
                        _ => {}
                    }
                }
            }
            let Some(b) = raw_copper_bounds(item, &transform) else {
                return Some(format!("{label} unproven for {name} on {layer}"));
            };
            if dist(b) - radius < clearance - 1e-6 {
                return Some(format!("{label} to {name} on {layer}"));
            }
        }
    }
    None
}

fn copper_names(pcb: &Pcb) -> Vec<String> {
    pcb.copper_layers().iter().map(|l| l.name.clone()).collect()
}

fn span(copper: &[String], endpoints: &[String]) -> Vec<String> {
    if endpoints.len() >= 2 && endpoints.iter().all(|l| copper.contains(l)) {
        let idx: Vec<usize> = endpoints.iter().filter_map(|l| copper.iter().position(|c| c == l)).collect();
        let lo = *idx.iter().min().expect("non-empty");
        let hi = *idx.iter().max().expect("non-empty");
        return copper[lo..=hi].to_vec();
    }
    endpoints.to_vec()
}

/// `_check_hole_to_copper`: drill-to-copper both ways, then raw graphics.
pub fn check_hole_to_copper(
    pcb: &Pcb,
    via_index: Option<usize>,
    via: &Via,
    x: f64,
    y: f64,
    floor: f64,
    min_copper_clearance: Option<f64>,
) -> Option<String> {
    let copper = copper_names(pcb);
    let layers = span(&copper, &via.layers);
    let has = |l: &str| layers.iter().any(|x| x == l);
    let overlaps = |item: &[String]| {
        item.iter().any(|l| has(l))
            || item.iter().any(|l| l == "*.Cu")
            || (item.iter().any(|l| l == "F&B.Cu") && (has("F.Cu") || has("B.Cu")))
    };
    let foreign = |net: i64| net == 0 || net != via.net_number;
    let hole_r = via.drill / 2.0;
    for seg in pcb.segments() {
        if has(&seg.layer) && foreign(seg.net_number) {
            let gap = dist_point_to_segment(x, y, seg) - hole_r - seg.width / 2.0;
            if gap < floor - 1e-6 {
                return Some(format!("hole-to-copper {gap:.3}mm to track on {}", seg.layer));
            }
        }
    }
    for (i, other) in pcb.vias().iter().enumerate() {
        if Some(i) == via_index || !foreign(other.net_number) {
            continue;
        }
        let os = span(&copper, &other.layers);
        if !os.iter().any(|l| has(l)) {
            continue;
        }
        let d = (x - other.position.0).hypot(y - other.position.1);
        let gap = (d - hole_r - other.size / 2.0).min(d - via.size / 2.0 - other.drill / 2.0);
        if gap < floor - 1e-6 {
            return Some(format!("hole-to-copper {gap:.3}mm to via on net {}", other.net_number));
        }
    }
    for fp in pcb.footprints() {
        for pad in &fp.pads {
            if !foreign(pad.net_number) || !overlaps(&pad.layers) {
                continue;
            }
            if has_unmodeled_drill(pad) {
                return Some(format!("hole-to-copper unmodeled drill at pad {}-{}", fp.reference, pad.number));
            }
            if pad.shape == "custom" {
                return Some(format!("hole-to-copper unproven for custom pad {}-{}", fp.reference, pad.number));
            }
            let b = pad_absolute_bbox(pad, fp);
            let mut gap = if pad.pad_type == "np_thru_hole" {
                f64::INFINITY
            } else {
                dist_point_to_aabb(x, y, b) - hole_r
            };
            if pad.drill > 0.0 {
                let c = ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
                gap = gap.min((x - c.0).hypot(y - c.1) - via.size / 2.0 - pad.drill / 2.0);
            }
            if gap < floor - 1e-6 {
                return Some(format!("hole-to-copper {gap:.3}mm to pad {}-{}", fp.reference, pad.number));
            }
        }
    }
    let pt = Geom::Point((x, y));
    for zone in pcb.zones() {
        if !foreign(zone.net_number) {
            continue;
        }
        for (idx, points) in zone.filled_polygons.iter().enumerate() {
            if !has(zone.filled_polygon_layer(idx)) {
                continue;
            }
            if let Some(solid) = fill_solid_region(points) {
                if sh::distance(&pt, &solid) - hole_r < floor - 1e-6 {
                    return Some("hole-to-copper to filled zone".into());
                }
            }
        }
    }
    let pdist = |b: Bbox| dist_point_to_aabb(x, y, b);
    let reason = check_raw_copper(pcb, via, &pdist, hole_r, &layers, floor, "hole-to-copper");
    if reason.is_some() {
        return reason;
    }
    let mc = min_copper_clearance?;
    check_raw_copper(pcb, via, &pdist, via.size / 2.0, &layers, mc, "clearance")
}

/// `_check_clearance`: reason string if placing via `via_index` at
/// `(new_x, new_y)` violates copper clearance / hole spacing.
#[allow(clippy::too_many_arguments)]
pub fn check_clearance(
    pcb: &Pcb,
    via_index: Option<usize>,
    via: &Via,
    new_x: f64,
    new_y: f64,
    pads_by_net: &PadsByNet,
    tht_pads: &ThtPads,
    min_clearance: f64,
    min_hole_to_hole: f64,
    min_hole_clearance: Option<f64>,
) -> Result<Option<String>> {
    let via_r = via.size / 2.0;
    let hole_r = via.drill / 2.0;
    for (i, other) in pcb.vias().iter().enumerate() {
        if Some(i) == via_index {
            continue;
        }
        let d = (other.position.0 - new_x).hypot(other.position.1 - new_y);
        let hole_gap = d - hole_r - other.drill / 2.0;
        if hole_gap < min_hole_to_hole - 1e-6 {
            return Ok(Some(format!(
                "hole-to-hole {hole_gap:.3}mm to via at ({:.2}, {:.2})",
                other.position.0, other.position.1
            )));
        }
        if other.net_number != via.net_number {
            let cu = d - via_r - other.size / 2.0;
            if cu < min_clearance - 1e-6 {
                return Ok(Some(format!(
                    "clearance {cu:.3}mm to via at ({:.2}, {:.2})",
                    other.position.0, other.position.1
                )));
            }
        }
    }
    let fps = pcb.footprints();
    for (net, entries) in pads_by_net {
        if *net != 0 && *net == via.net_number {
            continue;
        }
        for &(fi, pi, b) in entries {
            let cu = dist_point_to_aabb(new_x, new_y, b) - via_r;
            if cu < min_clearance - 1e-6 {
                return Ok(Some(format!(
                    "clearance {cu:.3}mm to pad {}-{}",
                    fps[fi].reference, fps[fi].pads[pi].number
                )));
            }
        }
    }
    for &(fi, pi, b, hc) in tht_pads {
        let pad = &fps[fi].pads[pi];
        let d = (hc.0 - new_x).hypot(hc.1 - new_y);
        let hole_gap = d - hole_r - pad.drill / 2.0;
        if hole_gap < min_hole_to_hole - 1e-6 {
            return Ok(Some(format!(
                "hole-to-hole {hole_gap:.3}mm to THT pad {}-{}",
                fps[fi].reference, pad.number
            )));
        }
        if pad.net_number != 0 && pad.net_number != via.net_number {
            let cu = dist_point_to_aabb(new_x, new_y, b) - via_r;
            if cu < min_clearance - 1e-6 {
                return Ok(Some(format!(
                    "clearance {cu:.3}mm to THT pad {}-{}",
                    fps[fi].reference, pad.number
                )));
            }
        }
    }
    for seg in pcb.segments() {
        if seg.net_number == via.net_number {
            continue;
        }
        let cu = dist_point_to_segment(new_x, new_y, seg) - via_r - seg.width / 2.0;
        if cu < min_clearance - 1e-6 {
            return Ok(Some(format!("clearance {cu:.3}mm to track on net {}", seg.net_number)));
        }
    }
    let floor = resolve_hole_clearance(pcb, min_hole_clearance)?;
    Ok(check_hole_to_copper(pcb, via_index, via, new_x, new_y, floor, Some(min_clearance)))
}

/// Footprint by index (helper for callers holding pad indices).
pub fn footprint_at(pcb: &Pcb, index: usize) -> &Footprint {
    &pcb.footprints()[index]
}


// ------------------------------------------------------------------ relocation

/// A via moved off-pad.
#[derive(Debug, Clone, PartialEq)]
pub struct ViaRelocation {
    pub old_x: f64,
    pub old_y: f64,
    pub new_x: f64,
    pub new_y: f64,
    pub net: i64,
    pub net_name: String,
    pub pad_ref: String,
    pub uuid: String,
    pub stub_layers: Vec<String>,
    /// `signal` or `plane-stitch`.
    pub kind: String,
}

/// A via that could not be relocated (`skipped` or `unresolvable`).
#[derive(Debug, Clone, PartialEq)]
pub struct ViaRelocationSkip {
    pub x: f64,
    pub y: f64,
    pub net: i64,
    pub net_name: String,
    pub pad_ref: String,
    pub reason: String,
    pub uuid: String,
    pub category: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RelocationResult {
    pub moved: Vec<ViaRelocation>,
    pub skipped: Vec<ViaRelocationSkip>,
    pub unresolvable: Vec<ViaRelocationSkip>,
    pub supported_noop: bool,
}

impl RelocationResult {
    pub fn changed(&self) -> bool {
        !self.moved.is_empty()
    }
}

const PLANE_DIRECTIONS: [(f64, f64); 8] = [
    (1.0, 0.0),
    (0.0, 1.0),
    (-1.0, 0.0),
    (0.0, -1.0),
    (0.707, 0.707),
    (-0.707, 0.707),
    (-0.707, -0.707),
    (0.707, -0.707),
];

fn endpoint_at(seg: &Segment, x: f64, y: f64) -> Option<(f64, f64)> {
    if (seg.start.0 - x).abs() < COINCIDENT_TOL && (seg.start.1 - y).abs() < COINCIDENT_TOL {
        return Some(seg.end);
    }
    if (seg.end.0 - x).abs() < COINCIDENT_TOL && (seg.end.1 - y).abs() < COINCIDENT_TOL {
        return Some(seg.start);
    }
    None
}

fn pad_copper_layer(pad: &Pad) -> String {
    pad.layers
        .iter()
        .find(|l| l.ends_with(".Cu") && !l.starts_with('*'))
        .cloned()
        .unwrap_or_else(|| "F.Cu".into())
}

fn via_spans_zone_layer(via: &Via, zone: &crate::schema::pcb::Zone) -> bool {
    let has = |l: &str| via.layers.iter().any(|x| x == l);
    has(&zone.layer) || (has("F.Cu") && has("B.Cu") && zone.layer.ends_with(".Cu"))
}

fn same_net_zone_boundaries(pcb: &Pcb, via: &Via, net_name: &str) -> Vec<Vec<(f64, f64)>> {
    pcb.zones()
        .iter()
        .filter(|z| {
            ((via.net_number != 0 && z.net_number == via.net_number)
                || (!net_name.is_empty() && z.net_name == net_name))
                && via_spans_zone_layer(via, z)
                && !z.polygon.is_empty()
        })
        .map(|z| z.polygon.clone())
        .collect()
}

fn plane_stub_layers(pcb: &Pcb, via: &Via, pad: &Pad, target: (f64, f64), net_name: &str) -> Vec<String> {
    use crate::validate::rules::placement::point_in_polygon;
    let old_land = sh::point_buffer(via.position, via.size / 2.0);
    let new_land = sh::point_buffer(target, via.size / 2.0);
    let mut layers = vec![pad_copper_layer(pad)];
    let mut connected: Vec<String> = Vec::new();
    for zone in pcb.zones() {
        if zone.keepout.is_some() || !via_spans_zone_layer(via, zone) {
            continue;
        }
        if !((via.net_number != 0 && zone.net_number == via.net_number)
            || (!net_name.is_empty() && zone.net_name == net_name))
        {
            continue;
        }
        if zone.filled_polygons.is_empty()
            && point_in_polygon(via.position.0, via.position.1, &zone.polygon)
        {
            layers.push(zone.layer.clone());
        }
        for (i, pts) in zone.filled_polygons.iter().enumerate() {
            let layer = zone.filled_polygon_layer(i).to_string();
            let Some(solid) = fill_solid_region(pts) else {
                continue;
            };
            for comp in solid.polys() {
                let g = Geom::Poly(comp.clone());
                if sh::intersects(&g, &old_land) {
                    layers.push(layer.clone());
                }
                let covers = |land: &Geom| sh::difference(land, &g).area() <= 1e-12;
                if covers(&old_land) && covers(&new_land) {
                    connected.push(layer.clone());
                }
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    for l in layers {
        if !l.is_empty() && !connected.contains(&l) && !out.contains(&l) {
            out.push(l);
        }
    }
    out
}

/// `_check_stub_clearance`: swept copper of straight stubs on `layers`.
#[allow(clippy::too_many_arguments)]
pub fn check_stub_clearance(
    pcb: &Pcb,
    via_index: usize,
    via: &Via,
    target: (f64, f64),
    stub_layers: &[String],
    stub_width: f64,
    min_clearance: f64,
    min_hole_clearance: Option<f64>,
) -> Result<Option<String>> {
    let hole_floor = resolve_hole_clearance(pcb, min_hole_clearance)?;
    let path = Geom::Line(vec![via.position, target]);
    let radius = stub_width / 2.0;
    let copper = copper_names(pcb);
    let has = |l: &str| stub_layers.iter().any(|x| x == l);
    let overlaps = |item: &[String]| {
        item.iter().any(|l| has(l))
            || (item.iter().any(|l| l == "*.Cu") && !stub_layers.is_empty())
            || (item.iter().any(|l| l == "F&B.Cu") && (has("F.Cu") || has("B.Cu")))
    };
    let foreign = |net: i64| net == 0 || net != via.net_number;
    let quad = 64usize;
    let circ = 1.0 / (std::f64::consts::PI / (4.0 * quad as f64)).cos();
    let keepouts: std::cell::RefCell<Vec<(f64, Geom)>> = std::cell::RefCell::new(Vec::new());
    let too_close = |shape: &Geom, other_r: f64, clearance: f64| -> bool {
        if sh::distance(&path, shape) - radius - other_r >= clearance - 1e-6 {
            return false;
        }
        let mut ks = keepouts.borrow_mut();
        let idx = match ks.iter().position(|(c, _)| *c == clearance) {
            Some(i) => i,
            None => {
                let cand =
                    sh::segment_buffer_q(via.position, target, (radius + clearance) * circ, quad);
                let old = sh::point_buffer_q(via.position, via.size / 2.0 + clearance, quad);
                ks.push((clearance, sh::difference(&cand, &old)));
                ks.len() - 1
            }
        };
        let copper_shape = if other_r > 0.0 {
            match shape {
                Geom::Point(p) => sh::point_buffer_q(*p, other_r * circ, quad),
                Geom::Line(v) if v.len() == 2 => {
                    sh::segment_buffer_q(v[0], v[1], other_r * circ, quad)
                }
                other => sh::buffer_polygon_q(other, other_r * circ, quad),
            }
        } else {
            shape.clone()
        };
        sh::intersects(&ks[idx].1, &copper_shape)
    };
    for seg in pcb.segments() {
        if has(&seg.layer)
            && foreign(seg.net_number)
            && too_close(&Geom::Line(vec![seg.start, seg.end]), seg.width / 2.0, min_clearance)
        {
            return Ok(Some(format!(
                "stub clearance to track on {}, net {}",
                seg.layer, seg.net_number
            )));
        }
    }
    for (i, other) in pcb.vias().iter().enumerate() {
        if i == via_index || !foreign(other.net_number) {
            continue;
        }
        let sp = span(&copper, &other.layers);
        if overlaps(&sp) && too_close(&Geom::Point(other.position), other.drill / 2.0, hole_floor) {
            return Ok(Some(format!(
                "stub hole-to-copper to via on net {}",
                other.net_number
            )));
        }
        if overlaps(&sp) && too_close(&Geom::Point(other.position), other.size / 2.0, min_clearance)
        {
            return Ok(Some(format!("stub clearance to via on net {}", other.net_number)));
        }
    }
    for fp in pcb.footprints() {
        for pad in &fp.pads {
            if !foreign(pad.net_number) || !overlaps(&pad.layers) {
                continue;
            }
            if has_unmodeled_drill(pad) {
                return Ok(Some(format!(
                    "hole-to-copper unmodeled drill at pad {}-{}",
                    fp.reference, pad.number
                )));
            }
            if pad.shape == "custom" {
                return Ok(Some(format!(
                    "stub clearance unproven for custom pad {}-{}",
                    fp.reference, pad.number
                )));
            }
            let b = pad_absolute_bbox(pad, fp);
            if pad.drill > 0.0 {
                let c = ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
                if too_close(&Geom::Point(c), pad.drill / 2.0, hole_floor) {
                    return Ok(Some(format!(
                        "stub hole-to-copper to pad {}-{}",
                        fp.reference, pad.number
                    )));
                }
            }
            if too_close(&sh::box_poly(b.0, b.1, b.2, b.3), 0.0, min_clearance) {
                return Ok(Some(format!(
                    "stub clearance to pad {}-{}",
                    fp.reference, pad.number
                )));
            }
        }
    }
    for zone in pcb.zones() {
        if !foreign(zone.net_number) {
            continue;
        }
        for (i, pts) in zone.filled_polygons.iter().enumerate() {
            if !has(zone.filled_polygon_layer(i)) {
                continue;
            }
            if let Some(solid) = fill_solid_region(pts) {
                if too_close(&solid, 0.0, min_clearance) {
                    return Ok(Some(format!(
                        "stub clearance to filled zone on {}",
                        zone.filled_polygon_layer(i)
                    )));
                }
            }
        }
    }
    let pdist = |b: Bbox| sh::distance(&path, &sh::box_poly(b.0, b.1, b.2, b.3));
    Ok(check_raw_copper(
        pcb,
        via,
        &pdist,
        radius,
        stub_layers,
        min_clearance,
        "stub clearance",
    ))
}

/// Straight Edge.Cuts outline for `--search-alternatives`.
enum BoardRegion {
    /// No outline at all: containment is not checked.
    Unbounded,
    /// Curved/footprint/open outline: containment cannot be proven.
    Unsupported,
    /// Closed rings, combined by parity (cutouts are holes).
    Rings(Vec<Vec<(f64, f64)>>),
}

fn alternative_board_region(pcb: &Pcb) -> BoardRegion {
    let (ox, oy) = pcb.board_origin();
    let xy = |n: Option<&SExp>| -> Option<(f64, f64)> {
        let n = n?;
        if n.atoms().count() != 2 {
            return None;
        }
        let a: Vec<f64> = n.atoms().filter_map(|v| v.as_f64()).collect();
        if a.len() != 2 || !a[0].is_finite() || !a[1].is_finite() {
            return None;
        }
        Some((a[0] - ox, a[1] - oy))
    };
    let mut lines: Vec<((f64, f64), (f64, f64))> = Vec::new();
    let mut rings: Vec<Vec<(f64, f64)>> = Vec::new();
    for node in &pcb.sexp().children {
        if node.is_atom() {
            continue;
        }
        let fp = matches!(node.tag(), Some("footprint" | "module"));
        let items: Vec<&SExp> = if fp {
            node.children.iter().collect()
        } else {
            vec![node]
        };
        for item in items {
            if item.find("layer").and_then(|l| l.text_at(0)).as_deref() != Some("Edge.Cuts") {
                continue;
            }
            if fp {
                return BoardRegion::Unsupported;
            }
            match item.tag() {
                Some("gr_line") => match (xy(item.find("start")), xy(item.find("end"))) {
                    (Some(a), Some(b)) => lines.push((a, b)),
                    _ => return BoardRegion::Unsupported,
                },
                Some("gr_rect") => match (xy(item.find("start")), xy(item.find("end"))) {
                    (Some((x1, y1)), Some((x2, y2))) => {
                        rings.push(vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)])
                    }
                    _ => return BoardRegion::Unsupported,
                },
                Some("gr_poly") => {
                    let Some(pts) = item.find("pts") else {
                        return BoardRegion::Unsupported;
                    };
                    let v: Option<Vec<_>> = pts
                        .children
                        .iter()
                        .filter(|c| c.has_tag("xy"))
                        .map(|c| xy(Some(c)))
                        .collect();
                    match v {
                        Some(v) if v.len() >= 3 => rings.push(v),
                        _ => return BoardRegion::Unsupported,
                    }
                }
                _ => return BoardRegion::Unsupported,
            }
        }
    }
    if lines.is_empty() && rings.is_empty() {
        return BoardRegion::Unbounded;
    }
    let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6;
    while let Some((a, b)) = lines.pop() {
        let mut ring = vec![a, b];
        loop {
            let last = *ring.last().expect("non-empty");
            if near(last, ring[0]) {
                ring.pop();
                break;
            }
            let Some(i) = lines
                .iter()
                .position(|(p, q)| near(*p, last) || near(*q, last))
            else {
                return BoardRegion::Unsupported;
            };
            let (p, q) = lines.remove(i);
            ring.push(if near(p, last) { q } else { p });
        }
        if ring.len() < 3 {
            return BoardRegion::Unsupported;
        }
        rings.push(ring);
    }
    BoardRegion::Rings(rings)
}

fn alternative_contained(region: &BoardRegion, via: &Via, target: (f64, f64), width: f64) -> bool {
    use crate::validate::rules::placement::point_in_polygon;
    let rings = match region {
        BoardRegion::Unbounded => return true,
        BoardRegion::Unsupported => return false,
        BoardRegion::Rings(r) => r,
    };
    let inside = |p: (f64, f64)| {
        rings
            .iter()
            .filter(|r| point_in_polygon(p.0, p.1, r))
            .count()
            % 2
            == 1
    };
    if !inside(via.position) || !inside(target) {
        return false;
    }
    let need = width.max(via.size) / 2.0 + 1e-6;
    for r in rings {
        for i in 0..r.len() {
            let (a, b) = (r[i], r[(i + 1) % r.len()]);
            if sh::segment_to_segment(via.position, target, a, b) < need {
                return false;
            }
        }
    }
    true
}

/// Plane-stitch context: zone boundaries, source pad, net name.
type PlaneCtx<'a> = (&'a [Vec<(f64, f64)>], &'a Pad, &'a str);

#[allow(clippy::too_many_arguments)]
fn first_offpad_candidate(
    pcb: &Pcb,
    via_index: usize,
    via: &Via,
    bbox: Bbox,
    pads: &PadsByNet,
    tht: &ThtPads,
    min_clearance: f64,
    min_h2h: f64,
    stub_layers: &[String],
    stub_width: f64,
    min_hole_clearance: Option<f64>,
    plane: Option<PlaneCtx<'_>>,
    search_alternatives: bool,
) -> Result<Option<(f64, f64)>> {
    use crate::validate::rules::placement::point_in_polygon;
    let (mut cx, mut cy) = ((bbox.0 + bbox.2) / 2.0, (bbox.1 + bbox.3) / 2.0);
    let region = if search_alternatives {
        alternative_board_region(pcb)
    } else {
        BoardRegion::Unbounded
    };
    if search_alternatives {
        (cx, cy) = via.position;
    }
    let via_r = via.size / 2.0;
    let step = via.size + min_clearance;
    for extra in [0.0, step * 0.5, step] {
        for (dx, dy) in PLANE_DIRECTIONS {
            let t = ray_aabb_exit_distance(cx, cy, dx, dy, bbox);
            let slide = t + via_r + min_clearance + extra;
            let (nx, ny) = (cx + dx * slide, cy + dy * slide);
            if plane.is_none() {
                if !alternative_contained(&region, via, (nx, ny), stub_width) {
                    continue;
                }
                if search_alternatives
                    && pcb.footprints().iter().any(|fp| {
                        fp.pads.iter().any(|p| {
                            is_smd_pad(p)
                                && dist_point_to_aabb(nx, ny, pad_absolute_bbox(p, fp))
                                    < via.drill / 2.0 + min_clearance - 1e-6
                        })
                    })
                {
                    continue;
                }
            }
            if dist_point_to_aabb(nx, ny, bbox) - via_r < min_clearance - 1e-6 {
                continue;
            }
            if let Some((polys, _, _)) = plane {
                if !polys.iter().any(|p| point_in_polygon(nx, ny, p)) {
                    continue;
                }
            }
            if check_clearance(
                pcb,
                Some(via_index),
                via,
                nx,
                ny,
                pads,
                tht,
                min_clearance,
                min_h2h,
                min_hole_clearance,
            )?
            .is_some()
            {
                continue;
            }
            let (layers, width) = match plane {
                Some((_, pad, net_name)) => {
                    (plane_stub_layers(pcb, via, pad, (nx, ny), net_name), via.size)
                }
                None => (stub_layers.to_vec(), stub_width),
            };
            if check_stub_clearance(
                pcb,
                via_index,
                via,
                (nx, ny),
                &layers,
                width,
                min_clearance,
                min_hole_clearance,
            )?
            .is_some()
            {
                continue;
            }
            return Ok(Some((nx, ny)));
        }
    }
    Ok(None)
}

fn persist_via_with_stubs(
    pcb: &mut Pcb,
    via_index: usize,
    target: (f64, f64),
    stub_layers: &[String],
    stub_width: f64,
    net_name: &str,
) -> bool {
    use crate::schema::pcb::TraceOptions;
    let backup = pcb.clone();
    let old = pcb.vias()[via_index].position;
    let mut ok = pcb.relocate_via(via_index, target);
    if ok {
        for layer in stub_layers {
            let opts = TraceOptions {
                width: stub_width,
                layer: layer.clone(),
                net: (!net_name.is_empty()).then(|| net_name.to_string()),
                ..TraceOptions::default()
            };
            if pcb.add_trace(old, target, opts).is_err() {
                ok = false;
                break;
            }
        }
    }
    if !ok {
        *pcb = backup;
    }
    ok
}

fn skip(via: &Via, net_name: &str, pad_ref: &str, reason: &str, category: &str) -> ViaRelocationSkip {
    ViaRelocationSkip {
        x: via.position.0,
        y: via.position.1,
        net: via.net_number,
        net_name: net_name.to_string(),
        pad_ref: pad_ref.to_string(),
        reason: reason.to_string(),
        uuid: via.uuid.clone(),
        category: category.to_string(),
    }
}

/// Slide in-pad vias off-pad, preserving connectivity (Phases 1-3).
pub fn relocate_in_pad_vias(
    pcb: &mut Pcb,
    rules: &crate::manufacturers::DesignRules,
    nets: Option<&std::collections::BTreeSet<String>>,
    dry_run: bool,
    min_hole_clearance_mm: Option<f64>,
    search_alternatives: bool,
) -> Result<RelocationResult> {
    use crate::validate::rules::via_in_pad::via_inside_pad;
    let min_hole_clearance = Some(resolve_hole_clearance(pcb, min_hole_clearance_mm)?);
    if dry_run {
        let mut copy = pcb.clone();
        return relocate_in_pad_vias(
            &mut copy,
            rules,
            nets,
            false,
            min_hole_clearance,
            search_alternatives,
        );
    }
    let mut result = RelocationResult::default();
    if rules.via_in_pad_supported {
        result.supported_noop = true;
        return Ok(result);
    }
    let (min_clearance, min_h2h) = (rules.min_clearance_mm, rules.min_hole_to_hole_mm);
    let pads = collect_smd_pads_by_net(pcb);
    let tht = collect_tht_pads(pcb);
    for vi in 0..pcb.vias().len() {
        let via = pcb.vias()[vi].clone();
        if via.net_number == 0 {
            continue;
        }
        let Some(cands) = pads.get(&via.net_number) else {
            continue;
        };
        let fps = pcb.footprints();
        let Some(&(fi, pi, bbox)) = cands.iter().find(|(fi, pi, b)| {
            via_inside_pad(&via, *b, Some((&fps[*fi].pads[*pi], &fps[*fi])))
        }) else {
            continue;
        };
        let fp = fps[fi].clone();
        let pad = fp.pads[pi].clone();
        let pad_ref = format!("{}-{}", fp.reference, pad.number);
        let net_name = if !via.net_name.is_empty() {
            via.net_name.clone()
        } else {
            pad.net_name.clone()
        };
        if nets.is_some_and(|n| !n.contains(&net_name)) {
            continue;
        }
        let (vx, vy) = via.position;
        let connected: Vec<(Segment, (f64, f64))> = pcb
            .segments_in_net(via.net_number)
            .filter_map(|s| endpoint_at(s, vx, vy).map(|far| (s.clone(), far)))
            .collect();
        if connected.is_empty() {
            let polys = same_net_zone_boundaries(pcb, &via, &net_name);
            if polys.is_empty() {
                result.unresolvable.push(skip(
                    &via,
                    &net_name,
                    &pad_ref,
                    "plane-stitch via has no same-net zone on a via layer to relocate into",
                    "unresolvable",
                ));
                continue;
            }
            let target = first_offpad_candidate(
                pcb,
                vi,
                &via,
                bbox,
                &pads,
                &tht,
                min_clearance,
                min_h2h,
                &[],
                0.2,
                min_hole_clearance,
                Some((&polys, &pad, &net_name)),
                false,
            )?;
            let Some(target) = target else {
                result.unresolvable.push(skip(
                    &via,
                    &net_name,
                    &pad_ref,
                    "plane-stitch via: no clearance-legal off-pad location inside the same-net zone boundary (boxed in)",
                    "unresolvable",
                ));
                continue;
            };
            let layers = plane_stub_layers(pcb, &via, &pad, target, &net_name);
            if !persist_via_with_stubs(pcb, vi, target, &layers, via.size, &net_name) {
                result.unresolvable.push(skip(
                    &via,
                    &net_name,
                    &pad_ref,
                    "could not locate backing (via ...) node to persist move",
                    "unresolvable",
                ));
                continue;
            }
            result.moved.push(ViaRelocation {
                old_x: vx,
                old_y: vy,
                new_x: target.0,
                new_y: target.1,
                net: via.net_number,
                net_name: net_name.clone(),
                pad_ref,
                uuid: via.uuid.clone(),
                stub_layers: layers,
                kind: "plane-stitch".into(),
            });
            continue;
        }
        let (pcx, pcy) = ((bbox.0 + bbox.2) / 2.0, (bbox.1 + bbox.3) / 2.0);
        // Python `max` keeps the first maximal element.
        let mut escape = &connected[0];
        for c in &connected[1..] {
            if (c.1 .0 - pcx).hypot(c.1 .1 - pcy) > (escape.1 .0 - pcx).hypot(escape.1 .1 - pcy) {
                escape = c;
            }
        }
        let far = escape.1;
        let mut stub_layers: Vec<String> = Vec::new();
        for l in std::iter::once(pad_copper_layer(&pad))
            .chain(connected.iter().map(|(s, _)| s.layer.clone()))
        {
            if !l.is_empty() && !stub_layers.contains(&l) {
                stub_layers.push(l);
            }
        }
        let mut stub_width = if escape.0.width > 0.0 {
            escape.0.width
        } else {
            0.2
        };
        let (new_x, new_y);
        if dist_point_to_aabb(far.0, far.1, bbox) <= 1e-6 {
            let t = first_offpad_candidate(
                pcb,
                vi,
                &via,
                bbox,
                &pads,
                &tht,
                min_clearance,
                min_h2h,
                &stub_layers,
                stub_width,
                min_hole_clearance,
                None,
                false,
            )?;
            let Some(t) = t else {
                result.unresolvable.push(skip(
                    &via,
                    &net_name,
                    &pad_ref,
                    "multi-branch / internal escape: no clearance-legal off-pad location (boxed in)",
                    "unresolvable",
                ));
                continue;
            };
            (new_x, new_y) = t;
        } else {
            let (mut dx, mut dy) = (far.0 - vx, far.1 - vy);
            let len = dx.hypot(dy);
            if len < 1e-9 {
                result.unresolvable.push(skip(
                    &via,
                    &net_name,
                    &pad_ref,
                    "degenerate escape-track direction",
                    "unresolvable",
                ));
                continue;
            }
            dx /= len;
            dy /= len;
            let slide =
                ray_aabb_exit_distance(vx, vy, dx, dy, bbox) + via.drill / 2.0 + min_clearance;
            let (mut nx, mut ny) = (vx + dx * slide, vy + dy * slide);
            let mut reason = check_clearance(
                pcb,
                Some(vi),
                &via,
                nx,
                ny,
                &pads,
                &tht,
                min_clearance,
                min_h2h,
                min_hole_clearance,
            )?;
            if search_alternatives {
                if reason.is_none() {
                    reason = check_stub_clearance(
                        pcb,
                        vi,
                        &via,
                        (nx, ny),
                        &stub_layers,
                        stub_width,
                        min_clearance,
                        min_hole_clearance,
                    )?;
                }
                if let Some(r) = reason.clone() {
                    let all = copper_names(pcb);
                    let idx: Vec<usize> = via
                        .layers
                        .iter()
                        .filter_map(|l| all.iter().position(|a| a == l))
                        .collect();
                    let alt_layers = if idx.len() == via.layers.len() && idx.len() >= 2 {
                        let lo = *idx.iter().min().expect("non-empty");
                        let hi = *idx.iter().max().expect("non-empty");
                        all[lo..=hi].to_vec()
                    } else {
                        Vec::new()
                    };
                    let alt_width = stub_width.max(via.size);
                    let t = if alt_layers.is_empty() {
                        None
                    } else {
                        first_offpad_candidate(
                            pcb,
                            vi,
                            &via,
                            bbox,
                            &pads,
                            &tht,
                            min_clearance,
                            min_h2h,
                            &alt_layers,
                            alt_width,
                            min_hole_clearance,
                            None,
                            true,
                        )?
                    };
                    match t {
                        Some(t) => {
                            (nx, ny) = t;
                            stub_layers = alt_layers;
                            stub_width = alt_width;
                            reason = None;
                        }
                        None => {
                            reason = Some(
                                r + "; no safe bounded alternative (clearance/board containment)",
                            )
                        }
                    }
                }
            }
            if let Some(r) = reason {
                result
                    .skipped
                    .push(skip(&via, &net_name, &pad_ref, &r, "skipped"));
                continue;
            }
            (new_x, new_y) = (nx, ny);
        }
        if let Some(r) = check_stub_clearance(
            pcb,
            vi,
            &via,
            (new_x, new_y),
            &stub_layers,
            stub_width,
            min_clearance,
            min_hole_clearance,
        )? {
            result
                .unresolvable
                .push(skip(&via, &net_name, &pad_ref, &r, "unresolvable"));
            continue;
        }
        if !persist_via_with_stubs(pcb, vi, (new_x, new_y), &stub_layers, stub_width, &net_name) {
            result.unresolvable.push(skip(
                &via,
                &net_name,
                &pad_ref,
                "could not locate backing (via ...) node to persist move",
                "unresolvable",
            ));
            continue;
        }
        result.moved.push(ViaRelocation {
            old_x: vx,
            old_y: vy,
            new_x,
            new_y,
            net: via.net_number,
            net_name,
            pad_ref,
            uuid: via.uuid.clone(),
            stub_layers,
            kind: "signal".into(),
        });
    }
    Ok(result)
}

/// `print_relocation_results`.
pub fn print_relocation_results(
    result: &RelocationResult,
    format: &str,
    dry_run: bool,
    mfr: Option<&str>,
) {
    use crate::jobj;
    use crate::pyjson::{dumps_indent, Json};
    let skip_json = |s: &ViaRelocationSkip| {
        jobj! {
            "x" => s.x, "y" => s.y, "net" => Json::Int(s.net), "net_name" => s.net_name.as_str(),
            "pad" => s.pad_ref.as_str(), "reason" => s.reason.as_str(), "uuid" => s.uuid.as_str(),
        }
    };
    if format == "json" {
        let data = jobj! {
            "manufacturer" => mfr,
            "dry_run" => dry_run,
            "via_in_pad_supported_noop" => result.supported_noop,
            "moved" => Json::Arr(result.moved.iter().map(|m| jobj! {
                "old_x" => m.old_x, "old_y" => m.old_y, "new_x" => m.new_x, "new_y" => m.new_y,
                "net" => Json::Int(m.net), "net_name" => m.net_name.as_str(), "pad" => m.pad_ref.as_str(),
                "uuid" => m.uuid.as_str(),
                "stub_layers" => Json::Arr(m.stub_layers.iter().map(|l| Json::Str(l.clone())).collect()),
                "kind" => m.kind.as_str(),
            }).collect()),
            "skipped" => Json::Arr(result.skipped.iter().map(skip_json).collect()),
            "unresolvable" => Json::Arr(result.unresolvable.iter().map(skip_json).collect()),
        };
        println!("{}", dumps_indent(&data, 2));
        return;
    }
    if result.supported_noop {
        let m = mfr.map(|m| format!(" {m}")).unwrap_or_default();
        println!("Manufacturer profile{m} supports via-in-pad; no relocation needed.");
        return;
    }
    let action = if dry_run { "Would move" } else { "Moved" };
    if format == "summary" {
        println!("{action} {} in-pad via(s) off-pad", result.moved.len());
        if !result.skipped.is_empty() {
            println!("  {} skipped (clearance/hole-to-hole)", result.skipped.len());
        }
        if !result.unresolvable.is_empty() {
            println!("  {} unresolvable (Phase 2/3)", result.unresolvable.len());
        }
        return;
    }
    if result.moved.is_empty() && result.skipped.is_empty() && result.unresolvable.is_empty() {
        println!("No via-in-pad vias found; nothing to relocate.");
        return;
    }
    println!("{action} {} in-pad via(s) off-pad:", result.moved.len());
    for m in result.moved.iter().take(10) {
        let tie = if m.stub_layers.is_empty() {
            "plane-stitch (no stub; connected filled copper)".to_string()
        } else {
            format!("stubs on {}", m.stub_layers.join(", "))
        };
        let short: String = m.uuid.chars().take(8).collect();
        let short = if short.is_empty() { "?".to_string() } else { short };
        println!(
            "  Via {short} (net '{}') on pad {}: ({:.3}, {:.3}) -> ({:.3}, {:.3}); {tie}",
            m.net_name, m.pad_ref, m.old_x, m.old_y, m.new_x, m.new_y
        );
    }
    if result.moved.len() > 10 {
        println!("  ... and {} more", result.moved.len() - 10);
    }
    if result.moved.iter().any(|m| m.kind == "plane-stitch") {
        println!(
            "\nNote: run `kicad-cli pcb drc --refill-zones` to validate plane-stitch pour connections after saving."
        );
    }
    if !result.skipped.is_empty() {
        println!("\nSkipped {} via(s) (would violate clearance):", result.skipped.len());
        for s in result.skipped.iter().take(10) {
            println!("  Via at ({:.3}, {:.3}) on pad {}: {}", s.x, s.y, s.pad_ref, s.reason);
        }
        if result.skipped.len() > 10 {
            println!("  ... and {} more", result.skipped.len() - 10);
        }
    }
    if !result.unresolvable.is_empty() {
        println!(
            "\nUnresolvable {} via(s) (deferred to Phase 2/3 follow-ups):",
            result.unresolvable.len()
        );
        for u in result.unresolvable.iter().take(10) {
            println!("  Via at ({:.3}, {:.3}) on pad {}: {}", u.x, u.y, u.pad_ref, u.reason);
        }
        if result.unresolvable.len() > 10 {
            println!("  ... and {} more", result.unresolvable.len() - 10);
        }
    }
}

/// `fix-vias --relocate-in-pad` (`_run_relocate_in_pad`).
#[allow(clippy::too_many_arguments)]
pub fn run_relocate_in_pad(
    mfr: &str,
    layers: i64,
    copper: f64,
    pcb_path: &Path,
    output: Option<&str>,
    nets: &[String],
    dry_run: bool,
    search_alternatives: bool,
    quiet: bool,
    format: &str,
) -> Result<i32> {
    let Ok(rules) = crate::drc::checker::get_design_rules(mfr, layers, copper) else {
        eprintln!("Error: No design rules found for manufacturer '{mfr}'");
        return Ok(1);
    };
    let mut pcb = match Pcb::load(pcb_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error parsing PCB file: {e}");
            return Ok(1);
        }
    };
    let net_set: Option<std::collections::BTreeSet<String>> =
        (!nets.is_empty()).then(|| nets.iter().cloned().collect());
    let result = match relocate_in_pad_vias(
        &mut pcb,
        &rules,
        net_set.as_ref(),
        dry_run,
        None,
        search_alternatives,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error resolving relocation constraints: {e}");
            return Ok(1);
        }
    };
    if !quiet {
        print_relocation_results(&result, format, dry_run, Some(mfr));
    }
    if result.changed() && !dry_run {
        let out = output
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| pcb_path.to_path_buf());
        if let Err(e) = pcb.save(Some(&out)) {
            eprintln!("Error saving PCB file: {e}");
            return Ok(1);
        }
        if !quiet && format == "text" {
            println!(
                "\nSaved to: {}",
                super::repair_common::py_path_str(&out.to_string_lossy())
            );
        }
    }
    Ok(
        if !result.skipped.is_empty() || !result.unresolvable.is_empty() {
            2
        } else {
            0
        },
    )
}
