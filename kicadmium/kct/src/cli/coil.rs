//! Planar-coil tools: checks KiCad DRC cannot do for a winding built from
//! tracks, arcs and vias of a single net.
//!
//! `coil spacing` measures copper-to-copper gaps *within* one net (DRC only
//! checks between nets, so a shorted turn is invisible to it). Copper is
//! sampled along each connected chain of tracks and arcs; two samples count
//! as a gap unless they sit on the same chain within `--path-separation` of
//! each other along the path (that is the track's own neighbourhood: a bend
//! or an arc turning back on itself). Vias are measured against same-net
//! copper too, excluding the stretch of chain that actually lands on them.
//!
//! `coil resistance` sums DC resistance per net from track length, width
//! and copper thickness (board stackup when present, else a default), plus
//! via barrels, at a given temperature.

use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;

/// Copper resistivity at 20 °C, Ω·m.
const RHO_CU_20: f64 = 1.68e-8;
/// Temperature coefficient of copper, 1/K.
const ALPHA_CU: f64 = 0.00393;
/// Endpoints closer than this (mm) are the same node when chaining copper.
const JOIN_MM: f64 = 1e-3;
/// Sagitta tolerance for sampling arcs (mm).
const ARC_ERROR_MM: f64 = 0.005;
/// Chain length (mm) either side of a via landing that is the via's own stub.
const VIA_STUB_MM: f64 = 1.5;

#[derive(Parser)]
#[command(about = "Planar coil checks: same-net spacing and per-net resistance")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Minimum copper-to-copper gap inside each net (shorted turns, via to turn).
    Spacing(SpacingArgs),
    /// DC resistance per net from tracks, arcs, copper thickness and vias.
    Resistance(ResistanceArgs),
}

#[derive(Parser)]
struct SpacingArgs {
    pcb: PathBuf,
    /// Nets to check (repeatable); default: every net with copper on more than one chain or a via.
    #[arg(long = "net")]
    nets: Vec<String>,
    /// Gap below which a pair is reported as a violation (mm).
    #[arg(long, default_value_t = 0.2)]
    min_gap: f64,
    /// Same-chain samples closer than this along the path are neighbours, not a gap (mm).
    #[arg(long, default_value_t = 2.5)]
    path_separation: f64,
    /// Sampling step along copper (mm).
    #[arg(long, default_value_t = 0.05)]
    step: f64,
    /// How many worst locations to list per net.
    #[arg(long, default_value_t = 5)]
    worst: usize,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
    /// Write the JSON report here as well.
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Parser)]
struct ResistanceArgs {
    pcb: PathBuf,
    #[arg(long = "net")]
    nets: Vec<String>,
    /// Copper temperature, °C.
    #[arg(long, default_value_t = 20.0)]
    temp: f64,
    /// Copper thickness (µm) for layers the stackup does not describe (35 µm = 1 oz).
    #[arg(long, default_value_t = 35.0)]
    copper_um: f64,
    /// Per-layer copper thickness override, e.g. `In1.Cu=17.5` (repeatable).
    #[arg(long = "copper-um-layer", value_name = "LAYER=UM")]
    copper_um_layer: Vec<String>,
    /// Via barrel plating thickness (µm).
    #[arg(long, default_value_t = 25.0)]
    plating_um: f64,
    /// Board thickness (mm); default from the board's `(general (thickness ..))`, else 1.6.
    #[arg(long)]
    board_thickness: Option<f64>,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
    #[arg(short, long)]
    output: Option<PathBuf>,
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("coil", args);
    match args.command {
        Command::Spacing(a) => spacing(a),
        Command::Resistance(a) => resistance(a),
    }
}

/// One sampled point of copper centreline.
#[derive(Clone, Copy)]
struct Sample {
    x: f64,
    y: f64,
    /// Position along the chain (mm).
    s: f64,
    chain: usize,
    width: f64,
}

/// A track or arc as a polyline with its width.
struct Prim {
    points: Vec<(f64, f64)>,
    width: f64,
    length: f64,
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Copper primitives of one net on one layer as polylines.
fn prims_for(pcb: &Pcb, net: i64, layer: &str) -> Result<Vec<Prim>> {
    let mut out = Vec::new();
    for s in pcb.segments_in_net(net).filter(|s| s.layer == layer) {
        out.push(Prim {
            points: vec![s.start, s.end],
            width: s.width,
            length: s.length(),
        });
    }
    for a in pcb.arcs_in_net(net).filter(|a| a.layer == layer) {
        let points = a
            .centerline_points(ARC_ERROR_MM)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        out.push(Prim {
            points,
            width: a.width,
            length: a.length(),
        });
    }
    Ok(out)
}

/// Group primitives into chains by shared endpoints and sample each chain
/// with a running path position. Branches are walked depth-first, so the
/// position is exact for a simple path (a coil) and a reasonable tree
/// distance otherwise.
fn sample_chains(prims: &[Prim], step: f64) -> Vec<Sample> {
    let key = |p: (f64, f64)| {
        (
            (p.0 / JOIN_MM).round() as i64,
            (p.1 / JOIN_MM).round() as i64,
        )
    };
    let mut nodes: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, p) in prims.iter().enumerate() {
        nodes
            .entry(key(*p.points.first().unwrap()))
            .or_default()
            .push(i);
        nodes
            .entry(key(*p.points.last().unwrap()))
            .or_default()
            .push(i);
    }
    let mut visited = vec![false; prims.len()];
    let mut samples = Vec::new();
    let mut chain = 0;
    // Start chains at free ends first so a simple path is walked end to end.
    let mut order: Vec<usize> = (0..prims.len()).collect();
    order.sort_by_key(|&i| {
        let p = &prims[i];
        let ends = nodes[&key(*p.points.first().unwrap())]
            .len()
            .min(nodes[&key(*p.points.last().unwrap())].len());
        (ends, i)
    });
    for start in order {
        if visited[start] {
            continue;
        }
        // (primitive, entry node is its first point?, path position at entry)
        let first_end = nodes[&key(*prims[start].points.first().unwrap())].len();
        let last_end = nodes[&key(*prims[start].points.last().unwrap())].len();
        let mut stack = vec![(start, first_end <= last_end, 0.0_f64)];
        while let Some((i, forward, s0)) = stack.pop() {
            if visited[i] {
                continue;
            }
            visited[i] = true;
            let p = &prims[i];
            let mut pts = p.points.clone();
            if !forward {
                pts.reverse();
            }
            let mut s = s0;
            let mut carry = 0.0;
            samples.push(Sample {
                x: pts[0].0,
                y: pts[0].1,
                s,
                chain,
                width: p.width,
            });
            for w in pts.windows(2) {
                let seg = dist(w[0], w[1]);
                let mut t = step - carry;
                while t < seg {
                    let f = t / seg;
                    samples.push(Sample {
                        x: w[0].0 + (w[1].0 - w[0].0) * f,
                        y: w[0].1 + (w[1].1 - w[0].1) * f,
                        s: s + t,
                        chain,
                        width: p.width,
                    });
                    t += step;
                }
                carry = seg - (t - step);
                s += seg;
            }
            let exit = *pts.last().unwrap();
            samples.push(Sample {
                x: exit.0,
                y: exit.1,
                s,
                chain,
                width: p.width,
            });
            for &j in &nodes[&key(exit)] {
                if !visited[j] {
                    let q = &prims[j];
                    let forward = key(*q.points.first().unwrap()) == key(exit);
                    stack.push((j, forward, s));
                }
            }
        }
        chain += 1;
    }
    samples
}

/// How far past the limit same-net gaps are measured (mm).
fn search_beyond(min_gap: f64) -> f64 {
    (3.0 * min_gap).max(1.0)
}

#[derive(Clone, Copy)]
struct Gap {
    gap: f64,
    at: (f64, f64),
}

/// Minimum gap between samples that are not path neighbours, and every
/// pair below `min_gap` (deduplicated to the worst per location cell).
fn same_net_gaps(
    samples: &[Sample],
    min_gap: f64,
    path_separation: f64,
) -> (Option<Gap>, Vec<Gap>) {
    let max_w = samples.iter().map(|s| s.width).fold(0.0, f64::max);
    // Search well beyond the limit so the reported minimum is the real one,
    // not just "nothing closer than the limit".
    let cutoff = max_w + search_beyond(min_gap);
    let cell = cutoff.max(1e-3);
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let key = |x: f64, y: f64| ((x / cell).floor() as i64, (y / cell).floor() as i64);
    for (i, s) in samples.iter().enumerate() {
        grid.entry(key(s.x, s.y)).or_default().push(i);
    }
    let mut best: Option<Gap> = None;
    let mut bad: HashMap<(i64, i64), Gap> = HashMap::new();
    for (i, a) in samples.iter().enumerate() {
        let (cx, cy) = key(a.x, a.y);
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(list) = grid.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &j in list {
                    if j <= i {
                        continue;
                    }
                    let b = &samples[j];
                    if a.chain == b.chain && (a.s - b.s).abs() < path_separation {
                        continue;
                    }
                    let d = (a.x - b.x).hypot(a.y - b.y);
                    if d > cutoff {
                        continue;
                    }
                    let gap = d - (a.width + b.width) / 2.0;
                    let at = ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                    if best.is_none_or(|g| gap < g.gap) {
                        best = Some(Gap { gap, at });
                    }
                    if gap < min_gap {
                        let k = ((at.0 / 0.5).round() as i64, (at.1 / 0.5).round() as i64);
                        let e = bad.entry(k).or_insert(Gap { gap, at });
                        if gap < e.gap {
                            *e = Gap { gap, at };
                        }
                    }
                }
            }
        }
    }
    let mut worst: Vec<Gap> = bad.into_values().collect();
    worst.sort_by(|a, b| a.gap.total_cmp(&b.gap));
    (best, worst)
}

/// Copper stack order: F.Cu, In1.Cu … InN.Cu, B.Cu.
fn copper_rank(name: &str) -> i64 {
    match name {
        "F.Cu" => 0,
        "B.Cu" => i64::MAX / 2,
        n => n
            .strip_prefix("In")
            .and_then(|r| r.strip_suffix(".Cu"))
            .and_then(|d| d.parse::<i64>().ok())
            .unwrap_or(i64::MAX / 4),
    }
}

/// Layers a via's barrel passes through, by copper stack order.
fn via_layers<'a>(via_layers: &[String], copper: &'a [String]) -> Vec<&'a str> {
    let (mut lo, mut hi) = (i64::MIN, i64::MAX);
    if via_layers.len() == 2 {
        let a = copper_rank(&via_layers[0]);
        let b = copper_rank(&via_layers[1]);
        lo = a.min(b);
        hi = a.max(b);
    }
    copper
        .iter()
        .filter(|l| (lo..=hi).contains(&copper_rank(l)))
        .map(String::as_str)
        .collect()
}

fn net_name(pcb: &Pcb, number: i64) -> String {
    pcb.nets()
        .iter()
        .find(|n| n.number == number)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| number.to_string())
}

/// Nets to analyse: named ones, else every net that has copper.
fn select_nets(pcb: &Pcb, wanted: &[String]) -> Result<Vec<(i64, String)>> {
    if !wanted.is_empty() {
        let mut out = Vec::new();
        for w in wanted {
            let net = pcb
                .nets()
                .iter()
                .find(|n| &n.name == w)
                .with_context(|| format!("net {w:?} not found on the board"))?;
            out.push((net.number, net.name.clone()));
        }
        return Ok(out);
    }
    let mut numbers: Vec<i64> = pcb
        .segments()
        .iter()
        .map(|s| s.net_number)
        .chain(pcb.arcs().iter().map(|a| a.net_number))
        .filter(|n| *n > 0)
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    Ok(numbers.into_iter().map(|n| (n, net_name(pcb, n))).collect())
}

fn copper_layer_names(pcb: &Pcb) -> Vec<String> {
    let mut names: Vec<String> = pcb.copper_layers().iter().map(|l| l.name.clone()).collect();
    names.sort_by_key(|n| copper_rank(n));
    names
}

fn write_report(output: Option<&Path>, format: &str, report: &Value, text: &str) -> Result<()> {
    if let Some(path) = output {
        crate::fsutil::atomic_write(path, &serde_json::to_vec_pretty(report)?)?;
    }
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print!("{text}");
    }
    Ok(())
}

fn spacing(a: SpacingArgs) -> Result<i32> {
    if a.min_gap < 0.0 || a.step <= 0.0 || a.path_separation < 0.0 {
        bail!("require min-gap >= 0, step > 0 and path-separation >= 0")
    }
    let pcb = Pcb::load(&a.pcb)?;
    let copper = copper_layer_names(&pcb);
    let mut nets_json = Vec::new();
    let mut text = String::new();
    let mut all_ok = true;
    for (number, name) in select_nets(&pcb, &a.nets)? {
        let mut layers_json = Vec::new();
        let mut net_min: Option<Gap> = None;
        let mut net_ok = true;
        let mut by_layer: BTreeMap<String, Vec<Sample>> = BTreeMap::new();
        for layer in &copper {
            let prims = prims_for(&pcb, number, layer)?;
            if prims.is_empty() {
                continue;
            }
            let samples = sample_chains(&prims, a.step);
            let chains = samples.iter().map(|s| s.chain).max().map_or(0, |c| c + 1);
            let (best, worst) = same_net_gaps(&samples, a.min_gap, a.path_separation);
            let violations = worst.len();
            if violations > 0 {
                net_ok = false;
            }
            if let Some(g) = best {
                if net_min.is_none_or(|m| g.gap < m.gap) {
                    net_min = Some(g);
                }
            }
            layers_json.push(json!({
                "layer": layer,
                "chains": chains,
                "samples": samples.len(),
                "min_gap_mm": best.map(|g| g.gap),
                "min_gap_at": best.map(|g| [g.at.0, g.at.1]),
                "violations": violations,
                "worst": worst.iter().take(a.worst).map(|g| json!({"gap_mm": g.gap, "at": [g.at.0, g.at.1]})).collect::<Vec<_>>(),
            }));
            by_layer.insert(layer.clone(), samples);
        }
        // Vias against same-net copper they do not terminate.
        let mut via_min: Option<Gap> = None;
        let mut via_violations = 0usize;
        for via in pcb.vias_in_net(number) {
            for layer in via_layers(&via.layers, &copper) {
                let Some(samples) = by_layer.get(layer) else {
                    continue;
                };
                // Chains landing on this via: exclude their stub either side.
                let landings: Vec<(usize, f64)> = samples
                    .iter()
                    .filter(|s| dist((s.x, s.y), via.position) < JOIN_MM)
                    .map(|s| (s.chain, s.s))
                    .collect();
                for s in samples {
                    if landings
                        .iter()
                        .any(|&(c, s0)| c == s.chain && (s.s - s0).abs() < VIA_STUB_MM)
                    {
                        continue;
                    }
                    let gap = dist((s.x, s.y), via.position) - via.size / 2.0 - s.width / 2.0;
                    if via_min.is_none_or(|m| gap < m.gap) {
                        via_min = Some(Gap {
                            gap,
                            at: via.position,
                        });
                    }
                }
            }
            if via_min.is_some_and(|m| m.gap < a.min_gap && m.at == via.position) {
                via_violations += 1;
            }
        }
        if via_min.is_some_and(|m| m.gap < a.min_gap) {
            net_ok = false;
        }
        all_ok &= net_ok;
        let overall = match (net_min, via_min) {
            (Some(t), Some(v)) => Some(if v.gap < t.gap { v } else { t }),
            (t, v) => t.or(v),
        };
        text.push_str(&format!(
            "{name}: {} min gap {} on {} layer(s), vias {}\n",
            if net_ok { "ok" } else { "VIOLATION" },
            overall.map_or("n/a".into(), |g| format!(
                "{:.3} mm at ({:.2}, {:.2})",
                g.gap, g.at.0, g.at.1
            )),
            layers_json.len(),
            via_min.map_or("n/a".into(), |g| format!("{:.3} mm", g.gap)),
        ));
        for l in &layers_json {
            text.push_str(&format!(
                "  {:<8} min {} violations {}\n",
                l["layer"].as_str().unwrap_or(""),
                l["min_gap_mm"]
                    .as_f64()
                    .map_or("n/a".into(), |g| format!("{g:.3} mm")),
                l["violations"]
            ));
        }
        nets_json.push(json!({
            "net": name,
            "ok": net_ok,
            "min_gap_mm": overall.map(|g| g.gap),
            "min_gap_at": overall.map(|g| [g.at.0, g.at.1]),
            "layers": layers_json,
            "vias": {"min_gap_mm": via_min.map(|g| g.gap), "min_gap_at": via_min.map(|g| [g.at.0, g.at.1]), "violations": via_violations},
        }));
    }
    let report = json!({
        "command": "coil spacing",
        "board": a.pcb.display().to_string(),
        "min_gap_mm": a.min_gap,
        "search_radius_mm": search_beyond(a.min_gap),
        "path_separation_mm": a.path_separation,
        "step_mm": a.step,
        "ok": all_ok,
        "nets": nets_json,
    });
    write_report(a.output.as_deref(), &a.format, &report, &text)?;
    Ok(if all_ok { 0 } else { 1 })
}

/// Board thickness from `(general (thickness N))`, when present.
fn general_thickness(path: &Path) -> Option<f64> {
    let text = std::fs::read_to_string(path).ok()?;
    let general = text.find("(general")?;
    let rest = &text[general..];
    let end = rest.find("(setup").unwrap_or(rest.len());
    let block = &rest[..end];
    let t = block.find("(thickness")?;
    block[t + "(thickness".len()..]
        .trim_start()
        .split(|c: char| c == ')' || c.is_whitespace())
        .next()?
        .parse()
        .ok()
}

fn resistance(a: ResistanceArgs) -> Result<i32> {
    if a.copper_um <= 0.0 || a.plating_um <= 0.0 {
        bail!("copper and plating thickness must be positive")
    }
    let pcb = Pcb::load(&a.pcb)?;
    let copper = copper_layer_names(&pcb);
    // Copper thickness per layer: explicit override, then stackup, then default.
    let mut thickness_um: BTreeMap<String, f64> = BTreeMap::new();
    if let Some(setup) = pcb.setup() {
        for l in &setup.stackup {
            if l.layer_type == "copper" && l.thickness > 0.0 {
                thickness_um.insert(l.name.clone(), l.thickness * 1000.0);
            }
        }
    }
    for spec in &a.copper_um_layer {
        let (layer, um) = spec
            .split_once('=')
            .with_context(|| format!("expected LAYER=UM, got {spec:?}"))?;
        thickness_um.insert(
            layer.trim().to_string(),
            um.trim().parse().context("copper µm")?,
        );
    }
    let board_mm = a
        .board_thickness
        .or_else(|| general_thickness(&a.pcb))
        .unwrap_or(1.6);
    let rho = RHO_CU_20 * (1.0 + ALPHA_CU * (a.temp - 20.0));
    let mut nets_json = Vec::new();
    let mut text = String::new();
    for (number, name) in select_nets(&pcb, &a.nets)? {
        let mut per_layer: BTreeMap<String, (f64, f64)> = BTreeMap::new(); // (length mm, R Ω)
        for layer in &copper {
            let t_um = thickness_um.get(layer).copied().unwrap_or(a.copper_um);
            let mut length = 0.0;
            let mut r = 0.0;
            for p in prims_for(&pcb, number, layer)? {
                length += p.length;
                r += rho * (p.length * 1e-3) / ((p.width * 1e-3) * (t_um * 1e-6));
            }
            if length > 0.0 {
                per_layer.insert(layer.clone(), (length, r));
            }
        }
        let vias: Vec<_> = pcb.vias_in_net(number).collect();
        let r_vias: f64 = vias
            .iter()
            .map(|v| {
                rho * (board_mm * 1e-3)
                    / (std::f64::consts::PI * (v.drill * 1e-3) * (a.plating_um * 1e-6))
            })
            .sum();
        let r_tracks: f64 = per_layer.values().map(|(_, r)| r).sum();
        let length: f64 = per_layer.values().map(|(l, _)| l).sum();
        text.push_str(&format!(
            "{name}: {:.4} Ω at {:.0} °C (tracks {:.4} Ω over {:.1} mm, {} vias {:.4} Ω)\n",
            r_tracks + r_vias,
            a.temp,
            r_tracks,
            length,
            vias.len(),
            r_vias
        ));
        nets_json.push(json!({
            "net": name,
            "resistance_ohm": r_tracks + r_vias,
            "tracks_ohm": r_tracks,
            "vias_ohm": r_vias,
            "length_mm": length,
            "via_count": vias.len(),
            "layers": per_layer.iter().map(|(l, (len, r))| json!({"layer": l, "length_mm": len, "ohm": r, "copper_um": thickness_um.get(l).copied().unwrap_or(a.copper_um)})).collect::<Vec<_>>(),
        }));
    }
    let report = json!({
        "command": "coil resistance",
        "board": a.pcb.display().to_string(),
        "temperature_c": a.temp,
        "rho_ohm_m": rho,
        "board_thickness_mm": board_mm,
        "plating_um": a.plating_um,
        "default_copper_um": a.copper_um,
        "nets": nets_json,
    });
    write_report(a.output.as_deref(), &a.format, &report, &text)?;
    Ok(0)
}
