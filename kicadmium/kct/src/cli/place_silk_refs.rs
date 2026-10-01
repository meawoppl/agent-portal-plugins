//! Collision-screened silkscreen reference placement.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Serialize;

use super::{parse_args, Globals};
use crate::core::geometry::rotate_pad_offset;
use crate::schema::pcb::Pcb;
use crate::sexp::SExp;

#[derive(Parser)]
#[command(about = "Move visible silkscreen references clear of pads, vias, text, and edges")]
struct Args {
    pcb: PathBuf,
    #[arg(long)]
    mfr: Option<String>,
    #[arg(long, default_value_t = 2)]
    layers: u8,
    #[arg(long, default_value_t = 1.0)]
    copper: f64,
    #[arg(long, default_value_t = 0.2)]
    clearance: f64,
    #[arg(long = "edge-clearance", default_value_t = 0.5)]
    edge_clearance: f64,
    #[arg(long = "max-offset", default_value_t = 5.0)]
    max_offset: f64,
    #[arg(long, default_value_t = 0.25)]
    step: f64,
    #[arg(long = "allow-rotate")]
    allow_rotate: bool,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long = "verify-drc")]
    verify_drc: bool,
    #[arg(long)]
    render: Option<PathBuf>,
    #[arg(long, default_value = "text", value_parser = ["text", "json", "summary"])]
    format: String,
}

#[derive(Clone, Copy)]
struct Rect {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}
impl Rect {
    fn expanded(self, d: f64) -> Self {
        Self {
            x1: self.x1 - d,
            y1: self.y1 - d,
            x2: self.x2 + d,
            y2: self.y2 + d,
        }
    }
    fn overlaps(self, o: Self) -> bool {
        self.x1 < o.x2 && self.x2 > o.x1 && self.y1 < o.y2 && self.y2 > o.y1
    }
}

#[derive(Serialize)]
struct Placement {
    footprint_ref: String,
    old_position: (f64, f64),
    new_position: (f64, f64),
    old_rotation: f64,
    new_rotation: f64,
    moved: bool,
    status: &'static str,
    reason: String,
}

fn global(fp: (f64, f64), local: (f64, f64), rotation: f64) -> (f64, f64) {
    let d = rotate_pad_offset(local.0, local.1, rotation);
    (fp.0 + d.0, fp.1 + d.1)
}
fn local(fp: (f64, f64), point: (f64, f64), rotation: f64) -> (f64, f64) {
    rotate_pad_offset(point.0 - fp.0, point.1 - fp.1, -rotation)
}
fn text_rect(center: (f64, f64), chars: usize, font: (f64, f64), rotation: f64) -> Rect {
    let mut w = font.0 * (chars.max(1) as f64) * 0.65;
    let mut h = font.1;
    if ((rotation / 90.0).round() as i64).rem_euclid(2) == 1 {
        std::mem::swap(&mut w, &mut h)
    }
    Rect {
        x1: center.0 - w / 2.0,
        y1: center.1 - h / 2.0,
        x2: center.0 + w / 2.0,
        y2: center.1 + h / 2.0,
    }
}

fn raw_reference(node: &SExp) -> Option<&str> {
    node.children_named("property")
        .find(|p| p.string_at(0) == Some("Reference"))
        .and_then(|p| p.string_at(1))
        .or_else(|| {
            node.children_named("fp_text")
                .find(|p| p.string_at(0) == Some("reference"))
                .and_then(|p| p.string_at(1))
        })
}
fn set_reference_rotation(root: &mut SExp, reference: &str, rotation: f64) {
    for fp in &mut root.children {
        if !matches!(fp.tag(), Some("footprint" | "module")) || raw_reference(fp) != Some(reference)
        {
            continue;
        }
        for text in &mut fp.children {
            let is_ref = (text.has_tag("fp_text") && text.string_at(0) == Some("reference"))
                || (text.has_tag("property") && text.string_at(0) == Some("Reference"));
            if is_ref {
                if let Some(at) = text.get_mut("at") {
                    at.set_value(2, rotation)
                }
            }
        }
        break;
    }
}

fn verify_drc(path: &Path) -> serde_json::Value {
    let mut report = std::env::temp_dir();
    report.push(format!(
        "kicadmium-silk-drc-{}.json",
        crate::schema::pcb::util::new_uuid()
    ));
    let cli = std::env::var_os("KICADMIUM_KICAD_CLI")
        .or_else(|| std::env::var_os("KICAD_CLI"))
        .unwrap_or_else(|| "kicad-cli".into());
    let result = std::process::Command::new(cli)
        .args(["pcb", "drc", "--format", "json", "--output"])
        .arg(&report)
        .arg(path)
        .output();
    let document = match result {
        Err(e) => serde_json::json!({"available":false,"error":e.to_string()}),
        Ok(out) if !report.exists() => {
            serde_json::json!({"available":true,"error":String::from_utf8_lossy(&out.stderr)})
        }
        Ok(_) => match std::fs::read_to_string(&report)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            None => serde_json::json!({"available":true,"error":"invalid native DRC JSON"}),
            Some(v) => {
                let all = v
                    .get("violations")
                    .and_then(|x| x.as_array())
                    .cloned()
                    .unwrap_or_default();
                let silk = all
                    .iter()
                    .filter(|x| {
                        x.get("type")
                            .and_then(|t| t.as_str())
                            .is_some_and(|t| t.contains("silk"))
                    })
                    .count();
                serde_json::json!({"available":true,"total_violations":all.len(),"silk_violations":silk})
            }
        },
    };
    let _ = std::fs::remove_file(report);
    document
}

fn render_svg(path: &Path, w: f64, h: f64, placements: &[Placement]) -> Result<()> {
    let scale = (900.0 / w.max(h).max(1.0)).min(20.0);
    let mut svg=format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {w} {h}\"><rect width=\"100%\" height=\"100%\" fill=\"#1a1b26\"/><rect x=\"0.1\" y=\"0.1\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"#565f89\" stroke-width=\"{}\"/>",w*scale,h*scale,(w-0.2).max(0.0),(h-0.2).max(0.0),1.0/scale);
    for p in placements {
        svg.push_str(&format!("<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#e0af68\" stroke-width=\"{}\"/><circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#f7768e\"/><circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#9ece6a\"/><text x=\"{}\" y=\"{}\" fill=\"#c0caf5\" font-size=\"{}\">{}</text>",p.old_position.0,p.old_position.1,p.new_position.0,p.new_position.1,1.0/scale,p.old_position.0,p.old_position.1,2.0/scale,p.new_position.0,p.new_position.1,2.0/scale,p.new_position.0+2.0/scale,p.new_position.1,9.0/scale,p.footprint_ref));
    }
    svg.push_str("</svg>");
    std::fs::write(path, svg).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("place-silk-refs", args);
    if args.pcb.extension().and_then(|s| s.to_str()) != Some("kicad_pcb") {
        bail!("expected a .kicad_pcb file")
    };
    if args.step <= 0.0
        || args.max_offset < 0.0
        || args.clearance < 0.0
        || args.edge_clearance < 0.0
    {
        bail!("step must be positive and distances non-negative")
    };
    if args.max_offset / args.step > 4096.0 {
        bail!("search would exceed 4096 rings; increase --step or reduce --max-offset")
    };
    if !args.dry_run && args.output.is_none() {
        bail!("place-silk-refs is a design edit; pass --output (or use --dry-run)")
    }
    let mask_clearance = match &args.mfr {
        Some(m) => crate::manufacturers::rules(m, args.layers, args.copper)
            .map(|r| r.min_solder_mask_clearance_mm)
            .unwrap_or(0.05),
        None => 0.05,
    };
    let mut pcb = Pcb::load(&args.pcb)?;
    let (w, h) = pcb.board_size()?;
    if w <= 0.0 || h <= 0.0 {
        bail!("board has no usable Edge.Cuts outline")
    }
    let mut obstacles = Vec::new();
    for fp in pcb.footprints() {
        for pad in &fp.pads {
            if let Some(p) = pcb.get_pad_position(&fp.reference, &pad.number) {
                obstacles.push(text_rect(
                    p,
                    1,
                    (
                        pad.size.0 + 2.0 * mask_clearance,
                        pad.size.1 + 2.0 * mask_clearance,
                    ),
                    pad.rotation,
                ))
            }
        }
        for text in &fp.texts {
            if !text.hidden && text.text_type != "reference" && text.layer.ends_with("SilkS") {
                obstacles.push(text_rect(
                    global(fp.position, text.position, fp.rotation),
                    text.text.chars().count(),
                    text.font_size,
                    text.rotation,
                ))
            }
        }
    }
    for via in pcb.vias() {
        obstacles.push(Rect {
            x1: via.position.0 - via.size / 2.0 - mask_clearance,
            y1: via.position.1 - via.size / 2.0 - mask_clearance,
            x2: via.position.0 + via.size / 2.0 + mask_clearance,
            y2: via.position.1 + via.size / 2.0 + mask_clearance,
        })
    }
    let refs: Vec<_> = pcb
        .footprints()
        .iter()
        .filter_map(|fp| {
            fp.texts
                .iter()
                .find(|t| t.text_type == "reference" && !t.hidden && t.layer.ends_with("SilkS"))
                .map(|t| (fp.reference.clone(), fp.position, fp.rotation, t.clone()))
        })
        .collect();
    let mut placements = Vec::new();
    let dirs = [
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (1.0, 1.0),
        (1.0, -1.0),
        (-1.0, 1.0),
        (-1.0, -1.0),
    ];
    for (reference, fp_pos, fp_rotation, text) in refs {
        let old = global(fp_pos, text.position, fp_rotation);
        let mut chosen = None;
        let rings = (args.max_offset / args.step).floor() as usize;
        for ring in 0..=rings {
            let candidates: Vec<_> = if ring == 0 {
                vec![(old, text.rotation)]
            } else {
                dirs.iter()
                    .flat_map(|d| {
                        let p = (
                            fp_pos.0 + d.0 * ring as f64 * args.step,
                            fp_pos.1 + d.1 * ring as f64 * args.step,
                        );
                        if args.allow_rotate {
                            vec![(p, text.rotation), (p, text.rotation + 90.0)]
                        } else {
                            vec![(p, text.rotation)]
                        }
                    })
                    .collect()
            };
            for (center, rotation) in candidates {
                let rect = text_rect(center, reference.chars().count(), text.font_size, rotation);
                let in_board = rect.x1 >= args.edge_clearance
                    && rect.y1 >= args.edge_clearance
                    && rect.x2 <= w - args.edge_clearance
                    && rect.y2 <= h - args.edge_clearance;
                let clear = in_board
                    && !obstacles
                        .iter()
                        .any(|o| rect.overlaps(o.expanded(args.clearance)));
                if clear {
                    chosen = Some((center, rotation));
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }
        match chosen {
            Some((new, rotation)) => {
                let moved = (new.0 - old.0).abs() > 1e-9
                    || (new.1 - old.1).abs() > 1e-9
                    || (rotation - text.rotation).abs() > 1e-9;
                if moved && !args.dry_run {
                    let local_pos = local(fp_pos, new, fp_rotation);
                    pcb.move_reference(&reference, (0.0, 0.0), Some(local_pos), None);
                    set_reference_rotation(pcb.sexp_mut(), &reference, rotation);
                }
                if moved {
                    obstacles.push(text_rect(
                        new,
                        reference.chars().count(),
                        text.font_size,
                        rotation,
                    ));
                }
                placements.push(Placement {
                    footprint_ref: reference,
                    old_position: old,
                    new_position: new,
                    old_rotation: text.rotation,
                    new_rotation: rotation,
                    moved,
                    status: if moved { "moved" } else { "unchanged" },
                    reason: if moved {
                        "nearest collision-free search position".into()
                    } else {
                        "already clear".into()
                    },
                })
            }
            None => placements.push(Placement {
                footprint_ref: reference,
                old_position: old,
                new_position: old,
                old_rotation: text.rotation,
                new_rotation: text.rotation,
                moved: false,
                status: "unplaceable",
                reason: "no collision-free location inside search radius".into(),
            }),
        }
    }
    if !args.dry_run {
        pcb.save(args.output.as_deref())?
    }
    if let Some(path) = &args.render {
        render_svg(path, w, h, &placements)?
    }
    let moved = placements.iter().filter(|p| p.moved).count();
    let unplaceable = placements
        .iter()
        .filter(|p| p.status == "unplaceable")
        .count();
    let drc = if args.verify_drc && !args.dry_run {
        Some(verify_drc(args.output.as_ref().unwrap()))
    } else {
        None
    };
    let drc_ok = drc.as_ref().is_none_or(|d| {
        d.get("available").and_then(|v| v.as_bool()) == Some(true)
            && d.get("error").is_none()
            && d.get("silk_violations").and_then(|v| v.as_u64()) == Some(0)
    });
    let document = serde_json::json!({"command":"place-silk-refs","pcb":args.pcb,"output":args.output,"clearance_mm":args.clearance,"mask_clearance_mm":mask_clearance,"dry_run":args.dry_run,"total_moved":moved,"total_unchanged":placements.len()-moved-unplaceable,"total_unplaceable":unplaceable,"placements":placements,"drc_verification":drc,"render_artifact":args.render,"success":drc_ok});
    if args.format == "json" {
        println!("{}", serde_json::to_string_pretty(&document)?)
    } else if args.format == "summary" {
        println!(
            "{} {moved} reference(s); {unplaceable} unplaceable (clearance: {:.2}mm)",
            if args.dry_run { "Would move" } else { "Moved" },
            args.clearance
        )
    } else if placements.is_empty() {
        println!("No visible reference designators found.")
    } else {
        println!(
            "{} {moved} reference designator(s); {unplaceable} unplaceable",
            if args.dry_run { "Would move" } else { "Moved" }
        );
        for p in placements.iter().filter(|p| p.moved) {
            println!(
                "  {}: ({:.3}, {:.3}) -> ({:.3}, {:.3})",
                p.footprint_ref,
                p.old_position.0,
                p.old_position.1,
                p.new_position.0,
                p.new_position.1
            )
        }
        if !args.dry_run && moved > 0 {
            println!("Saved to: {}", args.output.as_ref().unwrap().display())
        }
    }
    Ok(if drc_ok { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rectangle_overlap_is_strict() {
        let a = Rect {
            x1: 0.,
            y1: 0.,
            x2: 1.,
            y2: 1.,
        };
        assert!(a.overlaps(Rect {
            x1: 0.5,
            y1: 0.5,
            x2: 2.,
            y2: 2.
        }));
        assert!(!a.overlaps(Rect {
            x1: 1.,
            y1: 0.,
            x2: 2.,
            y2: 1.
        }));
    }
    #[test]
    fn transforms_round_trip() {
        let p = (3., 7.);
        let g = global((10., 20.), p, 90.);
        let q = local((10., 20.), g, 90.);
        assert!((q.0 - p.0).abs() < 1e-9 && (q.1 - p.1).abs() < 1e-9);
    }
}
