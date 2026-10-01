//! `kct fix-silkscreen` (port of `kicad_tools.cli.fix_silkscreen_cmd`; public
//! flags from `parser._add_fix_silkscreen_parser`).

use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use clap::Parser;

use super::repair_common::{
    load_design_rules_from_yaml, pick_rules, py_path_str, ALL_MANUFACTURER_NAMES,
};
use super::{parse_args, Globals};
use crate::drc::repair_silkscreen::{
    SilkscreenRepairResult, SilkscreenRepairer, TextHeightRepairResult,
};
use crate::jobj;
use crate::pyjson::{dumps_indent, py_float_repr, Json};

#[derive(Parser, Debug)]
#[command(about = "Fix silkscreen line widths to meet manufacturer specifications")]
struct Args {
    /// Path to .kicad_pcb file
    pcb: String,
    /// Manufacturer to use for design rules (default: jlcpcb)
    #[arg(long, default_value = "jlcpcb", value_parser = clap::builder::PossibleValuesParser::new(ALL_MANUFACTURER_NAMES))]
    mfr: String,
    /// Number of PCB layers (default: 2)
    #[arg(long, default_value_t = 2)]
    layers: i64,
    /// Outer copper weight in oz (default: 1.0)
    #[arg(long, default_value_t = 1.0)]
    copper: f64,
    /// Minimum silkscreen line width in mm (overrides manufacturer rules)
    #[arg(long)]
    min_width: Option<f64>,
    /// Minimum silkscreen text height in mm (overrides manufacturer rules)
    #[arg(long)]
    min_height: Option<f64>,
    /// Output file path (required unless --dry-run or --in-place)
    #[arg(short = 'o', long)]
    output: Option<String>,
    /// Explicitly authorize overwriting the input PCB
    #[arg(long, conflicts_with = "output")]
    in_place: bool,
    /// Preview changes without modifying files
    #[arg(long)]
    dry_run: bool,
    /// Output format (default: text)
    #[arg(long, default_value = "text", value_parser = ["text", "json", "summary"])]
    format: String,
    /// Suppress progress output (for scripting)
    #[arg(short = 'q', long)]
    quiet: bool,
}

fn resolve(mfr: &str, layers: i64, copper: f64, explicit: Option<f64>, height: bool) -> f64 {
    if let Some(v) = explicit {
        return v;
    }
    if !mfr.is_empty() {
        match load_design_rules_from_yaml(mfr) {
            Some(all) => {
                if let Some(r) = pick_rules(&all, layers, copper) {
                    return if height {
                        r.min_silkscreen_height_mm
                    } else {
                        r.min_silkscreen_width_mm
                    };
                }
            }
            None => eprintln!("Warning: No configuration found for manufacturer '{mfr}'"),
        }
    }
    if height {
        1.0
    } else {
        0.15
    }
}

/// `Counter.most_common()`: count desc, ties in first-seen order.
fn most_common(keys: &[String]) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for k in keys {
        match out.iter_mut().find(|(n, _)| n == k) {
            Some(e) => e.1 += 1,
            None => out.push((k.clone(), 1)),
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out
}

fn print_results(
    line: &SilkscreenRepairResult,
    text: &TextHeightRepairResult,
    format: &str,
    dry_run: bool,
    mfr: &str,
) {
    let source = if mfr.is_empty() {
        String::new()
    } else {
        format!(" to {} minimum", mfr.to_uppercase())
    };
    match format {
        "json" => {
            let data = jobj! {
                "min_width_mm" => line.min_width_mm,
                "min_height_mm" => text.min_height_mm,
                "manufacturer" => mfr,
                "dry_run" => dry_run,
                "total_line_width_fixed" => Json::Int(line.total_fixed() as i64),
                "total_text_height_fixed" => Json::Int(text.total_fixed() as i64),
                "total_fixed" => Json::Int((line.total_fixed() + text.total_fixed()) as i64),
                "line_width_fixes" => Json::Arr(line.fixes.iter().map(|f| jobj! {
                    "element_type" => f.element_type.as_str(),
                    "layer" => f.layer.as_str(),
                    "footprint_ref" => f.footprint_ref.as_str(),
                    "old_width_mm" => f.old_width,
                    "new_width_mm" => f.new_width,
                }).collect()),
                "text_height_fixes" => Json::Arr(text.fixes.iter().map(|f| jobj! {
                    "element_type" => f.element_type.as_str(),
                    "layer" => f.layer.as_str(),
                    "footprint_ref" => f.footprint_ref.as_str(),
                    "old_height_mm" => f.old_height,
                    "new_height_mm" => f.new_height,
                    "old_width_mm" => f.old_width,
                    "new_width_mm" => f.new_width,
                }).collect()),
            };
            println!("{}", dumps_indent(&data, 2));
        }
        "summary" => {
            let action = if dry_run { "Would fix" } else { "Fixed" };
            let total = line.total_fixed() + text.total_fixed();
            let mut parts = Vec::new();
            if line.total_fixed() > 0 {
                parts.push(format!(
                    "{} line width violation(s) (min: {}mm)",
                    line.total_fixed(),
                    py_float_repr(line.min_width_mm)
                ));
            }
            if text.total_fixed() > 0 {
                parts.push(format!(
                    "{} text height violation(s) (min: {}mm)",
                    text.total_fixed(),
                    py_float_repr(text.min_height_mm)
                ));
            }
            if parts.is_empty() {
                parts.push("0 silkscreen violation(s)".into());
            }
            println!(
                "{action} {total} silkscreen violation(s){source}: {}",
                parts.join("; ")
            );
        }
        _ => {
            if line.total_fixed() == 0 && text.total_fixed() == 0 {
                println!("No silkscreen violations found.");
                return;
            }
            if line.total_fixed() > 0 {
                let action = if dry_run { "Would widen" } else { "Widened" };
                println!(
                    "{action} {} silkscreen line(s){source} (min: {}mm):",
                    line.total_fixed(),
                    py_float_repr(line.min_width_mm)
                );
                let keys: Vec<String> = line
                    .fixes
                    .iter()
                    .map(|f| {
                        if f.footprint_ref.is_empty() {
                            "(board-level)".into()
                        } else {
                            f.footprint_ref.clone()
                        }
                    })
                    .collect();
                for (key, count) in most_common(&keys) {
                    let i = keys.iter().position(|k| *k == key).unwrap_or(0);
                    let fx = &line.fixes[i];
                    println!(
                        "  {key}: {count} line(s) widened ({:.2}mm -> {:.2}mm)",
                        fx.old_width, fx.new_width
                    );
                }
            }
            if text.total_fixed() > 0 {
                let action = if dry_run { "Would scale" } else { "Scaled" };
                println!(
                    "{action} {} silkscreen text element(s){source} (min height: {}mm):",
                    text.total_fixed(),
                    py_float_repr(text.min_height_mm)
                );
                let keys: Vec<String> = text
                    .fixes
                    .iter()
                    .map(|f| {
                        if f.footprint_ref.is_empty() {
                            "(board-level)".into()
                        } else {
                            f.footprint_ref.clone()
                        }
                    })
                    .collect();
                for (key, count) in most_common(&keys) {
                    let i = keys.iter().position(|k| *k == key).unwrap_or(0);
                    let fx = &text.fixes[i];
                    println!(
                        "  {key}: {count} text element(s) scaled ({:.2}mm -> {:.2}mm)",
                        fx.old_height, fx.new_height
                    );
                }
            }
        }
    }
}

pub fn run(argv: Vec<OsString>, g: &Globals) -> Result<i32> {
    let mut args: Args = parse_args("fix-silkscreen", argv);
    args.quiet |= g.quiet;
    if !args.dry_run && args.output.is_none() && !args.in_place {
        eprintln!("Error: design edits require --output, --in-place, or --dry-run");
        return Ok(1);
    }
    let pcb_path = Path::new(&args.pcb);
    if !pcb_path.exists() {
        eprintln!("Error: PCB file not found: {}", py_path_str(&args.pcb));
        return Ok(1);
    }
    let suffix = pcb_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    if suffix.to_lowercase() != ".kicad_pcb" {
        eprintln!("Error: Expected .kicad_pcb file, got: {suffix}");
        return Ok(1);
    }
    let min_width = resolve(&args.mfr, args.layers, args.copper, args.min_width, false);
    let min_height = resolve(&args.mfr, args.layers, args.copper, args.min_height, true);
    let mut repairer = match SilkscreenRepairer::new(pcb_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error parsing PCB file: {e}");
            return Ok(1);
        }
    };
    let line = repairer.repair_line_widths(min_width, args.dry_run);
    let text = repairer.repair_text_heights(min_height, args.dry_run);
    if !args.quiet {
        print_results(&line, &text, &args.format, args.dry_run, &args.mfr);
    }
    if line.total_fixed() + text.total_fixed() > 0 && !args.dry_run {
        let out = args.output.clone().unwrap_or_else(|| args.pcb.clone());
        if let Err(e) = repairer.save(Some(Path::new(&out))) {
            eprintln!("Error saving PCB file: {e}");
            return Ok(1);
        }
        if !args.quiet && args.format == "text" {
            println!("\nSaved to: {}", py_path_str(&out));
        }
    }
    Ok(0)
}
