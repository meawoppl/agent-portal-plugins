//! `kct fix-drc` (port of `kicad_tools.cli.fix_drc_cmd`): multi-pass
//! clearance + drill-clearance repair with connectivity-guarded rollback.
//!
//! Exit codes: 0 all repaired, 1 error / no progress, 2 partial repair or
//! non-repairable violations remain (or no write target), 3 connectivity
//! rollback.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;

use super::repair_common::{py_path_str, require_write_target, ALL_MANUFACTURER_NAMES};
use super::runner::{find_kicad_cli, run_drc};
use super::{parse_args, Globals};
use crate::drc::compat::drc_results_to_report;
use crate::drc::repair_clearance::{ClearanceRepairer, RepairResult};
use crate::drc::repair_drill_clearance::{DrillClearanceRepairer, DrillRepairResult};
use crate::drc::violation::{DRCViolation, ViolationType};
use crate::drc::DRCReport;
use crate::jobj;
use crate::manufacturers::DesignRules;
use crate::pyjson::{dumps_indent, py_float_repr, py_repr_str, py_round, Json};
use crate::validate::connectivity::{ConnectivityResult, ConnectivityValidator};

/// Topology advisories excluded from the pure-Rust DRC fallback (#4680).
pub const UNREPAIRABLE_TOPOLOGY_RULE_IDS: &[&str] = &["track_dangling", "via_dangling", "isolated_copper"];

#[derive(Parser, Debug)]
#[command(about = "Automated DRC violation repair (clearance + drill)")]
struct Args {
    /// Path to .kicad_pcb file
    pcb: String,
    /// Path to existing DRC report (.rpt or .json)
    #[arg(long)]
    drc_report: Option<String>,
    /// Target manufacturer for design rules (default: jlcpcb)
    #[arg(short = 'm', long, default_value = "jlcpcb", value_parser = clap::builder::PossibleValuesParser::new(ALL_MANUFACTURER_NAMES))]
    mfr: String,
    /// Number of PCB layers used to derive clearance rules (default: 2)
    #[arg(short = 'l', long, default_value_t = 2)]
    layers: i64,
    /// Maximum nudge/slide distance in mm (default: 0.5)
    #[arg(long, default_value_t = 0.5)]
    max_displacement: f64,
    /// Extra clearance margin beyond minimum in mm (default: 0.01)
    #[arg(long, default_value_t = 0.01)]
    margin: f64,
    /// Only fix a specific violation type
    #[arg(long, value_parser = ["clearance", "drill-clearance"])]
    only: Option<String>,
    /// Output file path (required unless --dry-run or --in-place)
    #[arg(short = 'o', long)]
    output: Option<String>,
    /// Explicitly authorize overwriting the input PCB
    #[arg(long, conflicts_with = "output")]
    in_place: bool,
    /// Preview changes without modifying files
    #[arg(long)]
    dry_run: bool,
    /// Maximum number of detect-repair cycles (default: 1)
    #[arg(long, default_value_t = 1)]
    max_passes: i64,
    /// Attempt local A* rerouting for infeasible violations (default on)
    #[arg(long, overrides_with = "no_local_reroute")]
    local_reroute: bool,
    /// Disable local A* rerouting
    #[arg(long)]
    no_local_reroute: bool,
    /// Skip post-pass connectivity check and rollback
    #[arg(long)]
    no_connectivity_check: bool,
    /// Run pure-Rust DRC before and after repair and report a delta
    #[arg(long)]
    verify: bool,
    /// Suppress progress output (for scripting)
    #[arg(short = 'q', long)]
    quiet: bool,
    /// Output format (default: text)
    #[arg(long, default_value = "text", value_parser = ["text", "json", "summary"])]
    format: String,
    /// Snapshot the output and roll back on failure (exit 1/3)
    #[arg(long)]
    transactional: bool,
}

/// Statistics for one repair pass.
#[derive(Debug, Clone, Default)]
pub struct PassResult {
    pub pass_number: i64,
    pub violations_before: i64,
    pub repaired: i64,
    pub clearance_result: RepairResult,
    pub drill_result: DrillRepairResult,
    pub non_targeted_count: i64,
    pub connectivity_before: Option<i64>,
    pub connectivity_after: Option<i64>,
    pub connectivity_rolled_back: bool,
    pub reverted_uuids: Vec<String>,
    pub connectivity_partial_rollback: bool,
}

impl PassResult {
    pub fn violations_after(&self) -> i64 {
        self.violations_before - self.repaired
    }

    pub fn converged(&self) -> bool {
        self.repaired == 0
    }
}

fn get_drc_report(path: Option<&str>, pcb_path: &Path, mfr: &str, layers: i64) -> Option<DRCReport> {
    if let Some(p) = path {
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
    if let Some(cli) = find_kicad_cli() {
        println!(
            "Running DRC (kicad-cli) on: {}",
            pcb_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        );
        let res = run_drc(pcb_path, None, "json", true, Some(&cli));
        if !res.success {
            eprintln!("Error running DRC: {}", res.stderr);
            return None;
        }
        let out = res.output_path?;
        let report = DRCReport::load(&out);
        let _ = std::fs::remove_file(&out);
        return match report {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("Error loading DRC report: {e}");
                None
            }
        };
    }
    run_native_drc(pcb_path, mfr, layers)
}

/// `_run_python_drc`: the native checker (all categories) as a report.
pub fn run_native_drc(pcb_path: &Path, mfr: &str, layers: i64) -> Option<DRCReport> {
    use crate::validate::checker::{DRCChecker, DRCCheckerOptions};
    let run = || -> Result<DRCReport> {
        let pcb = crate::schema::pcb::Pcb::load(pcb_path)?;
        let checker = DRCChecker::new(
            &pcb,
            DRCCheckerOptions {
                manufacturer: mfr.to_string(),
                layers,
                ..DRCCheckerOptions::default()
            },
        )?;
        let mut results = checker.check_all(false);
        results
            .violations
            .retain(|v| !UNREPAIRABLE_TOPOLOGY_RULE_IDS.contains(&v.rule_id.as_str()));
        Ok(drc_results_to_report(&results.violations, Some(pcb_path)))
    };
    match run() {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("Error running pure-Python DRC: {e}");
            eprintln!("Provide a DRC report with --drc-report, or install KiCad 8.");
            None
        }
    }
}

fn connectivity_report(path: &Path) -> Option<ConnectivityResult> {
    let mut v = ConnectivityValidator::from_path(path).ok()?;
    Some(v.validate(false))
}

fn count_connected_nets(path: &Path) -> i64 {
    connectivity_report(path).map_or(-1, |r| r.connected_nets as i64)
}

fn regressed_nets(before: Option<&ConnectivityResult>, after: Option<&ConnectivityResult>) -> BTreeSet<String> {
    let (Some(b), Some(a)) = (before, after) else {
        return BTreeSet::new();
    };
    let broken = |r: &ConnectivityResult| -> BTreeSet<String> {
        r.issues.iter().filter(|i| !i.net_name.is_empty()).map(|i| i.net_name.clone()).collect()
    };
    broken(a).difference(&broken(b)).cloned().collect()
}

/// `(reverted_all, kept_count, reverted_uuids)`.
fn attempt_granular_rollback(
    output: &Path,
    cr: &RepairResult,
    dr: &DrillRepairResult,
    regressed: &BTreeSet<String>,
    snapshot: &[u8],
    pass_number: i64,
) -> (bool, i64, Vec<String>) {
    let total = (cr.nudges.len() + dr.actions.len()) as i64;
    if total == 0 {
        return (false, 0, Vec::new());
    }
    let off_n: Vec<_> = cr.nudges.iter().filter(|n| regressed.contains(&n.net_name)).collect();
    let off_a: Vec<_> = dr.actions.iter().filter(|a| regressed.contains(&a.net_name)).collect();
    let offenders = (off_n.len() + off_a.len()) as i64;
    if offenders == 0 || offenders == total {
        let _ = std::fs::write(output, snapshot);
        return (true, 0, Vec::new());
    }
    let attempt = || -> Result<Vec<String>> {
        let mut reverted = Vec::new();
        if !off_n.is_empty() {
            let mut rep = ClearanceRepairer::new(output)?;
            for n in &off_n {
                if !rep.undo_nudge(n) {
                    anyhow::bail!(
                        "per-nudge undo failed for {} uuid={} on net {}",
                        n.object_type,
                        py_repr_str(&n.uuid),
                        py_repr_str(&n.net_name)
                    );
                }
                reverted.push(n.uuid.clone());
            }
            rep.save(Some(output))?;
        }
        if !off_a.is_empty() {
            let mut rep = DrillClearanceRepairer::new(output)?;
            for a in &off_a {
                if !rep.undo_action(a) {
                    anyhow::bail!(
                        "per-action undo failed for {} via uuid={} on net {}",
                        a.action,
                        py_repr_str(&a.uuid),
                        py_repr_str(&a.net_name)
                    );
                }
                reverted.push(a.uuid.clone());
            }
            rep.save(Some(output))?;
        }
        Ok(reverted)
    };
    match attempt() {
        Ok(rev) => (false, total - offenders, rev),
        Err(e) => {
            eprintln!(
                "Warning: pass {pass_number} per-nudge rollback failed ({e}); falling back to bulk snapshot restore."
            );
            let _ = std::fs::write(output, snapshot);
            (true, 0, Vec::new())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_single_pass(
    report: &DRCReport,
    pcb_path: &Path,
    output: &Path,
    clearance_violations: usize,
    drill_violations: &[DRCViolation],
    max_d: f64,
    margin: f64,
    dry_run: bool,
    pass_number: i64,
    local_reroute: bool,
    rules: Option<&DesignRules>,
) -> (RepairResult, DrillRepairResult) {
    let mut cr = RepairResult::default();
    let mut dr = DrillRepairResult::default();
    if clearance_violations > 0 {
        let load = if pass_number > 1 { output } else { pcb_path };
        match ClearanceRepairer::new(load) {
            Ok(mut rep) => {
                cr = rep.repair_from_report(report, max_d, margin, "move-trace", dry_run, local_reroute, 0.5, false);
                if cr.repaired > 0 && !dry_run {
                    if let Err(e) = rep.save(Some(output)) {
                        eprintln!("Error during clearance repair: {e}");
                    }
                }
            }
            Err(e) => eprintln!("Error during clearance repair: {e}"),
        }
    }
    if !drill_violations.is_empty() {
        let load = if cr.repaired > 0 && !dry_run { output } else { pcb_path };
        match DrillClearanceRepairer::new(load) {
            Ok(mut rep) => {
                dr = rep.repair(drill_violations, max_d, margin, dry_run, rules);
                if dr.repaired > 0 && !dry_run {
                    if let Err(e) = rep.save(Some(output)) {
                        eprintln!("Error during drill clearance repair: {e}");
                    }
                }
            }
            Err(e) => eprintln!("Error during drill clearance repair: {e}"),
        }
    }
    (cr, dr)
}

pub fn run(argv: Vec<OsString>, g: &Globals) -> Result<i32> {
    let mut args: Args = parse_args("fix-drc", argv);
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
    if args.max_passes < 1 {
        eprintln!("Error: --max-passes must be at least 1");
        return Ok(1);
    }
    let Some(report) = get_drc_report(args.drc_report.as_deref(), &pcb_path, &args.mfr, args.layers) else {
        return Ok(1);
    };
    let output = args.output.clone().map(PathBuf::from).unwrap_or_else(|| pcb_path.clone());
    if !args.transactional {
        return execute_repair(&args, &pcb_path, &output, report);
    }
    let mut txn = crate::transaction::board_transaction([&output], true, true)?;
    let mark = txn.mark();
    let code = execute_repair(&args, &pcb_path, &output, report)?;
    if code == 1 || code == 3 {
        txn.rollback()?;
        for line in txn.report_lines(Some(mark)) {
            eprintln!("{line}");
        }
    }
    txn.commit();
    Ok(code)
}

fn execute_repair(args: &Args, pcb_path: &Path, output: &Path, mut report: DRCReport) -> Result<i32> {
    let max_passes = if args.dry_run { 1 } else { args.max_passes };
    let local_reroute = !args.no_local_reroute;
    let verify_before = if args.verify {
        let r = run_native_drc(pcb_path, &args.mfr, args.layers);
        if let (Some(r), false) = (&r, args.quiet) {
            println!("[verify] Before repair: {} violation(s) via pure-Python DRC", r.violations.len());
        }
        r
    } else {
        None
    };
    let mut passes: Vec<PassResult> = Vec::new();
    let do_conn = !args.dry_run && !args.no_connectivity_check;
    let mut conn_rollback = false;
    let rules = crate::drc::checker::get_design_rules(&args.mfr, args.layers, 1.0).ok();

    for pass_num in 1..=max_passes {
        let do_clearance = args.only.as_deref().is_none_or(|o| o == "clearance");
        let do_drill = args.only.as_deref().is_none_or(|o| o == "drill-clearance");
        let clearance_n = if do_clearance {
            [
                ViolationType::CLEARANCE,
                ViolationType::CLEARANCE_SEGMENT_VIA,
                ViolationType::CLEARANCE_PAD_SEGMENT,
                ViolationType::CLEARANCE_PAD_VIA,
            ]
            .iter()
            .map(|t| report.by_type(*t).len())
            .sum()
        } else {
            0
        };
        let drill: Vec<DRCViolation> = if do_drill {
            let mut v: Vec<DRCViolation> = report.by_type(ViolationType::DRILL_CLEARANCE).into_iter().cloned().collect();
            v.extend(report.by_type(ViolationType::HOLE_NEAR_HOLE).into_iter().cloned());
            v
        } else {
            Vec::new()
        };
        let total_targeted = (clearance_n + drill.len()) as i64;
        let non_targeted = report.violations.len() as i64 - total_targeted;
        if total_targeted == 0 {
            if pass_num == 1 {
                if non_targeted > 0 {
                    if !args.quiet {
                        println!(
                            "No repairable violations found, but {non_targeted} non-repairable violation(s) detected (edge clearance, dimension, silkscreen, etc.)."
                        );
                    }
                    return Ok(2);
                }
                if !args.quiet {
                    println!("No targeted violations found. Nothing to repair.");
                }
                return Ok(0);
            }
            break;
        }
        if !args.quiet && args.format == "text" && pass_num == 1 {
            println!("Found {total_targeted} repairable violation(s) (kicad-cli engine):");
            if clearance_n > 0 {
                println!("  Clearance: {clearance_n}");
            }
            if !drill.is_empty() {
                println!("  Drill clearance: {}", drill.len());
            }
            if non_targeted > 0 {
                println!(
                    "Also found {non_targeted} non-repairable violation(s) (edge clearance, dimension, silkscreen, etc.)"
                );
            }
        }
        let mut snapshot: Option<Vec<u8>> = None;
        let mut baseline: Option<i64> = None;
        let mut baseline_report = None;
        if do_conn {
            let load = if pass_num > 1 { output } else { pcb_path };
            if load.exists() {
                snapshot = std::fs::read(load).ok();
                baseline = Some(count_connected_nets(load));
                baseline_report = connectivity_report(load);
            }
        }
        let (cr, dr) = run_single_pass(
            &report,
            pcb_path,
            output,
            clearance_n,
            &drill,
            args.max_displacement,
            args.margin,
            args.dry_run,
            pass_num,
            local_reroute,
            rules.as_ref(),
        );
        let mut repaired = cr.repaired + dr.repaired;
        let mut after: Option<i64> = None;
        let mut rolled_back = false;
        let mut partial = false;
        let mut reverted: Vec<String> = Vec::new();
        if let (Some(snap), Some(base)) = (&snapshot, baseline) {
            if base >= 0 && !args.dry_run && repaired > 0 && output.exists() {
                let a = count_connected_nets(output);
                after = Some(a);
                if a >= 0 && a < base {
                    let after_report = connectivity_report(output);
                    let regressed = regressed_nets(baseline_report.as_ref(), after_report.as_ref());
                    let (all, kept, uuids) = attempt_granular_rollback(output, &cr, &dr, &regressed, snap, pass_num);
                    if all {
                        rolled_back = true;
                        conn_rollback = true;
                        repaired = 0;
                        eprintln!("Warning: pass {pass_num} decreased connectivity ({base} -> {a} nets); rolled back.");
                    } else {
                        partial = true;
                        let n = uuids.len();
                        reverted = uuids;
                        repaired = kept;
                        let new_after = count_connected_nets(output);
                        if new_after >= 0 {
                            after = Some(new_after);
                        }
                        let nets: Vec<String> = regressed.iter().map(|s| py_repr_str(s)).collect();
                        eprintln!(
                            "Warning: pass {pass_num} initially decreased connectivity ({base} -> {a} nets); reverted {n} of {} nudge(s) on regressed net(s) [{}]; now {} connected net(s).",
                            n as i64 + kept,
                            nets.join(", "),
                            after.unwrap_or(a)
                        );
                    }
                }
            }
        }
        passes.push(PassResult {
            pass_number: pass_num,
            violations_before: total_targeted,
            repaired,
            clearance_result: cr,
            drill_result: dr,
            non_targeted_count: non_targeted,
            connectivity_before: baseline,
            connectivity_after: after,
            connectivity_rolled_back: rolled_back,
            reverted_uuids: reverted,
            connectivity_partial_rollback: partial,
        });
        if rolled_back || repaired == 0 {
            break;
        }
        if pass_num < max_passes {
            match run_native_drc(output, &args.mfr, args.layers) {
                Some(r) => report = r,
                None => break,
            }
        }
    }

    if !args.quiet {
        print_results(&passes, &args.format, args.dry_run, args.max_displacement, args.max_passes);
    }
    if args.verify && !args.dry_run {
        if let (Some(before), Some(after)) = (&verify_before, run_native_drc(output, &args.mfr, args.layers)) {
            let (b, a) = (before.violations.len() as i64, after.violations.len() as i64);
            let delta = b - a;
            if !args.quiet {
                let eq = "=".repeat(60);
                println!("\n{eq}");
                println!("VERIFICATION (pure-Python DRC)");
                println!("{eq}");
                println!("  Before repair: {b} violation(s)");
                println!("  After repair:  {a} violation(s)");
                if delta > 0 {
                    println!("  Resolved:      {delta}");
                } else if delta == 0 {
                    println!("  No change in violation count.");
                } else {
                    println!("  WARNING: {} new violation(s) introduced!", -delta);
                }
                println!("\nThese counts use the same engine as `kct check` for consistent comparison.");
            }
        }
    }
    if conn_rollback {
        return Ok(3);
    }
    let Some(last) = passes.last() else {
        return Ok(0);
    };
    let remaining = last.violations_before - last.repaired + last.non_targeted_count;
    if remaining == 0 {
        return Ok(0);
    }
    if last.repaired == 0 && last.non_targeted_count == 0 {
        return Ok(1);
    }
    Ok(2)
}

struct Effective {
    rolled_back: bool,
    partial: bool,
    reverted: BTreeSet<String>,
    clearance_reverted: i64,
    drill_reverted: i64,
}

fn effective(last: Option<&PassResult>) -> Effective {
    let rolled_back = last.is_some_and(|l| l.connectivity_rolled_back);
    let partial = last.is_some_and(|l| l.connectivity_partial_rollback);
    let reverted: BTreeSet<String> = last.map(|l| l.reverted_uuids.iter().cloned().collect()).unwrap_or_default();
    let (cr, dr) = last.map_or((0, 0), |l| {
        (
            l.clearance_result.nudges.iter().filter(|n| reverted.contains(&n.uuid)).count() as i64,
            l.drill_result.actions.iter().filter(|a| reverted.contains(&a.uuid)).count() as i64,
        )
    });
    Effective { rolled_back, partial, reverted, clearance_reverted: cr, drill_reverted: dr }
}

fn print_results(passes: &[PassResult], format: &str, dry_run: bool, max_d: f64, max_passes: i64) {
    match format {
        "json" => print_json(passes, dry_run, max_d, max_passes),
        "summary" => print_summary(passes, dry_run),
        _ => print_text(passes, dry_run, max_d),
    }
}

fn print_json(passes: &[PassResult], dry_run: bool, max_d: f64, max_passes: i64) {
    let last = passes.last();
    let default = PassResult::default();
    let l = last.unwrap_or(&default);
    let (cr, dr) = (&l.clearance_result, &l.drill_result);
    let e = effective(last);
    let total_repaired_all: i64 = passes.iter().map(|p| p.repaired).sum();
    let (total_violations, total_repaired) = if passes.len() == 1 {
        let tv = cr.total_violations + dr.total_violations;
        let tr = if e.rolled_back {
            0
        } else if e.partial {
            (cr.repaired - e.clearance_reverted) + (dr.repaired - e.drill_reverted)
        } else {
            cr.repaired + dr.repaired
        };
        (tv, tr)
    } else {
        (passes.first().map_or(0, |p| p.violations_before), total_repaired_all)
    };
    let (ecr, edr) = if e.rolled_back {
        (0, 0)
    } else if e.partial {
        (cr.repaired - e.clearance_reverted, dr.repaired - e.drill_reverted)
    } else {
        (cr.repaired, dr.repaired)
    };
    let mut data = jobj! {
        "dry_run" => dry_run,
        "max_displacement_mm" => max_d,
        "total_violations" => Json::Int(total_violations),
        "total_repaired" => Json::Int(total_repaired),
        "non_targeted_violations" => Json::Int(l.non_targeted_count),
        "clearance" => jobj! {
            "violations" => Json::Int(cr.total_violations),
            "repaired" => Json::Int(ecr),
            "relocated_vias" => Json::Int(cr.relocated_vias),
            "endpoint_nudges" => Json::Int(cr.endpoint_nudges),
            "local_rerouted" => Json::Int(cr.local_rerouted),
            "cluster_rerouted" => Json::Int(cr.cluster_rerouted),
            "skipped" => jobj! {
                "no_location" => Json::Int(cr.skipped_no_location),
                "no_delta" => Json::Int(cr.skipped_no_delta),
                "exceeds_max" => Json::Int(cr.skipped_exceeds_max),
                "infeasible" => Json::Int(cr.skipped_infeasible),
                "no_local_route" => Json::Int(cr.skipped_no_local_route),
            },
            "nudges" => Json::Arr(cr.nudges.iter().map(|n| jobj! {
                "object_type" => n.object_type.as_str(),
                "x" => n.x,
                "y" => n.y,
                "net_name" => n.net_name.as_str(),
                "displacement_mm" => py_round(n.displacement_mm, 4),
                "reverted" => e.rolled_back || e.reverted.contains(&n.uuid),
            }).collect()),
        },
        "drill_clearance" => jobj! {
            "violations" => Json::Int(dr.total_violations),
            "repaired" => Json::Int(edr),
            "deduplicated" => Json::Int(dr.deduplicated),
            "slid" => Json::Int(dr.slid),
            "skipped" => jobj! {
                "no_location" => Json::Int(dr.skipped_no_location),
                "no_delta" => Json::Int(dr.skipped_no_delta),
                "exceeds_max" => Json::Int(dr.skipped_exceeds_max),
                "infeasible" => Json::Int(dr.skipped_infeasible),
            },
            "actions" => Json::Arr(dr.actions.iter().map(|a| jobj! {
                "action" => a.action.as_str(),
                "via_x" => a.via_x,
                "via_y" => a.via_y,
                "net_name" => a.net_name.as_str(),
                "displacement_mm" => py_round(a.displacement_mm, 4),
                "detail" => a.detail.as_str(),
                "reverted" => e.rolled_back || e.reverted.contains(&a.uuid),
            }).collect()),
        },
    };
    let opt = |v: Option<i64>| v.map_or(Json::Null, Json::Int);
    let conn: Vec<Json> = passes
        .iter()
        .filter(|p| p.connectivity_before.is_some() || p.connectivity_after.is_some())
        .map(|p| {
            jobj! {
                "pass" => Json::Int(p.pass_number),
                "connected_nets_before" => opt(p.connectivity_before),
                "connected_nets_after" => opt(p.connectivity_after),
                "rolled_back" => p.connectivity_rolled_back,
                "partial_rollback" => p.connectivity_partial_rollback,
                "reverted_uuids" => Json::Arr(p.reverted_uuids.iter().map(|u| Json::Str(u.clone())).collect()),
            }
        })
        .collect();
    if !conn.is_empty() {
        data.set("connectivity_check", jobj! {"passes" => Json::Arr(conn)});
    }
    if max_passes > 1 {
        let ps: Vec<Json> = passes
            .iter()
            .map(|p| {
                let mut o = jobj! {
                    "pass" => Json::Int(p.pass_number),
                    "violations_before" => Json::Int(p.violations_before),
                    "repaired" => Json::Int(p.repaired),
                    "violations_after" => Json::Int(p.violations_after()),
                };
                if p.connectivity_before.is_some() {
                    o.set("connectivity_rolled_back", p.connectivity_rolled_back);
                }
                o
            })
            .collect();
        data.set("passes", Json::Arr(ps));
    }
    println!("{}", dumps_indent(&data, 2));
}

fn pass_line(p: &PassResult) {
    if p.converged() {
        println!("  Pass {}: {} -> {} (converged)", p.pass_number, p.violations_before, p.violations_before);
    } else {
        println!("  Pass {}: {} -> {} (-{})", p.pass_number, p.violations_before, p.violations_after(), p.repaired);
    }
}

fn print_summary(passes: &[PassResult], dry_run: bool) {
    let action = if dry_run { "Would repair" } else { "Repaired" };
    let last = passes.last();
    let non_targeted = last.map_or(0, |l| l.non_targeted_count);
    if passes.len() <= 1 {
        let default = PassResult::default();
        let l = last.unwrap_or(&default);
        let (cr, dr) = (&l.clearance_result, &l.drill_result);
        let tv = cr.total_violations + dr.total_violations;
        let e = effective(last);
        let (c, d) = if e.rolled_back {
            (0, 0)
        } else if e.partial {
            (cr.repaired - e.clearance_reverted, dr.repaired - e.drill_reverted)
        } else {
            (cr.repaired, dr.repaired)
        };
        println!("{action} {}/{tv} DRC violations", c + d);
        if cr.total_violations > 0 {
            println!("  Clearance: {c}/{}", cr.total_violations);
        }
        if dr.total_violations > 0 {
            println!("  Drill clearance: {d}/{}", dr.total_violations);
        }
    } else {
        let total: i64 = passes.iter().map(|p| p.repaired).sum();
        println!("{action} {total}/{} DRC violations", passes[0].violations_before);
        passes.iter().for_each(pass_line);
    }
    if non_targeted > 0 {
        println!("  Non-repairable: {non_targeted} (edge clearance, dimension, silkscreen, etc.)");
    }
}

fn print_text(passes: &[PassResult], dry_run: bool, max_d: f64) {
    let total_repaired: i64 = passes.iter().map(|p| p.repaired).sum();
    let first = passes.first().map_or(0, |p| p.violations_before);
    let last = passes.last();
    let default = PassResult::default();
    let l = last.unwrap_or(&default);
    let (cr, dr) = (&l.clearance_result, &l.drill_result);
    let action = if dry_run { "Would repair" } else { "Repaired" };
    let e = effective(last);
    let is_rev = |u: &str| e.rolled_back || (e.partial && e.reverted.contains(u));
    let eq = "=".repeat(60);
    let dash = "-".repeat(60);
    println!("\n{eq}");
    println!("DRC VIOLATION REPAIR");
    println!("{eq}");
    println!("Max displacement: {}mm", py_float_repr(max_d));
    println!("Mode: {}", if dry_run { "DRY RUN" } else { "APPLY" });
    if passes.len() > 1 {
        passes.iter().for_each(pass_line);
    }
    let suffix = if e.rolled_back {
        " (rolled back -- connectivity regression)".to_string()
    } else if e.partial {
        format!(
            " (partial rollback -- {} nudge(s) reverted on regressed nets)",
            e.clearance_reverted + e.drill_reverted
        )
    } else {
        String::new()
    };
    println!("\n{action} {total_repaired}/{first} violations{suffix}");
    if (e.rolled_back || e.partial) && last.is_some() {
        if let (Some(b), Some(a)) = (l.connectivity_before, l.connectivity_after) {
            if e.rolled_back {
                println!("  Connectivity: {b} -> {a} connected nets; nudges reverted to preserve connectivity.");
            } else {
                println!("  Connectivity: {b} -> {a} connected nets after granular rollback.");
            }
        }
    }
    if cr.total_violations > 0 {
        println!("\n{dash}");
        let (eff, hs) = if e.rolled_back {
            (0, " (reverted)".to_string())
        } else if e.partial {
            (
                cr.repaired - e.clearance_reverted,
                if e.clearance_reverted > 0 { format!(" ({} reverted)", e.clearance_reverted) } else { String::new() },
            )
        } else {
            (cr.repaired, String::new())
        };
        println!("CLEARANCE: {eff}/{}{hs}", cr.total_violations);
        for n in cr.nudges.iter().take(5) {
            let s = if is_rev(&n.uuid) { " (reverted)" } else { "" };
            println!("  [{}] {}{s}", n.object_type.to_uppercase(), n.net_name);
            println!("    at ({:.4}, {:.4}) -> {:.4}mm", n.x, n.y, n.displacement_mm);
        }
        if cr.nudges.len() > 5 {
            println!("  ... and {} more", cr.nudges.len() - 5);
        }
    }
    if dr.total_violations > 0 {
        println!("\n{dash}");
        let (eff, hs) = if e.rolled_back {
            (0, " (reverted)".to_string())
        } else if e.partial {
            (
                dr.repaired - e.drill_reverted,
                if e.drill_reverted > 0 { format!(" ({} reverted)", e.drill_reverted) } else { String::new() },
            )
        } else {
            (dr.repaired, String::new())
        };
        println!("DRILL CLEARANCE: {eff}/{}{hs}", dr.total_violations);
        if !e.rolled_back {
            if dr.deduplicated > 0 {
                println!("  De-duplicated: {}", dr.deduplicated);
            }
            if dr.slid > 0 {
                println!("  Slid apart: {}", dr.slid);
            }
        }
        for a in dr.actions.iter().take(5) {
            let s = if is_rev(&a.uuid) { " (reverted)" } else { "" };
            println!("  [{}] {}{s}", a.action.to_uppercase(), a.net_name);
            println!("    at ({:.4}, {:.4}) - {}", a.via_x, a.via_y, a.detail);
        }
        if dr.actions.len() > 5 {
            println!("  ... and {} more", dr.actions.len() - 5);
        }
    }
    let exceeds = cr.skipped_exceeds_max + dr.skipped_exceeds_max;
    let infeasible = cr.skipped_infeasible + dr.skipped_infeasible;
    let no_loc = cr.skipped_no_location + dr.skipped_no_location;
    let no_delta = cr.skipped_no_delta + dr.skipped_no_delta;
    let skipped = exceeds + infeasible + no_loc + no_delta;
    if skipped > 0 {
        println!("\n{dash}");
        println!("SKIPPED ({skipped}):");
        for (n, label) in [
            (exceeds, "Exceeds max displacement"),
            (infeasible, "Infeasible"),
            (no_loc, "No location info"),
            (no_delta, "No clearance delta info"),
        ] {
            if n > 0 {
                println!("  {label}: {n}");
            }
        }
    }
    println!("\n{eq}");
    let non_targeted = l.non_targeted_count;
    let remaining = first - total_repaired;
    if remaining <= 0 && non_targeted == 0 {
        println!("All violations repaired!");
    } else if remaining <= 0 {
        println!("All repairable violations repaired!");
        println!("{non_targeted} non-repairable violation(s) remain (edge clearance, dimension, silkscreen, etc.)");
    } else {
        println!("{} violation(s) remain:", remaining + non_targeted);
        if remaining > 0 {
            println!("  Repairable (not yet fixed): {remaining}");
            if exceeds > 0 {
                println!("    Try increasing --max-displacement (currently {}mm)", py_float_repr(max_d));
            }
        }
        if non_targeted > 0 {
            println!("  Non-repairable: {non_targeted} (edge clearance, dimension, silkscreen, etc.)");
        }
    }
}
