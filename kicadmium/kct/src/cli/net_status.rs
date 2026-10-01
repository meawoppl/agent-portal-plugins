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
    layers: Vec<String>,
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
                &json!({"pcb":a.pcb,"connectivity_model":if a.legacy_proximity{"legacy_proximity"}else{"strict"},"summary":{"total_nets":all.len(),"complete":all.len()-bad,"incomplete":bad,"unrouted":all.iter().filter(|v|v["status"]=="unrouted").count(),"total_unconnected_pads":all.iter().map(|v|v["unconnected_count"].as_u64().unwrap_or(0)).sum::<u64>()},"nets":shown})
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
                                layers: pad.layers.clone(),
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
                if !pad
                    .layers
                    .iter()
                    .any(|layer| layer == &s.layer || layer == "*.Cu")
                {
                    continue;
                }
                if crate::core::geometry::point_to_segment_distance(
                    pad.pos.0, pad.pos.1, s.start.0, s.start.1, s.end.0, s.end.1,
                ) <= pad.radius + s.width / 2.0 + 1e-6
                {
                    union(&mut parent, j, a)
                }
            }
            for (k, t) in segs.iter().take(i).enumerate() {
                if s.layer == t.layer
                    && crate::core::geometry::segment_to_segment_distance(
                        s.start.0, s.start.1, s.end.0, s.end.1, t.start.0, t.start.1, t.end.0,
                        t.end.1,
                    ) <= (s.width + t.width) / 2.0 + 1e-6
                {
                    union(&mut parent, a, base + k * 2);
                }
            }
        }
        let via_base = base + segs.len() * 2;
        for (vi, via) in vias.iter().enumerate() {
            let node = via_base + vi;
            for (pi, pad) in pads.iter().enumerate() {
                if layers_touch(&via.layers, &pad.layers)
                    && dist(via.position, pad.pos) <= via.size / 2.0 + pad.radius + 1e-6
                {
                    union(&mut parent, node, pi)
                }
            }
            for (si, seg) in segs.iter().enumerate() {
                if via.layers.iter().any(|layer| layer == &seg.layer)
                    && crate::core::geometry::point_to_segment_distance(
                        via.position.0,
                        via.position.1,
                        seg.start.0,
                        seg.start.1,
                        seg.end.0,
                        seg.end.1,
                    ) <= via.size / 2.0 + seg.width / 2.0 + 1e-6
                {
                    union(&mut parent, node, base + si * 2)
                }
            }
            for (oi, other) in vias.iter().take(vi).enumerate() {
                if layers_touch(&via.layers, &other.layers)
                    && dist(via.position, other.position) <= (via.size + other.size) / 2.0 + 1e-6
                {
                    union(&mut parent, node, via_base + oi)
                }
            }
        }
        for i in 0..pads.len() {
            for j in 0..i {
                if layers_touch(&pads[i].layers, &pads[j].layers)
                    && dist(pads[i].pos, pads[j].pos) <= pads[i].radius + pads[j].radius + 1e-6
                {
                    union(&mut parent, i, j)
                }
            }
        }
        let mut groups = std::collections::BTreeMap::<usize, Vec<usize>>::new();
        for i in 0..pads.len() {
            groups.entry(find(&mut parent, i)).or_default().push(i);
        }
        let islands = groups.len();
        let status = if islands <= 1 {
            "complete"
        } else {
            "incomplete"
        };
        if status != "complete" {
            bad += 1
        }
        let mut connected_indices = Vec::new();
        for group in groups.values() {
            if group.len() > connected_indices.len() {
                connected_indices = group.clone();
            }
        }
        let connected = connected_indices.len();
        let cp: Vec<_> = pads
            .iter()
            .enumerate()
            .filter(|(index, _)| connected_indices.contains(index))
            .map(|(_, p)| json!({"name":p.name,"position":[p.pos.0,p.pos.1]}))
            .collect();
        let mut up: Vec<_> = pads
            .iter()
            .enumerate()
            .filter(|(index, _)| !connected_indices.contains(index))
            .map(|(_, p)| json!({"name":p.name,"position":[p.pos.0,p.pos.1]}))
            .collect();
        up.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        let plane_layers = p
            .zones()
            .iter()
            .filter(|zone| zone.net_number == net.number)
            .map(|zone| zone.layers.first().cloned().unwrap_or_default())
            .collect::<Vec<_>>();
        let filled = p
            .zones()
            .iter()
            .any(|zone| zone.net_number == net.number && !zone.filled_polygons.is_empty());
        let power = is_power_net(&net.name);
        let unconnected = pads.len() - connected;
        out.push(json!({"net_number":net.number,"net_name":net.name,"net_class":"","status":status,"net_type":if power{"power"}else{"signal"},"total_pads":pads.len(),"connected_count":connected,"unconnected_count":unconnected,"connection_percentage":round1(connected as f64/pads.len() as f64*100.),"island_count":islands,"total_connections":pads.len().saturating_sub(1),"routed_connections":connected.saturating_sub(1),"open_connections":unconnected,"is_plane_net":!plane_layers.is_empty(),"has_filled_zone":filled,"is_advisory_incomplete":status=="incomplete"&&power,"plane_layer":plane_layers.first().cloned().unwrap_or_default(),"plane_layers":plane_layers,"has_routing":!segs.is_empty(),"has_vias":!vias.is_empty(),"suggested_fix":if unconnected>0{format!("Route traces to connect {unconnected} pads")}else{String::new()},"connected_pads":cp,"unconnected_pads":up}));
    }
    let order = |status: &str| match status {
        "incomplete" => 0,
        "unrouted" => 1,
        "complete" => 2,
        _ => 3,
    };
    out.sort_by(|a, b| {
        (
            order(a["status"].as_str().unwrap_or("")),
            a["net_name"].as_str().unwrap_or(""),
        )
            .cmp(&(
                order(b["status"].as_str().unwrap_or("")),
                b["net_name"].as_str().unwrap_or(""),
            ))
    });
    (out, bad)
}
fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}
fn is_power_net(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper == "GND"
        || upper.starts_with("GND")
        || upper.starts_with('V')
        || ["POWER", "PWR", "VBAT", "VIN", "VOUT"]
            .iter()
            .any(|part| upper.contains(part))
}
fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}
fn layers_touch(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| {
        b.iter().any(|y| {
            x == y || x == "*.Cu" && y.ends_with(".Cu") || y == "*.Cu" && x.ends_with(".Cu")
        })
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distance() {
        assert_eq!(dist((0., 0.), (3., 4.)), 5.);
    }
}
