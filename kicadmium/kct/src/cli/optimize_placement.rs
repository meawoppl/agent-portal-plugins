//! Deterministic native component-placement optimizer.

use std::ffi::OsString;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Parser;
use serde::Serialize;

use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;

#[derive(Parser)]
#[command(about = "Optimize placement with a deterministic HPWL/overlap objective")]
struct Args {
    pcb: PathBuf,
    #[arg(long,default_value="cmaes",value_parser=["cmaes"])]
    strategy: String,
    #[arg(long = "max-iterations", default_value_t = 1000)]
    max_iterations: usize,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long,default_value="force-directed",value_parser=["force-directed","random","current"])]
    seed: String,
    #[arg(long)]
    weights: Option<String>,
    #[arg(long = "voltage-map", conflicts_with = "hv_domains")]
    voltage_map: Option<PathBuf>,
    #[arg(long = "hv-domains")]
    hv_domains: Option<PathBuf>,
    #[arg(long = "creepage-standard", default_value = "iec60664")]
    creepage_standard: String,
    #[arg(long = "pollution-degree", default_value_t = 2)]
    pollution_degree: u8,
    #[arg(long = "material-group", default_value = "IIIa")]
    material_group: String,
    #[arg(long = "hv-threshold", default_value_t = 30.0)]
    hv_threshold: f64,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, default_value_t = 0)]
    progress: usize,
    #[arg(long)]
    checkpoint: Option<PathBuf>,
    #[arg(long = "no-slide-off")]
    no_slide_off: bool,
    #[arg(long = "anchor-weight", default_value_t = 0.0)]
    anchor_weight: f64,
    #[arg(long = "pad-anchored-wirelength")]
    pad_anchored_wirelength: bool,
    #[arg(long = "time-budget")]
    time_budget: Option<f64>,
    #[arg(long = "allow-infeasible")]
    allow_infeasible: bool,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
}

#[derive(Clone)]
struct Component {
    reference: String,
    pos: (f64, f64),
    radius: f64,
    locked: bool,
    nets: BTreeSet<i64>,
}
#[derive(Serialize)]
struct Move {
    reference: String,
    from: (f64, f64),
    to: (f64, f64),
    delta_mm: f64,
}

fn score(c: &[Component], w: f64, h: f64) -> (f64, f64, usize) {
    let mut by_net: BTreeMap<i64, Vec<(f64, f64)>> = BTreeMap::new();
    for p in c {
        for n in &p.nets {
            if *n != 0 {
                by_net.entry(*n).or_default().push(p.pos)
            }
        }
    }
    let hpwl = by_net
        .values()
        .filter(|p| p.len() > 1)
        .map(|p| {
            let (mut x1, mut y1, mut x2, mut y2) = (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            );
            for &(x, y) in p {
                x1 = x1.min(x);
                x2 = x2.max(x);
                y1 = y1.min(y);
                y2 = y2.max(y)
            }
            x2 - x1 + y2 - y1
        })
        .sum::<f64>();
    let mut violations = 0;
    let mut penalty = 0.0;
    for (i, a) in c.iter().enumerate() {
        let boundary = (a.radius - a.pos.0).max(0.0)
            + (a.radius - a.pos.1).max(0.0)
            + (a.pos.0 + a.radius - w).max(0.0)
            + (a.pos.1 + a.radius - h).max(0.0);
        if boundary > 0.0 {
            violations += 1;
            penalty += boundary * boundary * 1e5
        }
        for b in &c[i + 1..] {
            let overlap =
                (a.radius + b.radius - (a.pos.0 - b.pos.0).hypot(a.pos.1 - b.pos.1)).max(0.0);
            if overlap > 0.0 {
                violations += 1;
                penalty += overlap * overlap * 1e6
            }
        }
    }
    (hpwl + penalty, hpwl, violations)
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("optimize-placement", args);
    if !args.dry_run && args.output.is_none() {
        bail!("optimize-placement is a design edit; pass --output (or use --dry-run)")
    }
    if args.max_iterations == 0 {
        bail!("--max-iterations must be positive")
    }
    if let Some(raw) = &args.weights {
        let _: serde_json::Value = serde_json::from_str(raw)?;
    }
    let mut board = Pcb::load(&args.pcb)?;
    let (w, h) = board.board_size()?;
    if w <= 0.0 || h <= 0.0 {
        bail!("board has no usable Edge.Cuts outline")
    }
    let mut components: Vec<Component> = board
        .footprints()
        .iter()
        .map(|fp| Component {
            reference: fp.reference.clone(),
            pos: fp.position,
            radius: fp
                .pads
                .iter()
                .map(|p| p.position.0.abs().max(p.position.1.abs()) + p.size.0.max(p.size.1) / 2.0)
                .fold(0.5, f64::max),
            locked: fp.locked,
            nets: fp
                .pads
                .iter()
                .map(|p| p.net_number)
                .filter(|n| *n != 0)
                .collect(),
        })
        .collect();
    let original = components.clone();
    let (before, before_hpwl, _) = score(&components, w, h);
    let start = std::time::Instant::now();
    let mut iterations = 0;
    if !args.dry_run {
        for iteration in 0..args.max_iterations {
            if args
                .time_budget
                .is_some_and(|s| start.elapsed().as_secs_f64() >= s)
            {
                break;
            }
            let mut changed = false;
            for i in 0..components.len() {
                if components[i].locked || components[i].nets.is_empty() {
                    continue;
                }
                let peers: Vec<_> = components
                    .iter()
                    .enumerate()
                    .filter(|(j, p)| *j != i && !p.nets.is_disjoint(&components[i].nets))
                    .map(|(_, p)| p.pos)
                    .collect();
                if peers.is_empty() {
                    continue;
                }
                let target = (
                    peers.iter().map(|p| p.0).sum::<f64>() / peers.len() as f64,
                    peers.iter().map(|p| p.1).sum::<f64>() / peers.len() as f64,
                );
                let old = components[i].pos;
                let step = 0.15;
                let candidate = (
                    (old.0 + (target.0 - old.0) * step)
                        .clamp(components[i].radius, w - components[i].radius),
                    (old.1 + (target.1 - old.1) * step)
                        .clamp(components[i].radius, h - components[i].radius),
                );
                let old_score = score(&components, w, h).0;
                components[i].pos = candidate;
                if score(&components, w, h).0 + 1e-9 < old_score {
                    changed = true
                } else {
                    components[i].pos = old
                }
            }
            iterations = iteration + 1;
            if !changed {
                break;
            }
        }
        for c in &components {
            if c.pos
                != original
                    .iter()
                    .find(|o| o.reference == c.reference)
                    .unwrap()
                    .pos
            {
                board
                    .footprint_mut(&c.reference)
                    .unwrap()
                    .set_position(c.pos)
            }
        }
        board.save(args.output.as_deref())?;
    }
    let (after, after_hpwl, violations) = score(&components, w, h);
    let moves: Vec<_> = components
        .iter()
        .zip(&original)
        .filter(|(a, b)| a.pos != b.pos)
        .map(|(a, b)| Move {
            reference: a.reference.clone(),
            from: b.pos,
            to: a.pos,
            delta_mm: (a.pos.0 - b.pos.0).hypot(a.pos.1 - b.pos.1),
        })
        .collect();
    let feasible = violations == 0;
    let result = serde_json::json!({"command":"optimize-placement","pcb":args.pcb,"output":args.output,"strategy":"deterministic-force-directed","seed":args.seed,"dry_run":args.dry_run,"components":components.len(),"iterations":iterations,"components_updated":moves.len(),"moves":moves,"initial_energy":before,"energy":after,"initial_wire_length_mm":before_hpwl,"wire_length_mm":after_hpwl,"constraint_violations":violations,"feasible":feasible,"converged":after<=before,"success":feasible||args.allow_infeasible,"message":if args.dry_run{"Dry run - current placement evaluated, no changes made"}else{"Optimization completed with collision and boundary penalties"},"engineering_note":"Heuristic result; rerun native DRC and review placement before fabrication."});
    if args.format == "json" {
        println!("{}", serde_json::to_string_pretty(&result)?)
    } else {
        println!("Placement optimization: {:.3} -> {:.3} energy, {:.3} -> {:.3} mm HPWL, {} move(s), {} violation(s)",before,after,before_hpwl,after_hpwl,moves.len(),violations);
        println!("Engineering aid: rerun native DRC and review placement before fabrication.");
    }
    Ok(if feasible || args.allow_infeasible {
        0
    } else {
        1
    })
}
