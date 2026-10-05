use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use super::{parse_args, Globals};
use crate::manufacturers;

#[derive(Parser)]
#[command(about = "Manufacturer rules and capabilities")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    List {
        #[arg(long)]
        json: bool,
    },
    Info {
        manufacturer: String,
        #[arg(long)]
        json: bool,
    },
    Rules {
        manufacturer: String,
        #[arg(short = 'l', long, default_value_t = 2)]
        layers: u8,
        #[arg(short = 'c', long, default_value_t = 1.0)]
        copper: f64,
        #[arg(long)]
        json: bool,
    },
    Compare {
        #[arg(short = 'l', long, default_value_t = 2)]
        layers: u8,
        #[arg(short = 'c', long, default_value_t = 1.0)]
        copper: f64,
        #[arg(long)]
        json: bool,
    },
    Find {
        #[arg(long, default_value_t = 5.0)]
        trace: f64,
        #[arg(long, default_value_t = 5.0)]
        clearance: f64,
        #[arg(long, default_value_t = 0.3)]
        via: f64,
        #[arg(short = 'l', long, default_value_t = 2)]
        layers: u8,
        #[arg(long)]
        assembly: bool,
        #[arg(long)]
        json: bool,
    },
    ExportDru {
        manufacturer: String,
        #[arg(short = 'l', long, default_value_t = 2)]
        layers: u8,
        #[arg(short = 'c', long, default_value_t = 1.0)]
        copper: f64,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    ApplyRules {
        file: PathBuf,
        manufacturer: String,
        #[arg(short = 'l', long, default_value_t = 2)]
        layers: u8,
        #[arg(short = 'c', long, default_value_t = 1.0)]
        copper: f64,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    Validate {
        file: PathBuf,
        manufacturer: String,
        #[arg(short = 'l', long, default_value_t = 2)]
        layers: u8,
        #[arg(short = 'c', long, default_value_t = 1.0)]
        copper: f64,
        #[arg(long)]
        json: bool,
    },
    ImportDru {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(args: Vec<OsString>, _globals: &Globals) -> Result<i32> {
    match parse_args::<Args>("mfr", args).command {
        Command::List { json } => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(manufacturers::profiles())?
                );
            } else {
                for p in manufacturers::profiles() {
                    println!("{:<16} {}", p.id, p.name);
                }
            }
        }
        Command::Info { manufacturer, json } => {
            let p = manufacturers::profile(&manufacturer)?;
            if json {
                println!("{}", serde_json::to_string_pretty(p)?);
            } else {
                println!(
                    "{} ({})\n{}\nassembly: {}\nparts: {}",
                    p.name,
                    p.id,
                    p.website,
                    p.supports_assembly,
                    p.parts_library.unwrap_or("none")
                );
            }
        }
        Command::Rules {
            manufacturer,
            layers,
            copper,
            json,
        } => {
            let rules = manufacturers::rules(&manufacturer, layers, copper)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rules)?);
            } else {
                println!("{} {layers}-layer {copper} oz\ntrace: {:.4} mm ({:.2} mil)\nclearance: {:.4} mm ({:.2} mil)\nvia: {:.3}/{:.3} mm", manufacturers::profile(&manufacturer)?.name, rules.min_trace_width_mm, rules.min_trace_width_mil(), rules.min_clearance_mm, rules.min_clearance_mil(), rules.min_via_drill_mm, rules.min_via_diameter_mm);
            }
        }
        Command::Compare {
            layers,
            copper,
            json,
        } => {
            let rows = manufacturers::profiles()
                .iter()
                .filter_map(|p| {
                    manufacturers::rules(p.id, layers, copper)
                        .ok()
                        .map(|r| (p.id, r))
                })
                .collect::<Vec<_>>();
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for (id, r) in rows {
                    println!(
                        "{id:<16} trace {:>7.4} mm  clearance {:>7.4} mm  drill {:>6.3} mm",
                        r.min_trace_width_mm, r.min_clearance_mm, r.min_via_drill_mm
                    );
                }
            }
        }
        Command::Find {
            trace,
            clearance,
            via,
            layers,
            assembly,
            json,
        } => {
            let trace_mm = trace * 0.0254;
            let clearance_mm = clearance * 0.0254;
            let matches = manufacturers::profiles()
                .iter()
                .filter(|p| !assembly || p.supports_assembly)
                .filter(|p| p.supported_layers.contains(&layers))
                .filter(|p| {
                    manufacturers::rules(p.id, layers, 1.0).is_ok_and(|r| {
                        r.min_trace_width_mm <= trace_mm
                            && r.min_clearance_mm <= clearance_mm
                            && r.min_via_drill_mm <= via
                    })
                })
                .collect::<Vec<_>>();
            if json {
                println!("{}", serde_json::to_string_pretty(&matches)?);
            } else {
                for p in matches {
                    println!("{:<16} {}", p.id, p.name);
                }
            }
        }
        Command::ExportDru {
            manufacturer,
            layers,
            copper,
            output,
        } => {
            let dru = manufacturers::dru_preset(&manufacturer, layers, copper)?;
            if let Some(path) = output {
                crate::fsutil::atomic_write(&path, dru.as_bytes())?;
            } else {
                print!("{dru}");
            }
        }
        Command::ApplyRules {
            file,
            manufacturer,
            layers,
            copper,
            output,
            dry_run,
            json,
        } => {
            let dru = manufacturers::dru_preset(&manufacturer, layers, copper)?;
            let target = output.unwrap_or_else(|| file.with_extension("kicad_dru"));
            if !dry_run {
                crate::fsutil::atomic_write(&target, dru.as_bytes())?;
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"target":target,"manufacturer":manufacturer,"dry_run":dry_run})
                    )?
                );
            } else {
                println!(
                    "{} rules: {}",
                    if dry_run { "Would write" } else { "Wrote" },
                    target.display()
                );
            }
        }
        Command::Validate {
            file,
            manufacturer,
            layers,
            copper,
            json,
        } => {
            let limits = manufacturers::rules(&manufacturer, layers, copper)?;
            let root = crate::sexp::parse_file(&file)?;
            let mut violations = vec![];
            fn walk(
                n: &crate::sexp::SExp,
                limits: &crate::manufacturers::DesignRules,
                out: &mut Vec<serde_json::Value>,
            ) {
                if n.name.as_deref() == Some("segment") {
                    if let Some(w) = n
                        .children
                        .iter()
                        .find(|c| c.name.as_deref() == Some("width"))
                        .and_then(|c| c.children.first())
                        .and_then(|x| x.value.as_ref())
                        .and_then(crate::sexp::Value::as_f64)
                    {
                        if w < limits.min_trace_width_mm {
                            out.push(serde_json::json!({"kind":"trace_width","actual_mm":w,"minimum_mm":limits.min_trace_width_mm}));
                        }
                    }
                }
                if n.name.as_deref() == Some("via") {
                    let size = n
                        .children
                        .iter()
                        .find(|c| c.name.as_deref() == Some("size"))
                        .and_then(|c| c.children.first())
                        .and_then(|x| x.value.as_ref())
                        .and_then(crate::sexp::Value::as_f64);
                    let drill = n
                        .children
                        .iter()
                        .find(|c| c.name.as_deref() == Some("drill"))
                        .and_then(|c| c.children.first())
                        .and_then(|x| x.value.as_ref())
                        .and_then(crate::sexp::Value::as_f64);
                    if size.is_some_and(|v| v < limits.min_via_diameter_mm)
                        || drill.is_some_and(|v| v < limits.min_via_drill_mm)
                    {
                        out.push(serde_json::json!({"kind":"via_size","diameter_mm":size,"drill_mm":drill,"minimum_diameter_mm":limits.min_via_diameter_mm,"minimum_drill_mm":limits.min_via_drill_mm}));
                    }
                }
                for c in &n.children {
                    walk(c, limits, out);
                }
            }
            walk(&root, &limits, &mut violations);
            let ok = violations.is_empty();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"valid":ok,"violations":violations})
                    )?
                )
            } else {
                println!(
                    "{}: {} violation(s)",
                    if ok { "PASS" } else { "FAIL" },
                    violations.len()
                )
            }
            if !ok {
                return Ok(1);
            }
        }
        Command::ImportDru { file, json } => {
            let text = std::fs::read_to_string(&file)?;
            let rules = text
                .lines()
                .filter(|l| l.trim_start().starts_with("(rule "))
                .map(|l| l.trim().to_owned())
                .collect::<Vec<_>>();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"file":file,"rules":rules,"text":text})
                    )?
                )
            } else {
                println!("{}\n{} rule declaration(s)", file.display(), rules.len());
                for r in rules {
                    println!("  {r}")
                }
            }
        }
    }
    Ok(0)
}
