//! Native placement inspection and deterministic editing tools.

use std::ffi::OsString;

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};

use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Check {
        pcb: PathBuf,
        #[arg(long = "pad-clearance", default_value_t = 0.2)]
        pad_clearance: f64,
        #[arg(long,default_value="table",value_parser=["table","json","summary"])]
        format: String,
    },
    Snap(Snap),
    Align(Align),
    Distribute(Distribute),
    Suggest {
        pcb: PathBuf,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Fix {
        pcb: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Nudge {
        pcb: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Optimize {
        pcb: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Refine {
        pcb: PathBuf,
    },
}
#[derive(ClapArgs)]
struct Snap {
    pcb: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long, default_value_t = 0.5)]
    grid: f64,
    #[arg(long, default_value_t = 90.0)]
    rotation: f64,
    #[arg(long)]
    dry_run: bool,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
}
#[derive(ClapArgs)]
struct Align {
    pcb: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, value_delimiter = ',')]
    components: Vec<String>,
    #[arg(long,default_value="row",value_parser=["row","column"])]
    axis: String,
    #[arg(long, default_value = "center")]
    reference: String,
    #[arg(long, default_value_t = 0.1)]
    tolerance: f64,
    #[arg(long)]
    dry_run: bool,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
}
#[derive(ClapArgs)]
struct Distribute {
    pcb: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, value_delimiter = ',')]
    components: Vec<String>,
    #[arg(long,default_value="horizontal",value_parser=["horizontal","vertical"])]
    direction: String,
    #[arg(long)]
    dry_run: bool,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
}

fn require_output(output: &Option<PathBuf>, dry: bool) -> Result<()> {
    if !dry && output.is_none() {
        bail!("placement edits require --output (or use --dry-run)")
    }
    Ok(())
}
fn radius(fp: &crate::schema::pcb::Footprint) -> f64 {
    fp.pads
        .iter()
        .map(|p| p.position.0.abs().max(p.position.1.abs()) + p.size.0.max(p.size.1) / 2.0)
        .fold(0.5, f64::max)
}
fn conflicts(pcb: &Pcb, clearance: f64) -> Vec<serde_json::Value> {
    let f = pcb.footprints();
    let mut out = Vec::new();
    for i in 0..f.len() {
        for b in &f[i + 1..] {
            let a = &f[i];
            let actual = (a.position.0 - b.position.0).hypot(a.position.1 - b.position.1)
                - radius(a)
                - radius(b);
            if actual < clearance {
                out.push(serde_json::json!({"type":"component_overlap","component1":a.reference,"component2":b.reference,"clearance_mm":actual.max(0.0),"required_mm":clearance,"severity":"error"}));
            }
        }
    }
    out
}

#[derive(Clone)]
struct PlacementBody {
    reference: String,
    position: (f64, f64),
    radius: f64,
    locked: bool,
}

fn overlap_count(bodies: &[PlacementBody], clearance: f64) -> usize {
    bodies
        .iter()
        .enumerate()
        .map(|(i, a)| {
            bodies[i + 1..]
                .iter()
                .filter(|b| {
                    (a.position.0 - b.position.0).hypot(a.position.1 - b.position.1)
                        < a.radius + b.radius + clearance
                })
                .count()
        })
        .sum()
}

fn repair_placement(
    pcb: &PathBuf,
    output: &Option<PathBuf>,
    dry_run: bool,
    format: &str,
    mode: &str,
) -> Result<i32> {
    require_output(output, dry_run)?;
    let mut board = Pcb::load(pcb)?;
    let clearance = 0.2;
    let mut bodies: Vec<_> = board
        .footprints()
        .iter()
        .map(|fp| PlacementBody {
            reference: fp.reference.clone(),
            position: fp.position,
            radius: radius(fp),
            locked: fp.locked,
        })
        .collect();
    let initial = overlap_count(&bodies, clearance);
    let passes = match mode {
        "nudge" => 1,
        "fix" => 16,
        _ => 64,
    };
    let original = bodies.clone();
    for _ in 0..passes {
        let before = overlap_count(&bodies, clearance);
        if before == 0 {
            break;
        }
        let mut improved = false;
        'pairs: for i in 0..bodies.len() {
            for j in i + 1..bodies.len() {
                let dx = bodies[j].position.0 - bodies[i].position.0;
                let dy = bodies[j].position.1 - bodies[i].position.1;
                let distance = dx.hypot(dy);
                let needed = bodies[i].radius + bodies[j].radius + clearance;
                if distance >= needed {
                    continue;
                }
                let moving = if !bodies[j].locked {
                    j
                } else if !bodies[i].locked {
                    i
                } else {
                    continue;
                };
                let sign = if moving == j { 1.0 } else { -1.0 };
                let (ux, uy) = if distance > 1e-9 {
                    (dx / distance * sign, dy / distance * sign)
                } else {
                    (if moving % 2 == 0 { -1.0 } else { 1.0 }, 0.0)
                };
                let old = bodies[moving].position;
                let step = needed - distance + 0.05;
                bodies[moving].position = (old.0 + ux * step, old.1 + uy * step);
                if overlap_count(&bodies, clearance) < before {
                    improved = true;
                    break 'pairs;
                }
                bodies[moving].position = old;
            }
        }
        if !improved {
            break;
        }
    }
    let remaining = overlap_count(&bodies, clearance);
    let moves: Vec<_> = bodies
        .iter()
        .zip(&original)
        .filter(|(now, was)| now.position != was.position)
        .map(|(now, was)| {
            serde_json::json!({"reference":now.reference,"from":was.position,"to":now.position})
        })
        .collect();
    if !dry_run {
        for body in &bodies {
            let was = original
                .iter()
                .find(|candidate| candidate.reference == body.reference)
                .context("placement body disappeared")?;
            if body.position != was.position {
                board
                    .footprint_mut(&body.reference)
                    .with_context(|| format!("footprint {} disappeared", body.reference))?
                    .set_position(body.position);
            }
        }
        board.save(output.as_deref())?;
    }
    let result = serde_json::json!({"pcb":pcb,"output":output,"dry_run":dry_run,"mode":mode,"method":"greedy minimum-separation nudges accepted only when total overlap count decreases","initial_conflicts":initial,"remaining_conflicts":remaining,"components_updated":moves.len(),"moves":moves,"success":remaining==0,"engineering_note":"Conservative geometric repair only; rerun native DRC and review placement."});
    emit(
        format,
        &result,
        &format!(
            "{} {} component(s); placement conflicts {} -> {}",
            if dry_run { "Would move" } else { "Moved" },
            moves.len(),
            initial,
            remaining
        ),
    )?;
    Ok(if remaining == 0 { 0 } else { 1 })
}
fn emit(format: &str, value: &serde_json::Value, text: &str) -> Result<()> {
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(value)?)
    } else {
        println!("{text}")
    }
    Ok(())
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("placement", args);
    match args.command {
        Command::Check {
            pcb,
            pad_clearance,
            format,
        } => {
            let board = Pcb::load(&pcb)?;
            let c = conflicts(&board, pad_clearance);
            let result = serde_json::json!({"pcb":pcb,"components":board.footprints().len(),"conflicts":c,"conflict_count":c.len(),"passed":c.is_empty(),"method":"conservative component pad-envelope circles"});
            emit(
                &format,
                &result,
                &format!(
                    "Placement: {} component(s), {} conflict(s)",
                    board.footprints().len(),
                    c.len()
                ),
            )?;
            return Ok(if c.is_empty() { 0 } else { 1 });
        }
        Command::Snap(a) => {
            require_output(&a.output, a.dry_run)?;
            if a.grid <= 0.0 || a.rotation < 0.0 {
                bail!("grid must be > 0 and rotation >= 0")
            }
            let mut board = Pcb::load(&a.pcb)?;
            let changes: Vec<_> = board
                .footprints()
                .iter()
                .filter(|f| !f.locked)
                .map(|f| {
                    let p = (
                        (f.position.0 / a.grid).round() * a.grid,
                        (f.position.1 / a.grid).round() * a.grid,
                    );
                    let r = if a.rotation > 0.0 {
                        (f.rotation / a.rotation).round() * a.rotation
                    } else {
                        f.rotation
                    };
                    (f.reference.clone(), f.position, p, f.rotation, r)
                })
                .filter(|x| x.1 != x.2 || x.3 != x.4)
                .collect();
            if !a.dry_run {
                for (r, _, p, _, rot) in &changes {
                    let mut fp = board
                        .footprint_mut(r)
                        .with_context(|| format!("footprint {r} disappeared"))?;
                    fp.set_position(*p);
                    fp.set_rotation(*rot);
                }
                board.save(a.output.as_deref())?;
            }
            let result = serde_json::json!({"pcb":a.pcb,"output":a.output,"dry_run":a.dry_run,"grid_mm":a.grid,"rotation_degrees":a.rotation,"components_updated":changes.len(),"changes":changes.iter().map(|(r,old,new,ro,rn)|serde_json::json!({"reference":r,"from":[old.0,old.1,ro],"to":[new.0,new.1,rn]})).collect::<Vec<_>>()});
            emit(
                &a.format,
                &result,
                &format!(
                    "{} {} component(s)",
                    if a.dry_run { "Would snap" } else { "Snapped" },
                    changes.len()
                ),
            )?;
        }
        Command::Align(a) => {
            require_output(&a.output, a.dry_run)?;
            let mut board = Pcb::load(&a.pcb)?;
            let refs: BTreeSet<_> = a.components.iter().cloned().collect();
            let selected: Vec<_> = board
                .footprints()
                .iter()
                .filter(|f| refs.contains(&f.reference))
                .map(|f| (f.reference.clone(), f.position))
                .collect();
            if selected.len() < 2 {
                bail!("select at least two existing components")
            };
            let target = if a.axis == "row" {
                selected.iter().map(|x| x.1.1).sum::<f64>() / selected.len() as f64
            } else {
                selected.iter().map(|x| x.1.0).sum::<f64>() / selected.len() as f64
            };
            let changes: Vec<_> = selected
                .into_iter()
                .filter_map(|(r, p)| {
                    let n = if a.axis == "row" {
                        (p.0, target)
                    } else {
                        (target, p.1)
                    };
                    if (p.0 - n.0).abs().max((p.1 - n.1).abs()) > a.tolerance {
                        Some((r, p, n))
                    } else {
                        None
                    }
                })
                .collect();
            if !a.dry_run {
                for (r, _, p) in &changes {
                    board.footprint_mut(r).unwrap().set_position(*p)
                }
                board.save(a.output.as_deref())?
            }
            let result = serde_json::json!({"pcb":a.pcb,"output":a.output,"dry_run":a.dry_run,"axis":a.axis,"reference":a.reference,"target":target,"components_updated":changes.len(),"changes":changes});
            emit(
                &a.format,
                &result,
                &format!(
                    "{} {} component(s)",
                    if a.dry_run { "Would align" } else { "Aligned" },
                    changes.len()
                ),
            )?;
        }
        Command::Distribute(a) => {
            require_output(&a.output, a.dry_run)?;
            let mut board = Pcb::load(&a.pcb)?;
            let refs: BTreeSet<_> = a.components.iter().cloned().collect();
            let mut selected: Vec<_> = board
                .footprints()
                .iter()
                .filter(|f| refs.contains(&f.reference))
                .map(|f| (f.reference.clone(), f.position))
                .collect();
            if selected.len() < 3 {
                bail!("select at least three existing components")
            };
            let axis = if a.direction == "horizontal" { 0 } else { 1 };
            selected.sort_by(|x, y| {
                if axis == 0 {
                    x.1.0.total_cmp(&y.1.0)
                } else {
                    x.1.1.total_cmp(&y.1.1)
                }
            });
            let lo = if axis == 0 {
                selected[0].1.0
            } else {
                selected[0].1.1
            };
            let hi = if axis == 0 {
                selected.last().unwrap().1.0
            } else {
                selected.last().unwrap().1.1
            };
            let step = (hi - lo) / (selected.len() - 1) as f64;
            let changes: Vec<_> = selected
                .into_iter()
                .enumerate()
                .map(|(i, (r, p))| {
                    let n = if axis == 0 {
                        (lo + i as f64 * step, p.1)
                    } else {
                        (p.0, lo + i as f64 * step)
                    };
                    (r, p, n)
                })
                .collect();
            if !a.dry_run {
                for (r, _, p) in &changes {
                    board.footprint_mut(r).unwrap().set_position(*p)
                }
                board.save(a.output.as_deref())?
            }
            let result = serde_json::json!({"pcb":a.pcb,"output":a.output,"dry_run":a.dry_run,"direction":a.direction,"spacing_mm":step,"components_updated":changes.len(),"changes":changes});
            emit(
                &a.format,
                &result,
                &format!(
                    "{} {} component(s)",
                    if a.dry_run {
                        "Would distribute"
                    } else {
                        "Distributed"
                    },
                    changes.len()
                ),
            )?;
        }
        Command::Suggest { pcb, format } => {
            let board = Pcb::load(&pcb)?;
            let c = conflicts(&board, 0.2);
            let suggestions:Vec<_>=c.iter().map(|v|serde_json::json!({"action":"separate","components":[v["component1"].clone(),v["component2"].clone()],"rationale":"component pad envelopes overlap or violate the requested floor","advisory":true})).collect();
            let result = serde_json::json!({"pcb":pcb,"suggestions":suggestions,"count":suggestions.len(),"authorized_edits":false});
            emit(
                &format,
                &result,
                &format!(
                    "Generated {} advisory placement suggestion(s)",
                    suggestions.len()
                ),
            )?;
        }
        Command::Fix {
            pcb,
            output,
            dry_run,
            format,
        } => return repair_placement(&pcb, &output, dry_run, &format, "fix"),
        Command::Nudge {
            pcb,
            output,
            dry_run,
            format,
        } => return repair_placement(&pcb, &output, dry_run, &format, "nudge"),
        Command::Optimize {
            pcb,
            output,
            dry_run,
            format,
        } => return repair_placement(&pcb, &output, dry_run, &format, "optimize"),
        Command::Refine { pcb } => bail!(
            "interactive refinement is intentionally unavailable in the single binary; use the workbench on {}",
            pcb.display()
        ),
    }
    Ok(0)
}
