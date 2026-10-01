//! Thin copper sliver tips (port of
//! `kicad_tools.validate.rules.copper_sliver`, KiCad-compatible acute-tip
//! geometry).

use std::collections::HashMap;

use super::clearance::{collect_zone_fills, pad_on_layer, pad_polygon};
use crate::core::layers::via_spans_layer;
use crate::geometry::shapely::{self as sh, Geom, Poly};
use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::schema::pcb::Pcb;
use crate::utils::pymath::hypot;
use crate::validate::rules::DRC_TOLERANCE;
use crate::validate::violations::{DRCResults, DRCViolation};

pub const KICAD_SLIVER_WIDTH_TOLERANCE_MM: f64 = 0.08;
pub const KICAD_SLIVER_MINIMUM_LENGTH_MM: f64 = 0.0008;
pub const KICAD_SLIVER_MINIMUM_VERTEX_COUNT: usize = 6;
pub const KICAD_SLIVER_ANGLE_TOLERANCE_DEG: f64 = 20.0;

#[derive(Debug, Clone, Default)]
pub struct CopperSliverRule;

impl CopperSliverRule {
    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        results.set_rule("copper_sliver", 1);
        for (layer, geoms) in collect_copper_by_layer(pcb) {
            check_layer(
                &layer,
                &geoms,
                KICAD_SLIVER_WIDTH_TOLERANCE_MM,
                &mut results,
            );
        }
        results
    }
}

fn collect_copper_by_layer(pcb: &Pcb) -> Vec<(String, Vec<Geom>)> {
    let names: Vec<String> = pcb.copper_layers().iter().map(|l| l.name.clone()).collect();
    let mut by: Vec<(String, Vec<Geom>)> = names.iter().map(|n| (n.clone(), Vec::new())).collect();
    let idx = |by: &mut Vec<(String, Vec<Geom>)>, l: &str| -> usize {
        match by.iter().position(|(k, _)| k == l) {
            Some(i) => i,
            None => {
                by.push((l.to_string(), Vec::new()));
                by.len() - 1
            }
        }
    };
    for (layer, fills) in collect_zone_fills(pcb, false) {
        let i = idx(&mut by, &layer);
        for f in fills {
            by[i].1.push(f.polygon);
        }
    }
    for name in &names {
        let i = idx(&mut by, name);
        for s in pcb.segments_on_layer(name) {
            if s.width <= 0.0 {
                continue;
            }
            by[i]
                .1
                .push(sh::segment_buffer(s.start, s.end, s.width / 2.0));
        }
    }
    for fp in pcb.footprints() {
        for pad in &fp.pads {
            let Some(poly) = pad_polygon(pad, fp) else {
                continue;
            };
            if poly.is_empty() {
                continue;
            }
            for name in &names {
                if pad_on_layer(pad, name) {
                    let i = idx(&mut by, name);
                    by[i].1.push(poly.clone());
                }
            }
        }
        for g in &fp.graphics {
            if g.graphic_type != "poly" || g.points.len() < 3 {
                continue;
            }
            let Some(i) = by.iter().position(|(k, _)| *k == g.layer) else {
                continue;
            };
            let mut poly = Geom::Poly(Poly::new(g.points.clone()));
            if fp.rotation != 0.0 {
                poly = sh::rotate(&poly, fp.rotation);
            }
            poly = sh::translate(&poly, fp.position.0, fp.position.1);
            if !poly.is_empty() {
                by[i].1.push(poly);
            }
        }
    }
    for v in pcb.vias() {
        if v.size <= 0.0 {
            continue;
        }
        let disc = sh::point_buffer(v.position, v.size / 2.0);
        for name in &names {
            if via_spans_layer(&v.layers, name) {
                let i = idx(&mut by, name);
                by[i].1.push(disc.clone());
            }
        }
    }
    by
}

/// Douglas-Peucker simplification of one closed ring (topology checks are
/// not needed at the sub-micron tolerance this rule uses).
fn simplify_ring(ring: &[(f64, f64)], tol: f64) -> Vec<(f64, f64)> {
    let n = ring.len();
    if n <= 4 {
        return ring.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut far = a;
        let mut maxd = -1.0;
        for k in a + 1..b {
            let d = sh::point_to_segment(ring[k], ring[a], ring[b]);
            if d > maxd {
                maxd = d;
                far = k;
            }
        }
        if maxd > tol {
            keep[far] = true;
            stack.push((a, far));
            stack.push((far, b));
        }
    }
    let out: Vec<(f64, f64)> = ring
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(p, _)| *p)
        .collect();
    if out.len() < 4 {
        return ring.to_vec();
    }
    out
}

fn simplify(g: &Geom, tol: f64) -> Geom {
    let polys: Vec<Poly> = g
        .polys()
        .into_iter()
        .map(|p| Poly {
            shell: simplify_ring(&p.shell, tol),
            holes: p.holes.iter().map(|h| simplify_ring(h, tol)).collect(),
        })
        .collect();
    match polys.len() {
        0 => Geom::Empty,
        1 => Geom::Poly(polys.into_iter().next().unwrap()),
        _ => Geom::Multi(polys),
    }
}

fn check_layer(layer: &str, geoms: &[Geom], tol: f64, results: &mut DRCResults) {
    if geoms.is_empty() {
        return;
    }
    let g = sh::unary_union(geoms);
    if g.is_empty() {
        return;
    }
    let g = simplify(&g, 0.5 * DRC_TOLERANCE);
    if g.is_empty() {
        return;
    }
    for comp in g.polys() {
        for (tip, angle, width) in sliver_tips(comp, tol) {
            results.add(
                DRCViolation::new(
                    "copper_sliver",
                    "warning",
                    format!(
                        "Copper sliver on {layer}: acute tip exceeds native chord tolerance \
                         {tol:.3}mm (tip angle {angle:.1}°, opposite width {width:.3}mm)"
                    ),
                )
                .at(py_round(tip.0, 3), py_round(tip.1, 3))
                .layer(layer)
                .actual(py_round(width, 4))
                .required(tol),
            );
        }
    }
}

fn resolve_arm(coords: &[(f64, f64)], index: usize, step: isize) -> Option<usize> {
    let n = coords.len() as isize;
    let (x, y) = coords[index];
    for off in 1..n {
        let c = (index as isize + step * off).rem_euclid(n) as usize;
        let (cx, cy) = coords[c];
        if (cx - x).abs() >= KICAD_SLIVER_MINIMUM_LENGTH_MM
            || (cy - y).abs() >= KICAD_SLIVER_MINIMUM_LENGTH_MM
        {
            return Some(c);
        }
    }
    None
}

fn chord_locally_inside(coords: &[(f64, f64)], a: usize, tip: usize, b: usize) -> bool {
    let area = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
        (q.1 - p.1) * (r.0 - q.0) - (q.0 - p.0) * (r.1 - q.1)
    };
    let Some(prev) = resolve_arm(coords, a, -1) else {
        return false;
    };
    let f = tip;
    if area(coords[prev], coords[a], coords[f]) < 0.0 {
        return area(coords[a], coords[b], coords[f]) >= 0.0
            && area(coords[a], coords[prev], coords[b]) >= 0.0;
    }
    area(coords[a], coords[b], coords[prev]) < 0.0 || area(coords[a], coords[f], coords[b]) < 0.0
}

type Tip = ((f64, f64), f64, f64);

fn sliver_tips(comp: &Poly, tol: f64) -> Vec<Tip> {
    let rings: Vec<&Vec<(f64, f64)>> = std::iter::once(&comp.shell)
        .chain(comp.holes.iter())
        .collect();
    let open = |r: &Vec<(f64, f64)>| -> Vec<(f64, f64)> { r[..r.len().saturating_sub(1)].to_vec() };
    let key = |p: (f64, f64)| (p.0.to_bits(), p.1.to_bits());
    let mut membership: HashMap<(u64, u64), usize> = HashMap::new();
    for r in &rings {
        let mut seen = std::collections::HashSet::new();
        for p in open(r) {
            if seen.insert(key(p)) {
                *membership.entry(key(p)).or_default() += 1;
            }
        }
    }
    let mut out = Vec::new();
    for r in &rings {
        let coords = open(r);
        if coords.len() < KICAD_SLIVER_MINIMUM_VERTEX_COUNT {
            continue;
        }
        let mut seen_arms: std::collections::HashSet<(usize, usize)> = Default::default();
        for (i, &cur) in coords.iter().enumerate() {
            let (Some(pi), Some(ni)) = (resolve_arm(&coords, i, -1), resolve_arm(&coords, i, 1))
            else {
                continue;
            };
            if seen_arms.contains(&(pi, ni)) {
                continue;
            }
            if membership.get(&key(cur)).copied().unwrap_or(0) > 1 {
                continue;
            }
            let (prev, next) = (coords[pi], coords[ni]);
            if !chord_locally_inside(&coords, pi, i, ni) {
                continue;
            }
            let a1 = hypot(prev.0 - cur.0, prev.1 - cur.1);
            let a2 = hypot(next.0 - cur.0, next.1 - cur.1);
            if a1 <= 0.0 || a2 <= 0.0 {
                continue;
            }
            let dot = (prev.0 - cur.0) * (next.0 - cur.0) + (prev.1 - cur.1) * (next.1 - cur.1);
            let cosine = (dot / (a1 * a2)).clamp(-1.0, 1.0);
            let angle = cosine.acos().to_degrees();
            let width = hypot(next.0 - prev.0, next.1 - prev.1);
            if angle < KICAD_SLIVER_ANGLE_TOLERANCE_DEG && width > tol {
                let cross = ((next.0 - prev.0) * (cur.1 - prev.1)
                    - (next.1 - prev.1) * (cur.0 - prev.0))
                    .abs();
                if cross / width <= tol {
                    continue;
                }
                seen_arms.insert((pi, ni));
                out.push((cur, angle, width));
            }
        }
    }
    out
}
