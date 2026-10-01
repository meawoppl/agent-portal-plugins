//! Native plane stitching via placement.

use std::ffi::OsString;

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Parser;
use serde::Serialize;

use super::{parse_args, Globals};
use crate::schema::pcb::{Pcb, ViaOptions};

#[derive(Parser)]
#[command(about = "Add collision-screened plane stitching vias")]
struct Args {
    pcb: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long = "net")]
    nets: Vec<String>,
    #[arg(long = "via-size", default_value_t = 0.6)]
    via_size: f64,
    #[arg(long, default_value_t = 0.3)]
    drill: f64,
    #[arg(long, default_value_t = 0.25)]
    clearance: f64,
    #[arg(long, default_value_t = 5.0)]
    spacing: f64,
    #[arg(long)]
    blanket: bool,
    #[arg(long = "edge-clearance", default_value_t = 1.0)]
    edge_clearance: f64,
    #[arg(long)]
    dry_run: bool,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
}

#[derive(Serialize)]
struct Placement {
    net: String,
    x: f64,
    y: f64,
    size: f64,
    drill: f64,
}

fn clear(pcb: &Pcb, x: f64, y: f64, r: f64, clearance: f64) -> bool {
    if pcb
        .vias()
        .iter()
        .any(|v| (v.position.0 - x).hypot(v.position.1 - y) < r + v.size / 2.0 + clearance)
    {
        return false;
    }
    for fp in pcb.footprints() {
        for pad in &fp.pads {
            if let Some((px, py)) = pcb.get_pad_position(&fp.reference, &pad.number) {
                if (px - x).hypot(py - y) < r + pad.size.0.max(pad.size.1) / 2.0 + clearance {
                    return false;
                }
            }
        }
    }
    true
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("stitch", args);
    if args.via_size <= args.drill || args.drill <= 0.0 || args.spacing <= 0.0 {
        bail!("require via-size > drill > 0 and spacing > 0")
    }
    if !args.dry_run && args.output.is_none() {
        bail!("stitch is a design edit; pass --output (or use --dry-run)")
    }
    let mut pcb = Pcb::load(&args.pcb)?;
    let available: BTreeSet<_> = pcb.nets().iter().map(|n| n.name.clone()).collect();
    let nets: BTreeSet<String> = if args.nets.is_empty() {
        pcb.zones()
            .iter()
            .filter(|z| !z.net_name.is_empty())
            .map(|z| z.net_name.clone())
            .collect()
    } else {
        args.nets.iter().cloned().collect()
    };
    if nets.is_empty() {
        bail!("no plane nets detected; pass --net")
    }
    for net in &nets {
        if !available.contains(net) {
            bail!("net {net:?} not found on board")
        }
    }
    let (w, h) = pcb.board_size()?;
    if w <= 0.0 || h <= 0.0 {
        bail!("board has no usable Edge.Cuts outline")
    }
    let mut proposed = Vec::new();
    let r = args.via_size / 2.0;
    for net in &nets {
        let matching_zones: Vec<_> = pcb
            .zones()
            .iter()
            .filter(|z| z.net_name == *net && !z.polygon.is_empty())
            .collect();
        if matching_zones.is_empty() && !args.blanket {
            continue;
        }
        let mut y = args.edge_clearance;
        while y <= h - args.edge_clearance {
            let mut x = args.edge_clearance;
            while x <= w - args.edge_clearance {
                let in_zone = args.blanket
                    || matching_zones.iter().any(|z| {
                        let mut inside = false;
                        let p = &z.polygon;
                        for i in 0..p.len() {
                            let j = (i + p.len() - 1) % p.len();
                            if ((p[i].1 > y) != (p[j].1 > y))
                                && (x
                                    < (p[j].0 - p[i].0) * (y - p[i].1) / (p[j].1 - p[i].1) + p[i].0)
                            {
                                inside = !inside
                            }
                        }
                        inside
                    });
                if in_zone
                    && clear(&pcb, x, y, r, args.clearance)
                    && proposed
                        .iter()
                        .all(|p: &Placement| (p.x - x).hypot(p.y - y) >= args.spacing)
                {
                    proposed.push(Placement {
                        net: net.clone(),
                        x,
                        y,
                        size: args.via_size,
                        drill: args.drill,
                    });
                }
                x += args.spacing;
            }
            y += args.spacing;
        }
    }
    if !args.dry_run {
        for p in &proposed {
            pcb.add_via(
                p.x,
                p.y,
                ViaOptions {
                    size: p.size,
                    drill: p.drill,
                    layers: vec!["F.Cu".into(), "B.Cu".into()],
                    net: Some(p.net.clone()),
                    dedupe: true,
                },
            );
        }
        pcb.save(args.output.as_deref())?;
    }
    let document = serde_json::json!({"command":"stitch","pcb":args.pcb,"output":args.output,"nets":nets,"via_size_mm":args.via_size,"drill_mm":args.drill,"spacing_mm":args.spacing,"blanket":args.blanket,"dry_run":args.dry_run,"placed":proposed,"placed_count":proposed.len(),"success":!proposed.is_empty()});
    if args.format == "json" {
        println!("{}", serde_json::to_string_pretty(&document)?)
    } else {
        println!(
            "{} {} collision-screened stitching vias for {} net(s)",
            if args.dry_run { "Would add" } else { "Added" },
            proposed.len(),
            nets.len()
        );
    }
    Ok(if proposed.is_empty() { 1 } else { 0 })
}
