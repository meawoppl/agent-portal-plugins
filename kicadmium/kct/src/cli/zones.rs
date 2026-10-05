//! Native copper-zone inspection and generation.

use std::ffi::OsString;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use super::{parse_args, Globals};
use crate::core::board_outline::board_outline_bounds;
use crate::schema::pcb::{util::new_uuid, Pcb};
use crate::sexp::Document;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    List {
        pcb: PathBuf,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Add(Add),
    Batch {
        pcb: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long = "power-nets")]
        power_nets: String,
        #[arg(long, default_value_t = 0.3)]
        clearance: f64,
        #[arg(long)]
        dry_run: bool,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Fill {
        pcb: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        net: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
}

#[derive(clap::Args, Clone)]
struct Add {
    pcb: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    net: String,
    #[arg(long)]
    layer: String,
    #[arg(long, default_value_t = 0)]
    priority: i64,
    #[arg(long, default_value_t = 0.3)]
    clearance: f64,
    #[arg(long = "thermal-gap", default_value_t = 0.3)]
    thermal_gap: f64,
    #[arg(long = "thermal-bridge", default_value_t = 0.4)]
    thermal_bridge: f64,
    #[arg(long = "min-thickness", default_value_t = 0.25)]
    min_thickness: f64,
    #[arg(long, conflicts_with = "region")]
    bbox: Option<String>,
    #[arg(long)]
    region: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
}

fn region(spec: &str) -> Result<Vec<(f64, f64)>> {
    let pts: Result<Vec<_>> = spec
        .split(';')
        .filter(|s| !s.trim().is_empty())
        .map(|item| {
            let v: Vec<_> = item.split(',').collect();
            if v.len() != 2 {
                bail!("invalid point {item:?}; expected X,Y")
            }
            Ok((v[0].trim().parse()?, v[1].trim().parse()?))
        })
        .collect();
    let pts = pts?;
    if pts.len() < 3 {
        bail!("a zone region needs at least three points")
    }
    Ok(pts)
}
fn bbox(spec: &str) -> Result<Vec<(f64, f64)>> {
    let v: Vec<f64> = spec
        .split(',')
        .map(|s| s.trim().parse())
        .collect::<std::result::Result<_, _>>()?;
    if v.len() != 4 || v[0] >= v[2] || v[1] >= v[3] {
        bail!("invalid bbox; expected MINX,MINY,MAXX,MAXY")
    }
    Ok(vec![(v[0], v[1]), (v[2], v[1]), (v[2], v[3]), (v[0], v[3])])
}

fn add(add: &Add) -> Result<serde_json::Value> {
    let pcb = Pcb::load(&add.pcb)?;
    let net = pcb
        .nets()
        .iter()
        .find(|n| n.name == add.net)
        .with_context(|| format!("net {:?} not found", add.net))?;
    if !pcb
        .layers()
        .iter()
        .any(|l| l.name == add.layer && l.layer_type.contains("copper"))
    {
        bail!("copper layer {:?} not found", add.layer);
    }
    let mut doc = Document::load(&add.pcb)?;
    let points = if let Some(s) = &add.bbox {
        bbox(s)?
    } else if let Some(s) = &add.region {
        region(s)?
    } else {
        let (x1, y1, x2, y2) = board_outline_bounds(&doc.root)?
            .context("board has no closed Edge.Cuts bounds; pass --bbox or --region")?;
        vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)]
    };
    let pts = points
        .iter()
        .map(|(x, y)| format!("(xy {x} {y})"))
        .collect::<Vec<_>>()
        .join(" ");
    let text = format!(
        "(zone (net {}) (net_name \"{}\") (layer \"{}\") (uuid \"{}\") (hatch edge 0.5) (priority {}) (connect_pads (clearance {})) (min_thickness {}) (fill yes (thermal_gap {}) (thermal_bridge_width {})) (polygon (pts {})))",
        net.number,
        add.net,
        add.layer,
        new_uuid(),
        add.priority,
        add.clearance,
        add.min_thickness,
        add.thermal_gap,
        add.thermal_bridge,
        pts
    );
    doc.root.children.push(crate::parse(&text)?);
    let output = add.output.clone();
    if !add.dry_run {
        let out = output
            .as_ref()
            .context("zones add is a design edit; pass --output (or use --dry-run)")?;
        doc.save(Some(out))?;
    }
    Ok(
        serde_json::json!({"pcb":add.pcb,"output":output,"dry_run":add.dry_run,"saved":!add.dry_run,"zone":{"net":add.net,"layer":add.layer,"priority":add.priority,"clearance_mm":add.clearance,"boundary_points":points.len()},"sexp":if add.dry_run{Some(text)}else{None}}),
    )
}

fn refill_copy(input: &Path, output: &Path) -> Result<()> {
    if input == output {
        bail!("zones fill never modifies its input; --output must name a different file")
    }
    std::fs::copy(input, output).with_context(|| {
        format!(
            "copy {} to {} before zone refill",
            input.display(),
            output.display()
        )
    })?;
    let cli = super::runner::find_kicad_cli()
        .context("kicad-cli not found; set KICADMIUM_KICAD_CLI or KICAD_CLI, or add it to PATH")?;
    let report = std::env::temp_dir().join(format!(
        "kct-zones-fill-{}-{}.json",
        std::process::id(),
        new_uuid()
    ));
    let result = std::process::Command::new(cli)
        .args(["pcb", "drc", "--format", "json", "--severity-all", "-o"])
        .arg(&report)
        .args(["--refill-zones", "--save-board"])
        .arg(output)
        .output()
        .context("launch kicad-cli zone refill")?;
    let report_ok = std::fs::metadata(&report).is_ok_and(|m| m.len() > 0);
    let _ = std::fs::remove_file(&report);
    if !result.status.success() && !report_ok {
        let stderr = String::from_utf8_lossy(&result.stderr);
        bail!("kicad-cli zone refill failed: {}", stderr.trim())
    }
    Ok(())
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("zones", args);
    match args.command {
        Command::List { pcb, format } => {
            let pcbm = Pcb::load(&pcb)?;
            let zones:Vec<_>=pcbm.zones().iter().map(|z|{let bounds=if z.polygon.is_empty(){None}else{Some(serde_json::json!({"min_x":z.polygon.iter().map(|p|p.0).fold(f64::INFINITY,f64::min),"min_y":z.polygon.iter().map(|p|p.1).fold(f64::INFINITY,f64::min),"max_x":z.polygon.iter().map(|p|p.0).fold(f64::NEG_INFINITY,f64::max),"max_y":z.polygon.iter().map(|p|p.1).fold(f64::NEG_INFINITY,f64::max)}))};serde_json::json!({"net_number":z.net_number,"net_name":z.net_name,"layer":z.layer,"priority":z.priority,"clearance":z.clearance,"thermal_gap":z.thermal_gap,"thermal_bridge_width":z.thermal_bridge_width,"is_filled":z.is_filled,"boundary_points":z.polygon.len(),"bounding_box":bounds})}).collect();
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&zones)?)
            } else {
                println!("Zones in {}: {}", pcb.display(), zones.len());
                for z in pcbm.zones() {
                    println!(
                        "  {:<20} {:<10} {} points{}",
                        z.net_name,
                        z.layer,
                        z.polygon.len(),
                        if z.is_filled { " filled" } else { "" }
                    );
                }
            }
        }
        Command::Add(add_args) => {
            let json = add(&add_args)?;
            if add_args.format == "json" {
                println!("{}", serde_json::to_string_pretty(&json)?)
            } else {
                println!(
                    "{} zone {} on {}",
                    if add_args.dry_run {
                        "Would add"
                    } else {
                        "Added"
                    },
                    add_args.net,
                    add_args.layer
                );
            }
        }
        Command::Batch {
            pcb,
            output,
            power_nets,
            clearance,
            dry_run,
            format,
        } => {
            let specs: Vec<_> = power_nets
                .split(',')
                .map(|s| {
                    s.split_once(':')
                        .context("--power-nets entries must be NET:LAYER")
                })
                .collect::<Result<_>>()?;
            let mut current = pcb.clone();
            let mut results = Vec::new();
            for (i, (net, layer)) in specs.iter().enumerate() {
                let final_out = output
                    .clone()
                    .context("zones batch requires --output (or --dry-run)")
                    .or_else(|e| if dry_run { Ok(PathBuf::new()) } else { Err(e) })?;
                let intermediate = if dry_run {
                    None
                } else {
                    Some(final_out.clone())
                };
                let a = Add {
                    pcb: current.clone(),
                    output: intermediate.clone(),
                    net: (*net).into(),
                    layer: (*layer).into(),
                    priority: i as i64,
                    clearance,
                    thermal_gap: 0.3,
                    thermal_bridge: 0.4,
                    min_thickness: 0.25,
                    bbox: None,
                    region: None,
                    dry_run,
                    format: "json".into(),
                };
                results.push(add(&a)?);
                if !dry_run {
                    current = final_out;
                }
            }
            let doc = serde_json::json!({"pcb":pcb,"output":output,"dry_run":dry_run,"saved":!dry_run,"zones":results});
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&doc)?)
            } else {
                println!(
                    "{} {} zones",
                    if dry_run { "Would add" } else { "Added" },
                    specs.len()
                );
            }
        }
        Command::Fill {
            pcb,
            output,
            net,
            dry_run,
            format,
        } => {
            if !dry_run && output.is_none() {
                bail!("zones fill is a design edit; pass --output (or use --dry-run)")
            }
            let target = output.as_ref().unwrap_or(&pcb);
            let board = Pcb::load(&pcb)?;
            if let Some(name) = &net {
                if !board.nets().iter().any(|candidate| &candidate.name == name) {
                    bail!("net {name:?} not found")
                }
            }
            if !dry_run {
                refill_copy(&pcb, target)?;
            }
            let note = if dry_run {
                "Dry run: input validated; no output copied or zones refilled"
            } else {
                "Copied the input and refilled all zones in the explicit output with kicad-cli"
            };
            let doc = serde_json::json!({"pcb":pcb,"output":output,"net":net,"dry_run":dry_run,"filled":!dry_run,"scope":"all zones (KiCad refill is board-wide)","note":note});
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&doc)?)
            } else {
                println!("{}", doc["note"].as_str().unwrap());
            }
        }
    }
    Ok(0)
}
