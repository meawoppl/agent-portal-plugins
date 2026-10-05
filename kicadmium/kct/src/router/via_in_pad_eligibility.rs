//! Component-hole census for via-in-pad process eligibility (partial port
//! of `kicad_tools.router.via_in_pad_eligibility`).

use crate::schema::pcb::Pcb;
use crate::utils::pymath::hypot;

/// One drilled component hole (`RouterPad` subset).
#[derive(Debug, Clone, PartialEq)]
pub struct HolePad {
    pub x: f64,
    pub y: f64,
    pub drill: f64,
    pub drill_size: Option<(f64, f64)>,
    pub drill_rotation: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComponentHoleContext {
    pub known: bool,
    pub nearest_distance_mm: Option<f64>,
}

/// `component_holes_from_document(pcb)`.
pub fn component_holes_from_document(pcb: &Pcb) -> Option<Vec<HolePad>> {
    let mut out = Vec::new();
    for fp in pcb.footprints() {
        let (fx, fy) = fp.position;
        let r = (-fp.rotation).to_radians();
        let (c, s) = (r.cos(), r.sin());
        for pad in &fp.pads {
            if pad.pad_type != "thru_hole" && pad.pad_type != "np_thru_hole" {
                continue;
            }
            let (px, py) = pad.position;
            if !(pad.drill_offset.0.is_finite() && pad.drill_offset.1.is_finite()) {
                return None;
            }
            out.push(HolePad {
                x: fx + px * c - py * s,
                y: fy + px * s + py * c,
                drill: pad.drill,
                drill_size: pad.drill_size,
                drill_rotation: pad.rotation,
            });
        }
    }
    Some(out)
}

fn hole_gap(p: &HolePad, vx: f64, vy: f64, via_r: f64) -> Option<f64> {
    match p.drill_size {
        None => {
            if !(p.x.is_finite() && p.y.is_finite() && p.drill.is_finite()) || p.drill <= 0.0 {
                return None;
            }
            Some(hypot(p.x - vx, p.y - vy) - via_r - p.drill / 2.0)
        }
        Some((w, h)) => {
            let a = p.drill_rotation.to_radians();
            if ![w, h, p.x, p.y, a].iter().all(|v| v.is_finite()) || w.min(h) <= 0.0 {
                return None;
            }
            let (dx, dy) = (vx - p.x, vy - p.y);
            let mut lx = dx * a.cos() - dy * a.sin();
            let mut ly = dx * a.sin() + dy * a.cos();
            let half = (w - h).abs() / 2.0;
            if w >= h {
                lx -= (-half).max(half.min(lx));
            } else {
                ly -= (-half).max(half.min(ly));
            }
            Some(hypot(lx, ly) - (if h < w { h } else { w }) / 2.0 - via_r)
        }
    }
}

/// `resolve_component_hole_context`.
pub fn resolve_component_hole_context(
    vx: f64,
    vy: f64,
    drill: f64,
    all: Option<&[HolePad]>,
) -> ComponentHoleContext {
    let unknown = ComponentHoleContext {
        known: false,
        nearest_distance_mm: None,
    };
    let Some(all) = all else { return unknown };
    if !vx.is_finite() || !vy.is_finite() || !drill.is_finite() || drill <= 0.0 {
        return unknown;
    }
    let r = drill / 2.0;
    let mut nearest: Option<f64> = None;
    for p in all {
        let Some(d) = hole_gap(p, vx, vy, r) else {
            return unknown;
        };
        if !d.is_finite() {
            return unknown;
        }
        if nearest.is_none_or(|n| d < n) {
            nearest = Some(d);
        }
    }
    ComponentHoleContext {
        known: true,
        nearest_distance_mm: Some(nearest.unwrap_or(f64::INFINITY)),
    }
}
