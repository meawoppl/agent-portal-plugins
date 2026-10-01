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
    point: (f64, f64),
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
            if dist_point_to_aabb(point.0, point.1, b) - radius < clearance - 1e-6 {
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
    let reason = check_raw_copper(pcb, via, (x, y), hole_r, &layers, floor, "hole-to-copper");
    if reason.is_some() {
        return reason;
    }
    let mc = min_copper_clearance?;
    check_raw_copper(pcb, via, (x, y), via.size / 2.0, &layers, mc, "clearance")
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

/// Run the relocation pass; the full off-pad slide is not ported yet.
#[allow(clippy::too_many_arguments)]
pub fn run_relocate_in_pad(
    _mfr: &str,
    _layers: i64,
    _copper: f64,
    _pcb_path: &Path,
    _output: Option<&str>,
    _nets: &[String],
    _dry_run: bool,
    _search_alternatives: bool,
    _quiet: bool,
    _format: &str,
) -> Result<i32> {
    eprintln!(
        "Error: --relocate-in-pad is not yet available in the native kct port \
         (relocate_in_pad_vias); run fix-vias without it to resize vias"
    );
    Ok(1)
}
