//! PCB <-> router I/O (port of the load/merge halves of
//! `kicad_tools.router.io`: `load_pcb_for_routing`, `detect_layer_stack`,
//! `merge_routes_into_pcb`).
//!
//! All router coordinates are sheet-absolute (KiCad file space), so emitted
//! segments/vias can be appended to the tree verbatim.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};

use super::grid::PadShape;
use super::layers::{Layer, LayerExt, LayerStack};
use super::primitives::{Pad, Route};
use crate::core::board_outline::board_outline_segments;
use crate::core::geometry::rotate_pad_offset;
use crate::schema::pcb::{Pcb, StripOptions};

/// One pad with its copper shape and the copper layer names it occupies.
#[derive(Debug, Clone)]
pub struct BoardPad {
    pub pad: Pad,
    pub shape: PadShape,
    /// Copper layer names ("F.Cu", ...) the pad has copper on.
    pub copper: Vec<String>,
    /// Plated or non-plated hole diameter (0 = none).
    pub hole: f64,
    /// Non-plated (mechanical) hole without copper.
    pub npth: bool,
}

/// Fixed copper (segment or via) already on the board.
#[derive(Debug, Clone)]
pub enum FixedCopper {
    Segment {
        a: (f64, f64),
        b: (f64, f64),
        width: f64,
        layer: String,
        net: i64,
    },
    Via {
        at: (f64, f64),
        diameter: f64,
        net: i64,
    },
}

impl FixedCopper {
    pub fn net(&self) -> i64 {
        match self {
            FixedCopper::Segment { net, .. } | FixedCopper::Via { net, .. } => *net,
        }
    }
}

/// Keepout (rule area) blocking tracks and/or vias.
#[derive(Debug, Clone)]
pub struct Keepout {
    pub polygon: Vec<(f64, f64)>,
    pub layers: Vec<String>,
    pub tracks: bool,
    pub vias: bool,
}

/// Everything the router needs from a board.
#[derive(Debug, Clone)]
pub struct BoardData {
    pub pads: Vec<BoardPad>,
    /// Net id -> name (non-empty names only).
    pub nets: BTreeMap<i64, String>,
    pub outline: Vec<((f64, f64), (f64, f64))>,
    /// (min_x, min_y, max_x, max_y) of the outline (or pads when absent).
    pub bounds: (f64, f64, f64, f64),
    pub fixed: Vec<FixedCopper>,
    pub keepouts: Vec<Keepout>,
    pub copper_layer_count: usize,
    pub name_only: bool,
    /// Names of nets that own a zone on the board.
    pub zone_nets: HashSet<String>,
    /// (net name, layer) of every copper zone on the board.
    pub zone_layers: Vec<(String, String)>,
}

fn pad_shape(p: &crate::schema::pcb::Pad, cx: f64, cy: f64) -> PadShape {
    let (w, h) = p.size;
    let (hx, hy) = (w / 2.0, h / 2.0);
    let r = match p.shape.as_str() {
        "circle" => {
            let rr = w.max(h) / 2.0;
            return PadShape::circle(cx, cy, rr);
        }
        "oval" => hx.min(hy),
        "roundrect" => (p.roundrect_rratio * w.min(h)).min(hx.min(hy)),
        _ => 0.0,
    };
    PadShape {
        cx,
        cy,
        hx,
        hy,
        r,
        rot: p.rotation,
    }
}

/// Expand KiCad pad layer specs to copper layer names for an `n`-layer stack.
pub fn copper_layers_for(spec: &[String], stack: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for s in spec {
        if s == "*.Cu" || s == "F&B.Cu" {
            for l in stack {
                if !out.contains(l) {
                    out.push(l.clone());
                }
            }
        } else if stack.contains(s) && !out.contains(s) {
            out.push(s.clone());
        }
    }
    out
}

/// Copper layer names (F.Cu, In1.Cu.., B.Cu) for a stack of `n` layers.
pub fn stack_layer_names(n: usize) -> Vec<String> {
    let n = n.clamp(1, 6);
    if n == 1 {
        return vec!["F.Cu".into()];
    }
    let mut v = vec!["F.Cu".to_string()];
    for i in 1..n - 1 {
        v.push(format!("In{i}.Cu"));
    }
    v.push("B.Cu".into());
    v
}

/// Layer stack for `n` copper layers (upstream presets).
pub fn layer_stack_for(n: usize) -> LayerStack {
    match n {
        0..=2 => LayerStack::two_layer(),
        3 | 4 => LayerStack::four_layer_all_signal(),
        _ => {
            let mut s = LayerStack::six_layer_sig_gnd_sig_sig_pwr_sig();
            for l in &mut s.layers {
                l.layer_type = super::layers::LayerType::Signal;
            }
            s.name = "6-Layer ALL-SIG".into();
            s
        }
    }
}

/// Upstream `detect_layer_stack`: number of copper layers declared.
pub fn detect_layer_count(pcb: &Pcb) -> usize {
    pcb.copper_layers().len().max(2)
}

/// Load the routing view of a board for an `n_layers` stack.
pub fn load_pcb_for_routing(pcb: &Pcb, n_layers: usize) -> Result<BoardData> {
    let stack = stack_layer_names(n_layers);
    let (ox, oy) = pcb.board_origin();
    let mut nets = BTreeMap::new();
    for n in pcb.nets() {
        if n.number > 0 && !n.name.is_empty() {
            nets.insert(n.number, n.name.clone());
        }
    }
    let mut pads = Vec::new();
    for fp in pcb.footprints() {
        for p in &fp.pads {
            let (rx, ry) = rotate_pad_offset(p.position.0, p.position.1, fp.rotation);
            let cx = fp.position.0 + ox + rx;
            let cy = fp.position.1 + oy + ry;
            let npth = p.pad_type == "np_thru_hole";
            let mut copper = copper_layers_for(&p.layers, &stack);
            if p.pad_type == "thru_hole" {
                copper = stack.clone();
            }
            if npth {
                copper.clear();
            }
            let hole = if p.drill > 0.0 {
                p.drill
            } else {
                p.drill_size.map(|d| d.0.min(d.1)).unwrap_or(0.0)
            };
            let first = copper
                .first()
                .and_then(|n| Layer::from_name(n))
                .unwrap_or(Layer::FCu);
            let rpad = Pad {
                x: cx,
                y: cy,
                width: p.size.0,
                height: p.size.1,
                net: p.net_number,
                net_name: p.net_name.clone(),
                layer: first,
                r#ref: fp.reference.clone(),
                pin: p.number.clone(),
                through_hole: p.pad_type == "thru_hole",
                drill: hole,
                footprint_name: fp.name.clone(),
                rotation: p.rotation,
                shape: match p.shape.as_str() {
                    "circle" | "rect" | "oval" | "roundrect" => p.shape.clone(),
                    _ => "rect".into(),
                },
                ..Default::default()
            };
            pads.push(BoardPad {
                shape: pad_shape(p, cx, cy),
                pad: rpad,
                copper,
                hole,
                npth,
            });
        }
    }
    let outline: Vec<((f64, f64), (f64, f64))> = board_outline_segments(pcb.sexp())
        .map(|s| s.segments.clone())
        .unwrap_or_default();
    let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut grow = |x: f64, y: f64| {
        bounds.0 = bounds.0.min(x);
        bounds.1 = bounds.1.min(y);
        bounds.2 = bounds.2.max(x);
        bounds.3 = bounds.3.max(y);
    };
    if outline.is_empty() {
        for p in &pads {
            let b = p.shape.bbox();
            grow(b.0 - 2.0, b.1 - 2.0);
            grow(b.2 + 2.0, b.3 + 2.0);
        }
    } else {
        for (a, b) in &outline {
            grow(a.0, a.1);
            grow(b.0, b.1);
        }
    }
    if bounds.0 > bounds.2 {
        bounds = (0.0, 0.0, 10.0, 10.0);
    }
    let mut fixed = Vec::new();
    for s in pcb.segments() {
        fixed.push(FixedCopper::Segment {
            a: (s.start.0 + ox, s.start.1 + oy),
            b: (s.end.0 + ox, s.end.1 + oy),
            width: s.width,
            layer: s.layer.clone(),
            net: s.net_number,
        });
    }
    for a in pcb.arcs() {
        // Arcs as chords through the midpoint.
        for (p, q) in [(a.start, a.mid), (a.mid, a.end)] {
            fixed.push(FixedCopper::Segment {
                a: (p.0 + ox, p.1 + oy),
                b: (q.0 + ox, q.1 + oy),
                width: a.width,
                layer: a.layer.clone(),
                net: a.net_number,
            });
        }
    }
    for v in pcb.vias() {
        fixed.push(FixedCopper::Via {
            at: (v.position.0 + ox, v.position.1 + oy),
            diameter: v.size,
            net: v.net_number,
        });
    }
    let mut keepouts = Vec::new();
    let mut zone_nets = HashSet::new();
    let mut zone_layers = Vec::new();
    for z in pcb.zones() {
        if let Some(k) = &z.keepout {
            let mut layers = z.layers.clone();
            if layers.is_empty() && !z.layer.is_empty() {
                layers.push(z.layer.clone());
            }
            keepouts.push(Keepout {
                polygon: z.polygon.iter().map(|p| (p.0 + ox, p.1 + oy)).collect(),
                layers: copper_layers_for(&layers, &stack),
                tracks: !k.tracks_allowed,
                vias: !k.vias_allowed,
            });
        } else if !z.net_name.is_empty() {
            zone_nets.insert(z.net_name.clone());
            let mut layers = z.layers.clone();
            if layers.is_empty() && !z.layer.is_empty() {
                layers.push(z.layer.clone());
            }
            for l in copper_layers_for(&layers, &stack) {
                zone_layers.push((z.net_name.clone(), l));
            }
        }
    }
    Ok(BoardData {
        pads,
        nets,
        outline,
        bounds,
        fixed,
        keepouts,
        copper_layer_count: detect_layer_count(pcb),
        name_only: pcb.net_name_only_dialect(),
        zone_nets,
        zone_layers,
    })
}

/// Rewrite the board's copper layer table for `target_layers` (upstream
/// `update_pcb_layer_stackup`): inner layers In1..In(n-2) are added when
/// missing. Returns the new text.
pub fn update_pcb_layer_stackup(pcb_content: &str, target_layers: usize) -> String {
    if target_layers <= 2 {
        return pcb_content.to_string();
    }
    let Some(start) = pcb_content.find("(layers") else {
        return pcb_content.to_string();
    };
    let mut out = pcb_content.to_string();
    for i in 1..target_layers - 1 {
        let name = format!("\"In{i}.Cu\"");
        if pcb_content[start..].contains(&name) {
            continue;
        }
        // KiCad layer ids: F.Cu=0, B.Cu=2 (v9+) / 31 (v8); inner layers use
        // even ids from 4 in v9+ and 1.. in v8.
        let v9 = pcb_content[start..].contains("(2 \"B.Cu\"");
        let id = if v9 { 2 + 2 * i } else { i };
        let line = format!("\n\t\t({id} {name} signal)");
        if let Some(pos) = out[start..].find("\"F.Cu\"") {
            let abs = start + pos;
            if let Some(close) = out[abs..].find(')') {
                let at = abs + close + 1;
                out.insert_str(at, &line);
            }
        }
    }
    out
}

/// Strip existing tracks of `nets` and append `routes`; save to `output`.
pub fn merge_routes_into_pcb(
    pcb_path: &Path,
    output: &Path,
    routes: &[Route],
    strip_nets: &[String],
    layers: usize,
) -> Result<()> {
    let text = std::fs::read_to_string(pcb_path)
        .with_context(|| format!("reading {}", pcb_path.display()))?;
    let text = update_pcb_layer_stackup(&text, layers);
    let mut pcb = Pcb::parse_str(&text)?;
    if !strip_nets.is_empty() {
        pcb.strip_traces(StripOptions {
            nets: Some(strip_nets.to_vec()),
            keep_zones: true,
            ..Default::default()
        });
    }
    let name_only = pcb.net_name_only_dialect();
    for r in routes {
        for seg in &r.segments {
            let text = seg.to_sexp(name_only).map_err(|e| anyhow::anyhow!("{e}"))?;
            let node = crate::sexp::parse(&text)?;
            pcb.sexp_mut().push(node);
        }
        for via in &r.vias {
            let node = crate::sexp::parse(&via.to_sexp(name_only))?;
            pcb.sexp_mut().push(node);
        }
    }
    pcb.save(Some(output))?;
    Ok(())
}

/// Pads grouped by net id (only nets with a name), in board order.
pub fn pads_by_net(board: &BoardData) -> BTreeMap<i64, Vec<usize>> {
    let mut m: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (i, p) in board.pads.iter().enumerate() {
        if p.pad.net > 0 && !p.copper.is_empty() {
            m.entry(p.pad.net).or_default().push(i);
        }
    }
    m
}
