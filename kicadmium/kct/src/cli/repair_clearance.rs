//! `kct repair-clearance` (port of `kicad_tools.cli.repair_clearance_cmd`):
//! nudge traces/vias to fix DRC clearance violations from a report (or a
//! fresh `kicad-cli pcb drc` run). Exit 1 when violations remain unrepaired.

use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use clap::Parser;

use super::repair_common::{py_path_str, MANUFACTURER_IDS};
use super::runner::{find_kicad_cli, run_drc};
use super::{parse_args, Globals};
use crate::drc::repair_clearance::{ClearanceRepairer, FootprintNudgeResult, NudgeResult, RepairResult};
use crate::drc::violation::ViolationType;
use crate::drc::DRCReport;
use crate::jobj;
use crate::pyjson::{dumps_indent, py_float_repr, py_round, Json};

#[derive(Parser, Debug)]
#[command(about = "Repair clearance violations by nudging traces")]
struct Args {
    /// Path to .kicad_pcb file
    pcb: String,
    /// Path to existing DRC report (.rpt or .json)
    #[arg(long)]
    drc_report: Option<String>,
    /// Target manufacturer (for context)
    #[arg(short = 'm', long, value_parser = clap::builder::PossibleValuesParser::new(MANUFACTURER_IDS))]
    mfr: Option<String>,
    /// Maximum nudge distance in mm (default: 0.1)
    #[arg(long, default_value_t = 0.1)]
    max_displacement: f64,
    /// Extra clearance margin beyond minimum in mm (default: 0.01)
    #[arg(long, default_value_t = 0.01)]
    margin: f64,
    /// Which object to move (default: move-trace)
    #[arg(long, default_value = "move-trace", value_parser = ["move-trace", "move-via"])]
    prefer: String,
    /// Enable footprint nudging for pad-pad clearance violations
    #[arg(long)]
    nudge_footprints: bool,
    /// Output file path (default: overwrite input)
    #[arg(short = 'o', long)]
    output: Option<String>,
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

/// `_get_drc_report`: load `--drc-report` or run kicad-cli DRC.
pub fn get_drc_report(drc_report: Option<&str>, pcb_path: &Path) -> Option<DRCReport> {
    if let Some(p) = drc_report {
        let rp = Path::new(p);
        if !rp.exists() {
            eprintln!("Error: DRC report not found: {}", py_path_str(p));
            return None;
        }
        return match DRCReport::load(rp) {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("Error loading DRC report: {e}");
                None
            }
        };
    }
    let Some(cli) = find_kicad_cli() else {
        eprintln!("Error: No DRC report provided and kicad-cli not found.");
        eprintln!("Provide a DRC report with --drc-report, or install KiCad 8.");
        return None;
    };
    println!(
        "Running DRC on: {}",
        pcb_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    );
    let res = run_drc(pcb_path, None, "json", true, Some(&cli));
    if !res.success {
        eprintln!("Error running DRC: {}", res.stderr);
        return None;
    }
    let out = res.output_path?;
    let report = DRCReport::load(&out).ok();
    let _ = std::fs::remove_file(&out);
    if report.is_none() {
        eprintln!("Error loading DRC report: {}", out.display());
    }
    report
}

fn r4(v: f64) -> f64 {
    py_round(v, 4)
}

pub fn nudge_json(n: &NudgeResult) -> Json {
    jobj! {
        "object_type" => n.object_type.as_str(),
        "x" => n.x,
        "y" => n.y,
        "net_name" => n.net_name.as_str(),
        "layer" => n.layer.as_str(),
        "displacement_x_mm" => r4(n.displacement_x),
        "displacement_y_mm" => r4(n.displacement_y),
        "displacement_mm" => r4(n.displacement_mm),
        "old_clearance_mm" => r4(n.old_clearance_mm),
        "new_clearance_mm" => r4(n.new_clearance_mm),
        "uuid" => n.uuid.as_str(),
    }
}

fn fp_nudge_json(f: &FootprintNudgeResult) -> Json {
    jobj! {
        "reference" => f.reference.as_str(),
        "other_reference" => f.other_reference.as_str(),
        "x" => f.x,
        "y" => f.y,
        "displacement_x_mm" => r4(f.displacement_x),
        "displacement_y_mm" => r4(f.displacement_y),
        "displacement_mm" => r4(f.displacement_mm),
        "old_clearance_mm" => r4(f.old_clearance_mm),
        "new_clearance_mm" => r4(f.new_clearance_mm),
    }
}

fn print_nudge(n: &NudgeResult) {
    println!("\n  [{}] {}", n.object_type.to_uppercase(), n.net_name);
    println!("    Position: ({:.4}, {:.4}) on {}", n.x, n.y, n.layer);
    println!(
        "    Displacement: ({:+.4}, {:+.4}) mm = {:.4} mm",
        n.displacement_x, n.displacement_y, n.displacement_mm
    );
    println!("    Clearance: {:.4} -> {:.4} mm", n.old_clearance_mm, n.new_clearance_mm);
}

fn print_fp_nudge(f: &FootprintNudgeResult) {
    println!("\n  [{}] away from [{}]", f.reference, f.other_reference);
    println!("    Position: ({:.4}, {:.4})", f.x, f.y);
    println!(
        "    Displacement: ({:+.4}, {:+.4}) mm = {:.4} mm",
        f.displacement_x, f.displacement_y, f.displacement_mm
    );
    println!("    Clearance: {:.4} -> {:.4} mm", f.old_clearance_mm, f.new_clearance_mm);
}

fn print_results(r: &RepairResult, format: &str, dry_run: bool, max_d: f64, mfr: Option<&str>) {
    match format {
        "json" => {
            let data = jobj! {
                "dry_run" => dry_run,
                "max_displacement_mm" => max_d,
                "manufacturer" => mfr,
                "total_violations" => Json::Int(r.total_violations),
                "repaired" => Json::Int(r.repaired),
                "skipped" => jobj! {
                    "no_location" => Json::Int(r.skipped_no_location),
                    "no_delta" => Json::Int(r.skipped_no_delta),
                    "exceeds_max" => Json::Int(r.skipped_exceeds_max),
                    "infeasible" => Json::Int(r.skipped_infeasible),
                },
                "nudges" => Json::Arr(r.nudges.iter().map(nudge_json).collect()),
                "footprint_nudges" => Json::Arr(r.footprint_nudge_results.iter().map(fp_nudge_json).collect()),
            };
            println!("{}", dumps_indent(&data, 2));
        }
        "summary" => {
            let action = if dry_run { "Would repair" } else { "Repaired" };
            println!("{action} {}/{} clearance violations", r.repaired, r.total_violations);
            let un = r.total_violations - r.repaired;
            if un > 0 {
                println!("  {un} violations could not be repaired");
            }
        }
        _ => print_text(r, dry_run, max_d, mfr),
    }
}

fn print_text(r: &RepairResult, dry_run: bool, max_d: f64, mfr: Option<&str>) {
    let action = if dry_run { "Would repair" } else { "Repaired" };
    let mfr_str = mfr.map(|m| format!(" (target: {})", m.to_uppercase())).unwrap_or_default();
    let eq = "=".repeat(60);
    let dash = "-".repeat(60);
    println!("\n{eq}");
    println!("CLEARANCE REPAIR{mfr_str}");
    println!("{eq}");
    println!("Max displacement: {}mm", py_float_repr(max_d));
    println!("Mode: {}", if dry_run { "DRY RUN" } else { "APPLY" });
    println!("\n{action} {}/{} clearance violations", r.repaired, r.total_violations);
    if !r.nudges.is_empty() {
        println!("\n{dash}");
        println!("NUDGES:");
        let shown = if r.nudges.len() <= 5 { &r.nudges[..] } else { &r.nudges[..3] };
        shown.iter().for_each(print_nudge);
        if r.nudges.len() > 5 {
            println!("\n  ... and {} more", r.nudges.len() - 3);
        }
    }
    if !r.footprint_nudge_results.is_empty() {
        println!("\n{dash}");
        println!("FOOTPRINT NUDGES:");
        let f = &r.footprint_nudge_results;
        let shown = if f.len() <= 5 { &f[..] } else { &f[..3] };
        shown.iter().for_each(print_fp_nudge);
        if f.len() > 5 {
            println!("\n  ... and {} more", f.len() - 3);
        }
    }
    let skipped = r.skipped_exceeds_max
        + r.skipped_infeasible
        + r.skipped_no_location
        + r.skipped_no_delta
        + r.footprint_skipped_locked
        + r.footprint_skipped_connector
        + r.footprint_skipped_same_component
        + r.footprint_skipped_exceeds_max;
    if skipped > 0 {
        println!("\n{dash}");
        println!("SKIPPED ({skipped}):");
        for (n, label) in [
            (r.skipped_exceeds_max, "Exceeds max displacement"),
            (r.skipped_infeasible, "Infeasible (no movable object)"),
            (r.skipped_no_location, "No location info"),
            (r.skipped_no_delta, "No clearance delta info"),
            (r.footprint_skipped_locked, "Footprint locked"),
            (r.footprint_skipped_connector, "Connector footprint"),
            (r.footprint_skipped_same_component, "Same-component pads"),
            (r.footprint_skipped_exceeds_max, "Footprint exceeds max displacement"),
        ] {
            if n > 0 {
                println!("  {label}: {n}");
            }
        }
    }
    println!("\n{eq}");
    if r.repaired == r.total_violations {
        println!("All clearance violations repaired!");
    } else {
        println!("{} violation(s) require manual repair", r.total_violations - r.repaired);
        if r.skipped_exceeds_max > 0 {
            println!("  Try increasing --max-displacement (currently {}mm)", py_float_repr(max_d));
        }
        if r.skipped_infeasible > 0 {
            println!(
                "  Coincident (0.000mm) copper cannot be nudged apart -- raising --max-displacement \
                 will not help.  Re-route the offending net (dogleg/split off the via center); a \
                 fresh 'kct route --route-engine lattice' run declines such copper (#4318)."
            );
        }
    }
}

pub fn run(argv: Vec<OsString>, g: &Globals) -> Result<i32> {
    let mut args: Args = parse_args("repair-clearance", argv);
    args.quiet |= g.quiet;
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
    let Some(report) = get_drc_report(args.drc_report.as_deref(), pcb_path) else {
        return Ok(1);
    };
    let clearance = report.by_type(ViolationType::CLEARANCE).len();
    let pad_pad = if args.nudge_footprints {
        report.by_type(ViolationType::CLEARANCE_PAD_PAD).len()
    } else {
        0
    };
    if clearance + pad_pad == 0 {
        if !args.quiet {
            println!("No clearance violations found. Nothing to repair.");
        }
        return Ok(0);
    }
    if !args.quiet && args.format == "text" {
        let mut msg = format!("Found {clearance} clearance violation(s) to repair");
        if pad_pad > 0 {
            msg.push_str(&format!(" + {pad_pad} pad-pad violation(s)"));
        }
        println!("{msg}");
    }
    let mut repairer = match ClearanceRepairer::new(pcb_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error loading PCB file: {e}");
            return Ok(1);
        }
    };
    let result = repairer.repair_from_report(
        &report,
        args.max_displacement,
        args.margin,
        &args.prefer,
        args.dry_run,
        false,
        0.5,
        args.nudge_footprints,
    );
    if !args.quiet {
        print_results(&result, &args.format, args.dry_run, args.max_displacement, args.mfr.as_deref());
    }
    if result.repaired > 0 && !args.dry_run {
        let out = args.output.clone().unwrap_or_else(|| args.pcb.clone());
        if let Err(e) = repairer.save(Some(Path::new(&out))) {
            eprintln!("Error saving PCB file: {e}");
            return Ok(1);
        }
        if !args.quiet && args.format == "text" {
            println!("\nSaved to: {}", py_path_str(&out));
        }
    }
    Ok(if result.total_violations - result.repaired > 0 { 1 } else { 0 })
}
