//! `kct place-silk-refs` (port of `kicad_tools.cli.place_silk_refs_cmd`):
//! move readable silkscreen reference designators to clear collisions,
//! driven by [`crate::silkscreen::place_refs::SilkRefPlacer`].
//!
//! kicadmium design-edit policy: writing requires `-o/--output` or an
//! explicit `--in-place` (upstream overwrites the input by default).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;

use super::repair_common::{py_path_str, require_write_target, ALL_MANUFACTURER_NAMES};
use super::{parse_args, Globals};
use crate::pyjson::{dumps_indent, Json};
use crate::silkscreen::place_refs::{PlaceSilkRefsResult, PlanOptions, SilkRefPlacer};
use crate::silkscreen::silk_defaults::{
    DEFAULT_CLEARANCE_MM, DEFAULT_MAX_OFFSET_MM, DEFAULT_STEP_MM, SILK_EDGE_CLEARANCE_MM,
};

#[derive(Parser)]
#[command(about = "Move readable silkscreen reference designators to clear collisions")]
struct Args {
    /// Path to .kicad_pcb file
    pcb: String,
    /// Manufacturer to source solder-mask clearance from (default: built-in 0.05mm)
    #[arg(long, value_parser = ALL_MANUFACTURER_NAMES.to_vec())]
    mfr: Option<String>,
    /// Number of PCB layers (default: 2)
    #[arg(long, default_value_t = 2)]
    layers: u8,
    /// Outer copper weight in oz (default: 1.0)
    #[arg(long, default_value_t = 1.0)]
    copper: f64,
    /// Required silk-to-pad/silk-to-silk clearance in mm (default: 0.15)
    #[arg(long, default_value_t = DEFAULT_CLEARANCE_MM)]
    clearance: f64,
    /// Required silk-to-board-edge clearance in mm (default: 0.2)
    #[arg(long = "edge-clearance", default_value_t = SILK_EDGE_CLEARANCE_MM)]
    edge_clearance: f64,
    /// Maximum search distance from the component body in mm (default: 8.0)
    #[arg(long = "max-offset", default_value_t = DEFAULT_MAX_OFFSET_MM)]
    max_offset: f64,
    /// Positive search ring spacing in mm; at most 4096 rings (default: 0.25)
    #[arg(long, default_value_t = DEFAULT_STEP_MM)]
    step: f64,
    /// Also try a 90-degree rotated orientation when the original does not fit
    #[arg(long = "allow-rotate")]
    allow_rotate: bool,
    /// Output file path (required unless --dry-run or --in-place)
    #[arg(short = 'o', long)]
    output: Option<String>,
    /// Explicitly authorize overwriting the input PCB
    #[arg(long, conflicts_with = "output")]
    in_place: bool,
    /// Preview the move plan without modifying files
    #[arg(long)]
    dry_run: bool,
    /// After applying, run native DRC; fail on silk findings or unavailable/failed verification
    #[arg(long = "verify-drc")]
    verify_drc: bool,
    /// Write a rendered review artifact (SVG) of old/new reference positions
    #[arg(long, value_name = "SVG_PATH")]
    render: Option<String>,
    /// Output format (default: text)
    #[arg(long, default_value = "text", value_parser = ["text", "json", "summary"])]
    format: String,
    /// Suppress progress output (for scripting)
    #[arg(short, long)]
    quiet: bool,
}

/// `_get_mask_clearance`.
fn mask_clearance(mfr: Option<&str>, layers: u8, copper: f64) -> f64 {
    let Some(m) = mfr else {
        return 0.05;
    };
    let id = crate::manufacturers::canonical_id(m);
    match crate::manufacturers::design_rules(&id) {
        Ok(all) => {
            let key = format!("{layers}layer_{}oz", copper.trunc() as i64);
            all.get(&key)
                .or_else(|| all.get(&format!("{layers}layer_1oz")))
                .or_else(|| all.values().next())
                .map_or(0.05, |r| r.min_solder_mask_clearance_mm)
        }
        Err(_) => {
            eprintln!("Warning: No configuration found for manufacturer '{m}'");
            0.05
        }
    }
}

fn drc_summary_json(d: &DrcSummary) -> Json {
    let mut o = Json::obj();
    o.set("available", d.available);
    if let Some(m) = &d.message {
        o.set("message", m.as_str());
    }
    if let Some(e) = &d.error {
        o.set("error", e.as_str());
    }
    if let Some((total, silk)) = d.counts {
        o.set("total_violations", total as i64);
        o.set("silk_violations", silk as i64);
    }
    o
}

struct DrcSummary {
    available: bool,
    message: Option<String>,
    error: Option<String>,
    counts: Option<(usize, usize)>,
}

impl DrcSummary {
    fn failed(&self) -> bool {
        !self.available || self.error.is_some() || self.counts.is_some_and(|c| c.1 > 0)
    }
}

/// `_run_verify_drc`.
fn run_verify_drc(pcb_path: &Path) -> DrcSummary {
    use super::runner::{find_kicad_cli, run_drc};
    let fail = |e: String| DrcSummary {
        available: true,
        message: None,
        error: Some(e),
        counts: None,
    };
    if find_kicad_cli().is_none() {
        return DrcSummary {
            available: false,
            message: Some("kicad-cli not found; skipped native DRC verification".into()),
            error: None,
            counts: None,
        };
    }
    let res = run_drc(pcb_path, None, "json", false, None);
    let out = res.output_path.clone();
    let summary = (|| {
        let Some(path) = out.as_ref().filter(|_| res.success && res.return_code == 0) else {
            return fail(if res.stderr.is_empty() {
                "DRC run failed".into()
            } else {
                res.stderr.clone()
            });
        };
        let raw: serde_json::Value = match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
        {
            Ok(v) => v,
            Err(e) => return fail(format!("Invalid native DRC report: {e}")),
        };
        if !raw.get("violations").is_some_and(|v| v.is_array()) {
            return fail("Invalid native DRC report: missing violations array".into());
        }
        let report = match crate::drc::DRCReport::load(path) {
            Ok(r) => r,
            Err(e) => return fail(format!("Invalid native DRC report: {e}")),
        };
        const SILK: [&str; 4] = [
            "silk_over_copper",
            "silk_overlap",
            "silk_edge_clearance",
            "silkscreen",
        ];
        let silk = report
            .violations
            .iter()
            .filter(|v| SILK.iter().any(|t| v.type_str.contains(t)))
            .count();
        DrcSummary {
            available: true,
            message: None,
            error: None,
            counts: Some((report.violation_count(), silk)),
        }
    })();
    if let Some(p) = out {
        let _ = std::fs::remove_file(p);
    }
    summary
}

fn by_ref(
    mut v: Vec<&crate::silkscreen::place_refs::RefPlacement>,
) -> Vec<&crate::silkscreen::place_refs::RefPlacement> {
    v.sort_by(|a, b| a.footprint_ref.cmp(&b.footprint_ref));
    v
}

fn print_text(r: &PlaceSilkRefsResult, dry_run: bool) {
    if r.placements.is_empty() {
        println!("No visible reference designators found.");
        return;
    }
    let action = if dry_run { "Would move" } else { "Moved" };
    let (unplaceable, fallback) = (r.unplaceable(), r.under_component_fallback());
    if !r.moved().is_empty() {
        println!("{action} {} reference designator(s):", r.total_moved());
        for p in by_ref(r.moved()) {
            let rot = if p.new_rotation != p.old_rotation {
                format!(", {:.0}deg -> {:.0}deg", p.old_rotation, p.new_rotation)
            } else {
                String::new()
            };
            println!(
                "  {}: ({:.3}, {:.3}) -> ({:.3}, {:.3}){rot}",
                p.footprint_ref,
                p.old_position.0,
                p.old_position.1,
                p.new_position.0,
                p.new_position.1
            );
        }
    } else if unplaceable.is_empty() && fallback.is_empty() {
        println!("No collisions found -- every visible reference is already clear.");
    } else {
        println!("No references could be moved to a clear location.");
    }
    if !fallback.is_empty() {
        println!(
            "\n{} reference(s) fell back under their own component body:",
            fallback.len()
        );
        for p in by_ref(fallback) {
            println!("  {}: {}", p.footprint_ref, p.reason);
        }
    }
    if !unplaceable.is_empty() {
        println!(
            "\n{} reference(s) could not be placed cleanly:",
            unplaceable.len()
        );
        for p in by_ref(unplaceable) {
            println!("  {}: {}", p.footprint_ref, p.reason);
        }
    }
}

pub fn run(argv: Vec<OsString>, g: &Globals) -> Result<i32> {
    let mut args: Args = parse_args("place-silk-refs", argv);
    args.quiet |= g.quiet;
    if let Some(code) = require_write_target(args.dry_run, args.output.as_deref(), args.in_place) {
        return Ok(code);
    }
    let pcb_path = PathBuf::from(&args.pcb);
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
    let mask = mask_clearance(args.mfr.as_deref(), args.layers, args.copper);
    let mut placer = match SilkRefPlacer::new(&pcb_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error parsing PCB file: {e:#}");
            return Ok(1);
        }
    };
    let result = match placer.plan(&PlanOptions {
        clearance_mm: args.clearance,
        edge_clearance_mm: args.edge_clearance,
        mask_clearance_mm: mask,
        max_offset_mm: args.max_offset,
        step_mm: args.step,
        allow_rotate: args.allow_rotate,
    }) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e:#}");
            return Ok(1);
        }
    };
    let output_str = args.output.clone().unwrap_or_else(|| args.pcb.clone());
    let output_path = PathBuf::from(&output_str);
    let mut drc: Option<DrcSummary> = None;
    if !args.dry_run {
        let applied = placer.apply(&result)?;
        if applied > 0 || args.output.is_some() {
            if let Err(e) = placer.save(Some(&output_path)) {
                eprintln!("Error saving PCB file: {e:#}");
                return Ok(1);
            }
        }
        if args.verify_drc {
            drc = Some(run_verify_drc(&output_path));
        }
    }
    let render_path = match &args.render {
        Some(p) => Some(placer.render_svg(&result, Path::new(p))?),
        None => None,
    };
    let render_str = args.render.as_deref().map(py_path_str);

    if !args.quiet {
        match args.format.as_str() {
            "json" => {
                let mut d = Json::obj();
                d.set("clearance_mm", Json::Float(result.clearance_mm));
                d.set("dry_run", args.dry_run);
                d.set("total_moved", result.total_moved() as i64);
                d.set("total_unchanged", result.unchanged().len() as i64);
                d.set("total_unplaceable", result.unplaceable().len() as i64);
                d.set(
                    "total_under_component_fallback",
                    result.under_component_fallback().len() as i64,
                );
                d.set(
                    "placements",
                    Json::Arr(result.placements.iter().map(|p| p.to_dict()).collect()),
                );
                if let Some(s) = &drc {
                    d.set("drc_verification", drc_summary_json(s));
                }
                if let Some(r) = &render_str {
                    d.set("render_artifact", r.as_str());
                }
                println!("{}", dumps_indent(&d, 2));
            }
            "summary" => {
                let action = if args.dry_run { "Would move" } else { "Moved" };
                let mut parts = vec![format!("{action} {} reference(s)", result.total_moved())];
                if !result.unplaceable().is_empty() {
                    parts.push(format!("{} unplaceable", result.unplaceable().len()));
                }
                if !result.under_component_fallback().is_empty() {
                    parts.push(format!(
                        "{} under-component fallback",
                        result.under_component_fallback().len()
                    ));
                }
                println!(
                    "{} (clearance: {:.2}mm)",
                    parts.join("; "),
                    result.clearance_mm
                );
            }
            _ => print_text(&result, args.dry_run),
        }
        if args.format != "json" {
            if let Some(s) = &drc {
                if !s.available {
                    println!(
                        "Native DRC: {}",
                        s.message.as_deref().unwrap_or("unavailable")
                    );
                } else if let Some(e) = &s.error {
                    println!("Native DRC error: {e}");
                } else {
                    println!(
                        "Native DRC: {} silk violation(s)",
                        s.counts.map_or(0, |c| c.1)
                    );
                }
            }
        }
        if !args.dry_run && result.total_moved() > 0 && args.format == "text" {
            println!("\nSaved to: {}", py_path_str(&output_str));
        }
        if render_path.is_some() && args.format == "text" {
            println!("Review artifact: {}", render_str.as_deref().unwrap_or(""));
        }
    }
    Ok(if drc.as_ref().is_some_and(DrcSummary::failed) {
        1
    } else {
        0
    })
}
