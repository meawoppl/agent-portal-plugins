//! `kct optimize-traces` (port of `kicad_tools.cli.optimize_cmd`, flags from
//! the outer `parser._add_optimize_parser`).
//!
//! `--format json` emits one sorted-key document (issue #4674) describing
//! the run, or `{"error": ..., "success": false}` with exit 1.

use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use clap::Parser;

use super::repair_common::{emit_json, py_path_str, MANUFACTURER_IDS};
use super::{parse_args, Globals};
use crate::jobj;
use crate::pyjson::{py_float_repr, Json};
use crate::router::optimizer::{OptimizationConfig, TraceOptimizer};

#[derive(Parser, Debug)]
#[command(about = "Optimize PCB traces")]
struct Args {
    /// Path to .kicad_pcb file
    pcb: String,
    /// Output file (required unless --dry-run or --in-place)
    #[arg(short = 'o', long)]
    output: Option<String>,
    /// Explicitly authorize modifying the input PCB
    #[arg(long, conflicts_with = "output")]
    in_place: bool,
    /// Only optimize traces matching this net pattern
    #[arg(long)]
    net: Option<String>,
    /// Disable collinear merging
    #[arg(long)]
    no_merge: bool,
    /// Disable zigzag elimination
    #[arg(long)]
    no_zigzag: bool,
    /// Disable 45-degree corners
    #[arg(long = "no-45")]
    no_45: bool,
    /// 45-degree chamfer size in mm (default: 0.5)
    #[arg(long, default_value_t = 0.5)]
    chamfer_size: f64,
    #[arg(short = 'v', long)]
    verbose: bool,
    /// Show results without writing
    #[arg(long)]
    dry_run: bool,
    /// Suppress progress output
    #[arg(short = 'q', long)]
    quiet: bool,
    /// Enable DRC-aware mode: roll back per-net optimizations that increase violations
    #[arg(long)]
    drc_aware: bool,
    /// Target manufacturer for DRC rules (e.g., jlcpcb, oshpark). Required with --drc-aware
    #[arg(long)]
    mfr: Option<String>,
    /// Number of copper layers for DRC checks (default: 2)
    #[arg(long, default_value_t = 2)]
    layers: i64,
    /// Copper weight in oz for DRC checks (default: 1.0)
    #[arg(long, default_value_t = 1.0)]
    copper: f64,
    /// Output format (default: text)
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
}

fn fail(as_json: bool, pcb: &str, message: &str, text: Option<String>) -> i32 {
    if as_json {
        emit_json(jobj! {
            "command" => "optimize-traces",
            "pcb" => pcb,
            "error" => message,
            "saved" => false,
            "success" => false,
        });
    } else {
        eprintln!("{}", text.unwrap_or_else(|| format!("Error: {message}")));
    }
    1
}

pub fn run(argv: Vec<OsString>, g: &Globals) -> Result<i32> {
    let args: Args = parse_args("optimize-traces", argv);
    let as_json = args.format == "json";

    if args.drc_aware && args.mfr.is_none() {
        return Ok(fail(
            as_json,
            &args.pcb,
            "--drc-aware requires --mfr to specify the manufacturer profile (e.g., --mfr jlcpcb)",
            None,
        ));
    }
    if !args.dry_run && args.output.is_none() && !args.in_place {
        fail(
            as_json,
            &args.pcb,
            super::repair_common::DESIGN_EDIT_TARGET_ERROR,
            None,
        );
        return Ok(super::repair_common::EXIT_NO_WRITE_TARGET);
    }
    let pcb_str = py_path_str(&args.pcb);
    let pcb_path = Path::new(&args.pcb);
    if !pcb_path.exists() {
        return Ok(fail(
            as_json,
            &pcb_str,
            &format!("PCB file not found: {pcb_str}"),
            None,
        ));
    }
    if let Some(mfr) = &args.mfr {
        if !MANUFACTURER_IDS.contains(&mfr.as_str()) {
            return Ok(fail(
                as_json,
                &pcb_str,
                &format!(
                    "Unknown manufacturer '{mfr}'. Valid options: {}",
                    MANUFACTURER_IDS.join(", ")
                ),
                None,
            ));
        }
    }
    let quiet = args.quiet || g.quiet || as_json;
    let config = OptimizationConfig {
        merge_collinear: !args.no_merge,
        eliminate_zigzags: !args.no_zigzag,
        convert_45_corners: !args.no_45,
        corner_chamfer_size: args.chamfer_size,
        drc_aware: args.drc_aware,
        drc_manufacturer: args.mfr.clone(),
        drc_layers: args.layers,
        drc_copper_oz: args.copper,
        ..OptimizationConfig::default()
    };
    let optimizer = TraceOptimizer::new(Some(config.clone()), None);
    let yn = |b: bool| if b { "yes" } else { "no" };
    let mfr_s = args.mfr.clone().unwrap_or_else(|| "None".into());

    if !quiet {
        println!("{}", "=".repeat(50));
        println!("Trace Optimization");
        println!("{}", "=".repeat(50));
        println!("\nInput:  {pcb_str}");
        if let Some(o) = &args.output {
            println!("Output: {o}");
        }
        if let Some(n) = &args.net {
            println!("Filter: nets matching '{n}'");
        }
        if args.drc_aware {
            println!("DRC:    aware (mfr={mfr_s}, layers={})", args.layers);
        }
        println!();
        println!("Optimizations enabled:");
        println!("  - Collinear merge: {}", yn(config.merge_collinear));
        println!("  - Zigzag elimination: {}", yn(config.eliminate_zigzags));
        println!("  - 45 corners: {}", yn(config.convert_45_corners));
        if config.convert_45_corners {
            println!(
                "    (chamfer size: {}mm)",
                py_float_repr(config.corner_chamfer_size)
            );
        }
        if args.drc_aware {
            println!("  - DRC-aware rollback: yes (mfr={mfr_s})");
        }
        println!();
    }

    let drc_counter = |text: &str| {
        crate::router::optimizer::pcb::drc_error_counts(
            text,
            config.drc_manufacturer.as_deref().unwrap_or("jlcpcb"),
            config.drc_layers,
            config.drc_copper_oz,
        )
    };
    let stats = match optimizer.optimize_pcb(
        pcb_path,
        args.output.as_deref().map(Path::new),
        args.net.as_deref(),
        args.dry_run,
        Some(&drc_counter),
    ) {
        Ok(s) => s,
        Err(e) => {
            return Ok(fail(
                as_json,
                &pcb_str,
                &format!("during optimization: {e}"),
                Some(format!("Error during optimization: {e}")),
            ))
        }
    };

    if as_json {
        let written: Json = if args.dry_run {
            Json::Null
        } else {
            Json::Str(args.output.clone().unwrap_or_else(|| pcb_str.clone()))
        };
        emit_json(jobj! {
            "command" => "optimize-traces",
            "pcb" => pcb_str.as_str(),
            "output" => args.output.clone(),
            "in_place" => args.in_place,
            "net_filter" => args.net.clone(),
            "optimizations" => jobj! {
                "merge_collinear" => config.merge_collinear,
                "eliminate_zigzags" => config.eliminate_zigzags,
                "convert_45_corners" => config.convert_45_corners,
                "chamfer_size_mm" => config.corner_chamfer_size,
            },
            "drc_aware" => args.drc_aware,
            "manufacturer" => args.mfr.clone(),
            "layers" => Json::Int(args.layers),
            "copper_oz" => args.copper,
            "stats" => jobj! {
                "nets_optimized" => Json::Int(stats.nets_optimized as i64),
                "nets_rolled_back" => Json::Int(stats.nets_rolled_back as i64),
                "segments_before" => Json::Int(stats.segments_before as i64),
                "segments_after" => Json::Int(stats.segments_after as i64),
                "segment_reduction_pct" => stats.segment_reduction(),
                "corners_before" => Json::Int(stats.corners_before as i64),
                "corners_after" => Json::Int(stats.corners_after as i64),
                "length_before_mm" => stats.length_before,
                "length_after_mm" => stats.length_after,
                "length_reduction_pct" => stats.length_reduction(),
                "drc_errors_before" => Json::Int(stats.drc_errors_before as i64),
                "drc_errors_after" => Json::Int(stats.drc_errors_after as i64),
            },
            "dry_run" => args.dry_run,
            "saved" => !args.dry_run,
            "written_to" => written,
            "success" => true,
        });
        return Ok(0);
    }

    if !quiet {
        println!("{}", "-".repeat(50));
        println!("Results:");
        println!("{}", "-".repeat(50));
        if args.drc_aware {
            let safe = stats.nets_optimized as i64 - stats.nets_rolled_back as i64;
            println!(
                "  Nets optimized:  {} (DRC safe: {safe}, rolled back: {})",
                stats.nets_optimized, stats.nets_rolled_back
            );
        } else {
            println!("  Nets optimized:  {}", stats.nets_optimized);
        }
        println!();
        println!(
            "  Segments:        {:>6} -> {:>6}  ({:+.1}%)",
            stats.segments_before,
            stats.segments_after,
            -stats.segment_reduction()
        );
        println!(
            "  Corners:         {:>6} -> {:>6}",
            stats.corners_before, stats.corners_after
        );
        println!(
            "  Total length:    {:>6.1}mm -> {:>6.1}mm  ({:+.1}%)",
            stats.length_before,
            stats.length_after,
            -stats.length_reduction()
        );
        if args.drc_aware {
            let delta = stats.drc_errors_after as i64 - stats.drc_errors_before as i64;
            let suffix = if delta > 0 {
                format!(
                    "({delta} new errors, {} nets rolled back)",
                    stats.nets_rolled_back
                )
            } else if stats.nets_rolled_back > 0 {
                format!(
                    "(no regressions, {} nets rolled back)",
                    stats.nets_rolled_back
                )
            } else {
                "(no regressions)".to_string()
            };
            println!(
                "  DRC errors:      {:>6} -> {:>6}  {suffix}",
                stats.drc_errors_before, stats.drc_errors_after
            );
        }
        println!();
        if args.dry_run {
            println!("(Dry run - no changes written)");
        } else if let Some(o) = &args.output {
            println!("Saved to: {o}");
        } else {
            println!("Updated: {pcb_str}");
        }
        println!("{}", "=".repeat(50));
    }
    let _ = args.verbose;
    Ok(0)
}
