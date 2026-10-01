//! `kct fix-vias` (port of `kicad_tools.cli.fix_vias_cmd`; public flags from
//! `parser._add_fix_vias_parser`).
//!
//! Repairs same-layer through vias (layers reset to the outer copper pair)
//! and resizes undersized vias to the manufacturer minimum (drill, diameter,
//! annular ring), optionally skipping vias whose enlargement would violate
//! clearance. Exit codes: 0 clean, 1 error, 2 completed with warnings.

use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use clap::Parser;

use super::repair_common::{
    atoms_f64, first_f64, first_str, load_design_rules_from_yaml, pick_rules, py_path_str,
    ALL_MANUFACTURER_NAMES,
};
use super::{parse_args, Globals};
use crate::jobj;
use crate::pyjson::{dumps_indent, py_float_repr, Json};
use crate::schema::pcb::is_footprint_tag;
use crate::sexp::SExp;

#[derive(Parser, Debug)]
#[command(about = "Fix vias to meet manufacturer specifications")]
struct Args {
    /// Path to .kicad_pcb file
    pcb: String,
    /// With --relocate-in-pad, search safe alternate escapes if the preferred slide is blocked
    #[arg(long)]
    search_alternatives: bool,
    /// Manufacturer to use for design rules (default: jlcpcb)
    #[arg(long, default_value = "jlcpcb", value_parser = clap::builder::PossibleValuesParser::new(ALL_MANUFACTURER_NAMES))]
    mfr: String,
    /// Number of PCB layers (auto-detected from board if not specified)
    #[arg(long)]
    layers: Option<i64>,
    /// Outer copper weight in oz (default: 1.0)
    #[arg(long, default_value_t = 1.0)]
    copper: f64,
    /// Target drill diameter in mm (overrides manufacturer rules)
    #[arg(long)]
    drill: Option<f64>,
    /// Target via diameter in mm (overrides manufacturer rules)
    #[arg(long)]
    diameter: Option<f64>,
    /// Output file path (default: overwrite input)
    #[arg(short = 'o', long)]
    output: Option<String>,
    /// Preview changes without modifying files
    #[arg(long)]
    dry_run: bool,
    /// Output format (default: text)
    #[arg(long, default_value = "text", value_parser = ["text", "json", "summary"])]
    format: String,
    /// Suppress progress output (for scripting)
    #[arg(short = 'q', long)]
    quiet: bool,
    /// Skip resizing vias that would cause clearance violations
    #[arg(long)]
    skip_if_clearance_violation: bool,
    /// Relocate via-in-pad vias off-pad (connectivity-preserving)
    #[arg(long)]
    relocate_in_pad: bool,
    /// Restrict --relocate-in-pad to the given net name (repeatable)
    #[arg(long = "net", value_name = "NET")]
    nets: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ViaInfo {
    pub index: usize,
    pub x: f64,
    pub y: f64,
    pub drill: f64,
    pub diameter: f64,
    pub net: i64,
    pub uuid: String,
    pub start_layer: String,
    pub end_layer: String,
    pub via_type: String,
}

#[derive(Debug, Clone)]
pub struct ViaFix {
    pub x: f64,
    pub y: f64,
    pub net: i64,
    pub old_drill: f64,
    pub new_drill: f64,
    pub old_diameter: f64,
    pub new_diameter: f64,
    pub uuid: String,
}

#[derive(Debug, Clone)]
pub struct SameLayerViaFix {
    pub x: f64,
    pub y: f64,
    pub net: i64,
    pub old_start_layer: String,
    pub old_end_layer: String,
    pub new_start_layer: String,
    pub new_end_layer: String,
    pub uuid: String,
}

#[derive(Debug, Clone)]
pub struct SameLayerViaWarning {
    pub x: f64,
    pub y: f64,
    pub net: i64,
    pub start_layer: String,
    pub end_layer: String,
    pub via_type: String,
    pub uuid: String,
}

#[derive(Debug, Clone)]
pub struct ViaClearanceWarning {
    pub x: f64,
    pub y: f64,
    pub new_diameter: f64,
    pub nearby_item: String,
    pub clearance_mm: f64,
}

#[derive(Debug, Clone)]
pub struct ViaSkip {
    pub x: f64,
    pub y: f64,
    pub net: i64,
    pub current_drill: f64,
    pub current_diameter: f64,
    pub would_be_diameter: f64,
    pub reason: String,
    pub uuid: String,
}

/// `(min_drill, min_diameter, min_annular_ring, min_clearance)`.
pub fn get_design_rules(
    mfr: Option<&str>,
    layers: i64,
    copper: f64,
    drill: Option<f64>,
    diameter: Option<f64>,
) -> (f64, f64, f64, f64) {
    if let (Some(d), Some(dia)) = (drill, diameter) {
        return (d, dia, 0.0, 0.2);
    }
    if let Some(mfr) = mfr.filter(|m| !m.is_empty()) {
        match load_design_rules_from_yaml(mfr) {
            Some(all) => {
                if let Some(rules) = pick_rules(&all, layers, copper) {
                    let target_drill = drill.unwrap_or(rules.min_via_drill_mm);
                    let ring = rules.min_annular_ring_mm;
                    let eff = rules.min_via_diameter_mm.max(target_drill + 2.0 * ring);
                    return (
                        target_drill,
                        diameter.unwrap_or(eff),
                        ring,
                        rules.min_clearance_mm,
                    );
                }
            }
            None => eprintln!("Warning: No configuration found for manufacturer '{mfr}'"),
        }
    }
    (
        drill.filter(|d| *d != 0.0).unwrap_or(0.3),
        diameter.filter(|d| *d != 0.0).unwrap_or(0.6),
        0.0,
        0.2,
    )
}

fn closest_point_on_segment(x1: f64, y1: f64, x2: f64, y2: f64, px: f64, py: f64) -> (f64, f64, f64) {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-10 {
        return (x1, y1, ((x1 - px).powi(2) + (y1 - py).powi(2)).sqrt());
    }
    let t = (((px - x1) * dx + (py - y1) * dy) / len_sq).clamp(0.0, 1.0);
    let cx = x1 + t * dx;
    let cy = y1 + t * dy;
    (cx, cy, ((cx - px).powi(2) + (cy - py).powi(2)).sqrt())
}

fn xy(node: Option<&SExp>) -> Option<(f64, f64)> {
    let a = atoms_f64(node?);
    Some((a.first().copied().unwrap_or(0.0), a.get(1).copied().unwrap_or(0.0)))
}

/// All `(via ...)` nodes in document (pre-order) order.
pub fn find_all_vias(doc: &SExp) -> Vec<ViaInfo> {
    let mut out = Vec::new();
    for (index, via) in doc.find_all("via").enumerate() {
        let Some((x, y)) = xy(via.find("at")) else {
            continue;
        };
        let net = via
            .find("net")
            .and_then(|n| n.first_atom())
            .and_then(|v| match v {
                crate::sexp::Value::Int(i) => Some(*i),
                crate::sexp::Value::Str(s) => s.trim().parse().ok(),
                crate::sexp::Value::Float(_) => None,
            })
            .unwrap_or(0);
        let (start_layer, end_layer) = match via.find("layers") {
            Some(l) => (
                l.text_at(0).unwrap_or_default(),
                l.text_at(1).unwrap_or_default(),
            ),
            None => (String::new(), String::new()),
        };
        out.push(ViaInfo {
            index,
            x,
            y,
            drill: first_f64(via.find("drill")),
            diameter: first_f64(via.find("size")),
            net,
            uuid: first_str(via.find("uuid")),
            start_layer,
            end_layer,
            via_type: first_str(via.find("type")),
        });
    }
    out
}

/// Mutable access to the `n`-th descendant tagged `tag` (pre-order).
pub fn nth_tagged_mut<'a>(root: &'a mut SExp, tag: &str, n: usize) -> Option<&'a mut SExp> {
    fn walk<'a>(node: &'a mut SExp, tag: &str, n: usize, seen: &mut usize) -> Option<&'a mut SExp> {
        for child in node.children.iter_mut() {
            if child.has_tag(tag) {
                if *seen == n {
                    return Some(child);
                }
                *seen += 1;
            }
            // Recurse only when the target is not this child.
            let before = *seen;
            let found = walk(child, tag, n, seen);
            if found.is_some() {
                return found;
            }
            debug_assert!(*seen >= before);
        }
        None
    }
    let mut seen = 0;
    walk(root, tag, n, &mut seen)
}

/// Pads (footprint-local `at`, as upstream reads them), vias and tracks
/// within `radius`: `(type, x, y, width)`.
pub fn find_nearby_items(doc: &SExp, x: f64, y: f64, radius: f64) -> Vec<(String, f64, f64, f64)> {
    let mut items = Vec::new();
    let footprints: Vec<&SExp> = doc
        .children
        .iter()
        .flat_map(|c| c.iter_all())
        .filter(|n| is_footprint_tag(n.tag()))
        .collect();
    for fp in footprints {
        for pad in fp.find_all("pad") {
            if let Some((px, py)) = xy(pad.find("at")) {
                let dist = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
                if dist < radius {
                    let size = match pad.find("size") {
                        Some(s) => {
                            let a = atoms_f64(s);
                            let w = a.first().copied().unwrap_or(0.0);
                            let h = a.get(1).copied().unwrap_or(w);
                            w.min(h)
                        }
                        None => 0.0,
                    };
                    items.push(("pad".to_string(), px, py, size));
                }
            }
        }
    }
    for via in doc.find_all("via") {
        if let Some((vx, vy)) = xy(via.find("at")) {
            if (vx - x).abs() < 0.001 && (vy - y).abs() < 0.001 {
                continue;
            }
            let dist = ((vx - x).powi(2) + (vy - y).powi(2)).sqrt();
            if dist < radius {
                items.push(("via".to_string(), vx, vy, first_f64(via.find("size"))));
            }
        }
    }
    for seg in doc.find_all("segment") {
        let (Some((sx, sy)), Some((ex, ey))) = (xy(seg.find("start")), xy(seg.find("end"))) else {
            continue;
        };
        if (sx - x).abs() < 0.001 && (sy - y).abs() < 0.001 {
            continue;
        }
        if (ex - x).abs() < 0.001 && (ey - y).abs() < 0.001 {
            continue;
        }
        let (cx, cy, dist) = closest_point_on_segment(sx, sy, ex, ey, x, y);
        if dist < radius {
            items.push(("track".to_string(), cx, cy, first_f64(seg.find("width"))));
        }
    }
    items
}

/// Outer copper layer pair from the board `(layers ...)` table.
pub fn get_board_outer_layers(doc: &SExp) -> (String, String) {
    let fallback = ("F.Cu".to_string(), "B.Cu".to_string());
    let Some(section) = doc.find("layers") else {
        return fallback;
    };
    let mut copper = Vec::new();
    for child in &section.children {
        if child.children.is_empty() {
            continue;
        }
        let atoms: Vec<String> = (0..child.children.len())
            .filter_map(|i| child.text_at(i))
            .collect();
        if atoms.len() >= 2 {
            let layer_type = if atoms.len() >= 3 {
                atoms[atoms.len() - 1].as_str()
            } else {
                ""
            };
            if layer_type == "signal" || layer_type == "power" {
                copper.push(atoms[0].clone());
            }
        }
    }
    if copper.len() >= 2 {
        return (copper[0].clone(), copper[copper.len() - 1].clone());
    }
    fallback
}

/// Repair through vias whose start and end layer match; warn on blind/micro.
pub fn fix_same_layer_vias(
    doc: &mut SExp,
    dry_run: bool,
) -> (Vec<SameLayerViaFix>, Vec<SameLayerViaWarning>) {
    let (outer_start, outer_end) = get_board_outer_layers(doc);
    let mut fixes = Vec::new();
    let mut warnings = Vec::new();
    for v in find_all_vias(doc) {
        if v.start_layer.is_empty() || v.end_layer.is_empty() || v.start_layer != v.end_layer {
            continue;
        }
        if v.via_type.is_empty() || v.via_type == "through" {
            fixes.push(SameLayerViaFix {
                x: v.x,
                y: v.y,
                net: v.net,
                old_start_layer: v.start_layer.clone(),
                old_end_layer: v.end_layer.clone(),
                new_start_layer: outer_start.clone(),
                new_end_layer: outer_end.clone(),
                uuid: v.uuid.clone(),
            });
            if !dry_run {
                if let Some(via) = nth_tagged_mut(doc, "via", v.index) {
                    if let Some(layers) = find_mut(via, "layers") {
                        layers.set_value(0, outer_start.as_str());
                        layers.set_value(1, outer_end.as_str());
                    }
                }
            }
        } else {
            warnings.push(SameLayerViaWarning {
                x: v.x,
                y: v.y,
                net: v.net,
                start_layer: v.start_layer,
                end_layer: v.end_layer,
                via_type: v.via_type,
                uuid: v.uuid,
            });
        }
    }
    (fixes, warnings)
}

/// First descendant named `tag` (mutable, pre-order).
pub fn find_mut<'a>(node: &'a mut SExp, tag: &str) -> Option<&'a mut SExp> {
    nth_tagged_mut(node, tag, 0)
}

/// Resize undersized vias.
#[allow(clippy::too_many_arguments)]
pub fn fix_vias(
    doc: &mut SExp,
    target_drill: f64,
    target_diameter: f64,
    min_clearance: f64,
    dry_run: bool,
    min_annular_ring: f64,
    skip_on_clearance: bool,
) -> (Vec<ViaFix>, Vec<ViaClearanceWarning>, Vec<ViaSkip>) {
    let mut fixes = Vec::new();
    let mut warnings = Vec::new();
    let mut skips = Vec::new();
    for v in find_all_vias(doc) {
        let need_drill = v.drill < target_drill;
        let mut need_diameter = v.diameter < target_diameter;
        let new_drill = v.drill.max(target_drill);
        let ring_diameter = if min_annular_ring > 0.0 {
            let d = new_drill + 2.0 * min_annular_ring;
            need_diameter = need_diameter || v.diameter < d;
            d
        } else {
            0.0
        };
        if !need_drill && !need_diameter {
            continue;
        }
        let new_diameter = v.diameter.max(target_diameter).max(ring_diameter);
        let mut via_warnings = Vec::new();
        if new_diameter - v.diameter > 0.0 {
            let radius = new_diameter / 2.0 + min_clearance * 2.0;
            for (kind, ix, iy, w) in find_nearby_items(doc, v.x, v.y, radius) {
                let dist = ((ix - v.x).powi(2) + (iy - v.y).powi(2)).sqrt();
                let clearance = dist - new_diameter / 2.0 - w / 2.0;
                if clearance < min_clearance {
                    via_warnings.push(ViaClearanceWarning {
                        x: v.x,
                        y: v.y,
                        new_diameter,
                        nearby_item: format!("{kind} at ({ix:.2}, {iy:.2})"),
                        clearance_mm: clearance,
                    });
                }
            }
        }
        if skip_on_clearance && !via_warnings.is_empty() {
            let reasons: Vec<String> = via_warnings
                .iter()
                .map(|w| format!("{:.3}mm to {}", w.clearance_mm, w.nearby_item))
                .collect();
            skips.push(ViaSkip {
                x: v.x,
                y: v.y,
                net: v.net,
                current_drill: v.drill,
                current_diameter: v.diameter,
                would_be_diameter: new_diameter,
                reason: reasons.join("; "),
                uuid: v.uuid.clone(),
            });
            continue;
        }
        warnings.extend(via_warnings);
        fixes.push(ViaFix {
            x: v.x,
            y: v.y,
            net: v.net,
            old_drill: v.drill,
            new_drill,
            old_diameter: v.diameter,
            new_diameter,
            uuid: v.uuid.clone(),
        });
        if !dry_run {
            if let Some(via) = nth_tagged_mut(doc, "via", v.index) {
                if need_drill {
                    if let Some(d) = find_mut(via, "drill") {
                        d.set_value(0, new_drill);
                    }
                }
                if need_diameter {
                    if let Some(s) = find_mut(via, "size") {
                        s.set_value(0, new_diameter);
                    }
                }
            }
        }
    }
    (fixes, warnings, skips)
}

fn f(v: f64) -> Json {
    Json::Float(v)
}

#[allow(clippy::too_many_arguments)]
fn print_fix_results(
    fixes: &[ViaFix],
    warnings: &[ViaClearanceWarning],
    format: &str,
    dry_run: bool,
    target_drill: f64,
    target_diameter: f64,
    mfr: &str,
    skips: &[ViaSkip],
    sl_fixes: &[SameLayerViaFix],
    sl_warnings: &[SameLayerViaWarning],
) {
    if format == "json" {
        let data = jobj! {
            "target_drill_mm" => f(target_drill),
            "target_diameter_mm" => f(target_diameter),
            "manufacturer" => mfr,
            "dry_run" => dry_run,
            "fixes" => Json::Arr(fixes.iter().map(|x| jobj! {
                "x" => f(x.x), "y" => f(x.y), "net" => Json::Int(x.net),
                "old_drill_mm" => f(x.old_drill), "new_drill_mm" => f(x.new_drill),
                "old_diameter_mm" => f(x.old_diameter), "new_diameter_mm" => f(x.new_diameter),
                "uuid" => x.uuid.as_str(),
            }).collect()),
            "warnings" => Json::Arr(warnings.iter().map(|w| jobj! {
                "x" => f(w.x), "y" => f(w.y), "new_diameter_mm" => f(w.new_diameter),
                "nearby_item" => w.nearby_item.as_str(), "clearance_mm" => f(w.clearance_mm),
            }).collect()),
            "skipped" => Json::Arr(skips.iter().map(|s| jobj! {
                "x" => f(s.x), "y" => f(s.y), "net" => Json::Int(s.net),
                "current_drill_mm" => f(s.current_drill), "current_diameter_mm" => f(s.current_diameter),
                "would_be_diameter_mm" => f(s.would_be_diameter), "reason" => s.reason.as_str(),
                "uuid" => s.uuid.as_str(),
            }).collect()),
            "same_layer_fixes" => Json::Arr(sl_fixes.iter().map(|x| jobj! {
                "x" => f(x.x), "y" => f(x.y), "net" => Json::Int(x.net),
                "old_start_layer" => x.old_start_layer.as_str(), "old_end_layer" => x.old_end_layer.as_str(),
                "new_start_layer" => x.new_start_layer.as_str(), "new_end_layer" => x.new_end_layer.as_str(),
                "uuid" => x.uuid.as_str(),
            }).collect()),
            "same_layer_warnings" => Json::Arr(sl_warnings.iter().map(|w| jobj! {
                "x" => f(w.x), "y" => f(w.y), "net" => Json::Int(w.net),
                "start_layer" => w.start_layer.as_str(), "end_layer" => w.end_layer.as_str(),
                "via_type" => w.via_type.as_str(), "uuid" => w.uuid.as_str(),
            }).collect()),
        };
        println!("{}", dumps_indent(&data, 2));
        return;
    }
    let td = py_float_repr(target_drill);
    let tdia = py_float_repr(target_diameter);
    let source = if mfr.is_empty() {
        String::new()
    } else {
        format!(" to {} minimums", mfr.to_uppercase())
    };
    if format == "summary" {
        let action = if dry_run { "Would resize" } else { "Resized" };
        println!("{action} vias{source} (drill: {td}mm, diameter: {tdia}mm):");
        println!(
            "  {} vias {}updated",
            fixes.len(),
            if dry_run { "would be " } else { "" }
        );
        if !skips.is_empty() {
            println!("  {} vias skipped (clearance violation)", skips.len());
        }
        if !warnings.is_empty() {
            println!("  {} potential clearance violations", warnings.len());
        }
        if !sl_fixes.is_empty() {
            let a = if dry_run { "Would repair" } else { "Repaired" };
            println!("  {a} {} same-layer via(s)", sl_fixes.len());
        }
        if !sl_warnings.is_empty() {
            println!(
                "  {} same-layer via(s) need manual repair (blind/micro)",
                sl_warnings.len()
            );
        }
        return;
    }
    if !sl_fixes.is_empty() {
        let a = if dry_run { "Would repair" } else { "Repaired" };
        println!("\n{a} {} same-layer via(s):", sl_fixes.len());
        for x in sl_fixes.iter().take(5) {
            println!(
                "  Via at ({:.2}, {:.2}): layers {}/{} -> {}/{}",
                x.x, x.y, x.old_start_layer, x.old_end_layer, x.new_start_layer, x.new_end_layer
            );
        }
        if sl_fixes.len() > 5 {
            println!("  ... and {} more", sl_fixes.len() - 5);
        }
    }
    if !sl_warnings.is_empty() {
        println!(
            "\nWarning: {} same-layer via(s) need manual repair (blind/micro type, cannot auto-determine target layer):",
            sl_warnings.len()
        );
        for w in sl_warnings.iter().take(5) {
            println!(
                "  Via at ({:.2}, {:.2}): type={}, layers={}/{}",
                w.x, w.y, w.via_type, w.start_layer, w.end_layer
            );
        }
        if sl_warnings.len() > 5 {
            println!("  ... and {} more", sl_warnings.len() - 5);
        }
    }
    if fixes.is_empty() && skips.is_empty() {
        if sl_fixes.is_empty() && sl_warnings.is_empty() {
            println!("No vias needed resizing.");
        }
        return;
    }
    let action = if dry_run { "Would resize" } else { "Resizing" };
    println!("{action} vias{source} (drill: {td}mm, diameter: {tdia}mm):");
    println!("  Updated {} via(s)", fixes.len());
    let line = |x: &ViaFix| {
        println!(
            "    Via at ({:.2}, {:.2}): drill {:.3}->{:.3}mm, diameter {:.3}->{:.3}mm",
            x.x, x.y, x.old_drill, x.new_drill, x.old_diameter, x.new_diameter
        )
    };
    if fixes.len() <= 5 {
        fixes.iter().for_each(line);
    } else {
        fixes.iter().take(3).for_each(line);
        println!("    ... and {} more", fixes.len() - 3);
    }
    if !skips.is_empty() {
        println!("\n  Skipped {} via(s) (would violate clearance):", skips.len());
        for s in skips.iter().take(5) {
            println!(
                "    Via at ({:.2}, {:.2}): kept at {:.3}mm (enlarging to {:.3}mm would violate clearance: {})",
                s.x, s.y, s.current_diameter, s.would_be_diameter, s.reason
            );
        }
        if skips.len() > 5 {
            println!("    ... and {} more", skips.len() - 5);
        }
    }
    if !warnings.is_empty() {
        println!(
            "\nWarning: {} via(s) may cause DRC violations after resize:",
            warnings.len()
        );
        for w in warnings.iter().take(5) {
            println!(
                "  - Via at ({:.2}, {:.2}) - {:.2}mm clearance to {}",
                w.x, w.y, w.clearance_mm, w.nearby_item
            );
        }
        if warnings.len() > 5 {
            println!("  ... and {} more", warnings.len() - 5);
        }
    }
}

pub fn run(argv: Vec<OsString>, g: &Globals) -> Result<i32> {
    let mut args: Args = parse_args("fix-vias", argv);
    args.quiet |= g.quiet;
    // The outer parser forwards --drill/--diameter only when truthy.
    args.drill = args.drill.filter(|d| *d != 0.0);
    args.diameter = args.diameter.filter(|d| *d != 0.0);
    let pcb_str = py_path_str(&args.pcb);
    let pcb_path = Path::new(&args.pcb);
    if !pcb_path.exists() {
        eprintln!("Error: PCB file not found: {pcb_str}");
        return Ok(1);
    }
    let suffix = pcb_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    if suffix.to_lowercase() != ".kicad_pcb" {
        eprintln!("Error: Expected .kicad_pcb file, got: {suffix}");
        return Ok(1);
    }
    let layers = match args.layers {
        Some(l) => l,
        None => match crate::schema::pcb::Pcb::load(pcb_path) {
            Ok(p) => {
                let n = p.copper_layers().len() as i64;
                if n > 0 {
                    n
                } else {
                    2
                }
            }
            Err(_) => 2,
        },
    };
    if args.relocate_in_pad {
        return super::relocate_in_pad_vias::run_relocate_in_pad(
            &args.mfr,
            layers,
            args.copper,
            pcb_path,
            args.output.as_deref(),
            &args.nets,
            args.dry_run,
            args.search_alternatives,
            args.quiet,
            &args.format,
        );
    }
    let (target_drill, target_diameter, ring, clearance) =
        get_design_rules(Some(&args.mfr), layers, args.copper, args.drill, args.diameter);
    let mut doc = match crate::sexp::parse_file(pcb_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error parsing PCB file: {e}");
            return Ok(1);
        }
    };
    let (sl_fixes, sl_warnings) = fix_same_layer_vias(&mut doc, args.dry_run);
    let (fixes, warnings, skips) = fix_vias(
        &mut doc,
        target_drill,
        target_diameter,
        clearance,
        args.dry_run,
        ring,
        args.skip_if_clearance_violation,
    );
    if !args.quiet {
        print_fix_results(
            &fixes,
            &warnings,
            &args.format,
            args.dry_run,
            target_drill,
            target_diameter,
            &args.mfr,
            &skips,
            &sl_fixes,
            &sl_warnings,
        );
    }
    if (!fixes.is_empty() || !sl_fixes.is_empty()) && !args.dry_run {
        let out = args.output.clone().unwrap_or_else(|| args.pcb.clone());
        match crate::core::sexp_file::save_pcb(&doc, &out) {
            Ok(()) => {
                if !args.quiet && args.format == "text" {
                    println!("\nSaved to: {}", py_path_str(&out));
                }
            }
            Err(e) => {
                eprintln!("Error saving PCB file: {e}");
                return Ok(1);
            }
        }
    }
    Ok(if !warnings.is_empty() || !sl_warnings.is_empty() {
        2
    } else {
        0
    })
}
