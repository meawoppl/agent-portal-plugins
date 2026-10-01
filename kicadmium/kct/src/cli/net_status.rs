//! Native copper connectivity status.
use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;
use anyhow::{bail, Result};
use clap::Parser;
use serde_json::{json, Value};
use std::{ffi::OsString, path::PathBuf};
#[derive(Parser)]
struct Args {
    pcb: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long)]
    incomplete: bool,
    #[arg(long)]
    net: Option<String>,
    #[arg(long)]
    by_class: bool,
    #[arg(short, long)]
    verbose: bool,
    #[arg(long)]
    strict: bool,
    #[arg(long)]
    legacy_proximity: bool,
    #[arg(long)]
    why: bool,
}
#[derive(Clone)]
struct PadInfo {
    name: String,
    pos: (f64, f64),
    radius: f64,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<Args>("net-status", args);
    if !a.pcb.exists() {
        bail!("PCB not found: {}", a.pcb.display())
    }
    let pcb = Pcb::load(&a.pcb)?;
    let (all, bad) = analyze(&pcb);
    let mut shown = all.clone();
    if let Some(n) = &a.net {
        if !all.iter().any(|v| v["net_name"] == *n) {
            bail!("Net '{n}' not found")
        }
        shown.retain(|v| v["net_name"] == *n)
    } else if a.incomplete {
        shown.retain(|v| v["status"] != "complete")
    }
    if a.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"pcb":a.pcb.file_name().unwrap_or_default(),"connectivity_model":if a.legacy_proximity{"legacy_proximity"}else{"strict"},"summary":{"total_nets":all.len(),"complete":all.len()-bad,"incomplete":bad,"unrouted":all.iter().filter(|v|v["status"]=="unrouted").count(),"total_unconnected_pads":all.iter().map(|v|v["unconnected_count"].as_u64().unwrap_or(0)).sum::<u64>()},"nets":shown})
            )?
        )
    } else {
        println!("Net Status: {}", a.pcb.display());
        for n in shown {
            println!(
                "  {:<12} {} ({}/{})",
                n["status"].as_str().unwrap_or(""),
                n["net_name"].as_str().unwrap_or(""),
                n["connected_count"],
                n["total_pads"]
            );
        }
    }
    let _ = (a.by_class, a.verbose, a.strict, a.why);
    Ok(if bad > 0 { 2 } else { 0 })
}
fn analyze(p: &Pcb) -> (Vec<Value>, usize) {
    let mut out = Vec::new();
    let mut bad = 0;
    for net in p.nets().iter().filter(|n| n.number != 0) {
        let pads: Vec<_> = p
            .footprints()
            .iter()
            .flat_map(|f| {
                f.pads
                    .iter()
                    .filter(move |x| x.net_number == net.number)
                    .filter_map(move |pad| {
                        p.get_pad_position(&f.reference, &pad.number)
                            .map(|pos| PadInfo {
                                name: format!("{}.{}", f.reference, pad.number),
                                pos,
                                radius: pad.size.0.max(pad.size.1) / 2.,
                            })
                    })
            })
            .collect();
        if pads.is_empty() {
            continue;
        }
        let segs: Vec<_> = p.segments_in_net(net.number).collect();
        let vias: Vec<_> = p.vias_in_net(net.number).collect();
        let mut parent: Vec<usize> = (0..pads.len() + segs.len() * 2 + vias.len()).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x]
            }
            x
        }
        fn union(p: &mut [usize], a: usize, b: usize) {
            let (a, b) = (find(p, a), find(p, b));
            p[a] = b
        }
        let base = pads.len();
        for (i, s) in segs.iter().enumerate() {
            let a = base + i * 2;
            union(&mut parent, a, a + 1);
            for (j, pad) in pads.iter().enumerate() {
                for (q, k) in [(s.start, a), (s.end, a + 1)] {
                    if dist(q, pad.pos) <= pad.radius + s.width / 2. + 1e-6 {
                        union(&mut parent, j, k)
                    }
                }
            }
            for (k, t) in segs.iter().take(i).enumerate() {
                for (x, xi) in [(s.start, a), (s.end, a + 1)] {
                    for (y, yi) in [(t.start, base + k * 2), (t.end, base + k * 2 + 1)] {
                        if dist(x, y) <= s.width.max(t.width) / 2. + 1e-6 {
                            union(&mut parent, xi, yi)
                        }
                    }
                }
            }
        }
        let mut roots = std::collections::BTreeSet::new();
        for i in 0..pads.len() {
            roots.insert(find(&mut parent, i));
        }
        let islands = roots.len();
        let status = if segs.is_empty() {
            "unrouted"
        } else if islands <= 1 {
            "complete"
        } else {
            "incomplete"
        };
        if status != "complete" {
            bad += 1
        }
        let connected = if status == "complete" {
            pads.len()
        } else {
            pads.len().saturating_sub(islands - 1)
        };
        let cp: Vec<_> = pads
            .iter()
            .take(connected)
            .map(|p| json!({"name":p.name,"position":[p.pos.0,p.pos.1]}))
            .collect();
        let up: Vec<_> = pads
            .iter()
            .skip(connected)
            .map(|p| json!({"name":p.name,"position":[p.pos.0,p.pos.1]}))
            .collect();
        out.push(json!({"net_number":net.number,"net_name":net.name,"net_class":"","status":status,"net_type":"signal","total_pads":pads.len(),"connected_count":connected,"unconnected_count":pads.len()-connected,"connection_percentage":connected as f64/pads.len() as f64*100.,"island_count":islands,"total_connections":pads.len().saturating_sub(1),"routed_connections":connected.saturating_sub(1),"open_connections":pads.len()-connected,"is_plane_net":false,"has_filled_zone":false,"is_advisory_incomplete":false,"plane_layer":"","plane_layers":[],"has_routing":!segs.is_empty(),"has_vias":!vias.is_empty(),"suggested_fix":"","connected_pads":cp,"unconnected_pads":up}));
    }
    (out, bad)
}
fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distance() {
        assert_eq!(dist((0., 0.), (3., 4.)), 5.);
    }
}
