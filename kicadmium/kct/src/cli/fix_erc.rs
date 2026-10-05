//! Conservative automated repair for the small ERC subset with explicit intent.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Serialize;

use super::{parse_args, Globals};
use crate::erc::report::ERCReport;
use crate::erc::violation::ERCViolationType;
use crate::schema::schematic::Schematic;
use crate::schema::symbol::new_uuid;

#[derive(Parser)]
#[command(about = "Repair PWR_FLAG, no-connect, and dangling-wire ERC findings")]
struct Args {
    schematic: PathBuf,
    #[arg(long = "erc-report")]
    erc_report: Option<PathBuf>,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, default_value = "text", value_parser = ["text", "json", "summary"])]
    format: String,
}

#[derive(Serialize)]
struct Action {
    violation_type: String,
    action: &'static str,
    x: f64,
    y: f64,
    description: String,
}

fn generated_report(schematic: &Path) -> Result<(ERCReport, Option<PathBuf>)> {
    let mut report_path = std::env::temp_dir();
    report_path.push(format!("kicadmium-erc-{}.json", new_uuid()));
    let cli = std::env::var_os("KICADMIUM_KICAD_CLI")
        .or_else(|| std::env::var_os("KICAD_CLI"))
        .unwrap_or_else(|| "kicad-cli".into());
    let result = std::process::Command::new(cli)
        .args(["sch", "erc", "--format", "json", "--output"])
        .arg(&report_path)
        .arg(schematic)
        .output()
        .context("launch kicad-cli for ERC")?;
    if !report_path.exists() {
        bail!(
            "kicad-cli ERC did not produce a report: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    let report = ERCReport::load(&report_path)?;
    Ok((report, Some(report_path)))
}

fn at_key(x: f64, y: f64) -> (i64, i64) {
    ((x * 100.0).round() as i64, (y * 100.0).round() as i64)
}

fn insert_no_connect(schematic: &mut Schematic, x: f64, y: f64) -> Result<()> {
    let node = crate::parse(&format!(
        "(no_connect (at {x} {y}) (uuid \"{}\"))",
        new_uuid()
    ))?;
    schematic.sexp_mut().push(node);
    Ok(())
}

fn remove_wire_at(schematic: &mut Schematic, x: f64, y: f64) -> usize {
    let point = (x, y);
    let mut removed = 0;
    schematic.sexp_mut().retain(|node| {
        if !node.has_tag("wire") {
            return true;
        }
        let Some(pts) = node.find_child("pts") else {
            return true;
        };
        let matches = pts.children_named("xy").any(|xy| {
            xy.float_at(0)
                .zip(xy.float_at(1))
                .is_some_and(|p| (p.0 - point.0).abs() <= 0.01 && (p.1 - point.1).abs() <= 0.01)
        });
        if matches {
            removed += 1;
        }
        !matches
    });
    removed
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("fix-erc", args);
    if args.schematic.extension().and_then(|s| s.to_str()) != Some("kicad_sch") {
        bail!("expected a .kicad_sch file: {}", args.schematic.display());
    }
    if !args.dry_run && args.output.is_none() {
        bail!("fix-erc is a design edit; pass --output (or use --dry-run)");
    }
    let (report, temporary) = match &args.erc_report {
        Some(path) => (ERCReport::load(path)?, None),
        None => generated_report(&args.schematic)?,
    };
    let mut schematic = Schematic::load(&args.schematic)?;
    let mut flags: BTreeSet<_> = schematic
        .symbols()
        .iter()
        .filter(|s| s.lib_id == "power:PWR_FLAG")
        .map(|s| at_key(s.position.0, s.position.1))
        .collect();
    let mut no_connects: BTreeSet<_> = schematic
        .no_connects()
        .iter()
        .map(|n| at_key(n.position.0, n.position.1))
        .collect();
    let mut wire_positions = BTreeSet::new();
    let mut actions = Vec::new();
    let mut duplicates = 0usize;
    let mut skipped = 0usize;
    for violation in report.violations.iter().filter(|v| !v.excluded) {
        match violation.vtype {
            ERCViolationType::POWER_PIN_NOT_DRIVEN => {
                let (x, y) = (violation.pos_x, violation.pos_y + 2.54);
                if !flags.insert(at_key(x, y)) {
                    duplicates += 1;
                    continue;
                }
                if !args.dry_run {
                    schematic.add_power("PWR_FLAG", (x, y), 0.0, "", "/")?;
                }
                actions.push(Action {
                    violation_type: violation.type_str.clone(),
                    action: "insert_pwr_flag",
                    x,
                    y,
                    description: violation.description.clone(),
                });
            }
            ERCViolationType::PIN_NOT_CONNECTED => {
                let (x, y) = (violation.pos_x, violation.pos_y);
                if !no_connects.insert(at_key(x, y)) {
                    duplicates += 1;
                    continue;
                }
                if !args.dry_run {
                    insert_no_connect(&mut schematic, x, y)?;
                }
                actions.push(Action {
                    violation_type: violation.type_str.clone(),
                    action: "insert_no_connect",
                    x,
                    y,
                    description: violation.description.clone(),
                });
            }
            ERCViolationType::WIRE_DANGLING | ERCViolationType::UNCONNECTED_WIRE_ENDPOINT => {
                let (x, y) = (violation.pos_x, violation.pos_y);
                if !wire_positions.insert(at_key(x, y)) {
                    duplicates += 1;
                    continue;
                }
                if args.dry_run || remove_wire_at(&mut schematic, x, y) > 0 {
                    actions.push(Action {
                        violation_type: violation.type_str.clone(),
                        action: "remove_wire",
                        x,
                        y,
                        description: violation.description.clone(),
                    });
                } else {
                    skipped += 1;
                }
            }
            _ => skipped += 1,
        }
    }
    if !args.dry_run && !actions.is_empty() {
        schematic.save(args.output.as_deref())?;
    }
    if let Some(path) = temporary {
        let _ = std::fs::remove_file(path);
    }
    let pwr = actions
        .iter()
        .filter(|a| a.action == "insert_pwr_flag")
        .count();
    let nc = actions
        .iter()
        .filter(|a| a.action == "insert_no_connect")
        .count();
    let wires = actions.iter().filter(|a| a.action == "remove_wire").count();
    let targeted = report
        .violations
        .iter()
        .filter(|v| {
            !v.excluded
                && matches!(
                    v.vtype,
                    ERCViolationType::POWER_PIN_NOT_DRIVEN
                        | ERCViolationType::PIN_NOT_CONNECTED
                        | ERCViolationType::WIRE_DANGLING
                        | ERCViolationType::UNCONNECTED_WIRE_ENDPOINT
                )
        })
        .count();
    let remaining = targeted.saturating_sub(actions.len() + duplicates);
    let document = serde_json::json!({"command":"fix-erc","schematic":args.schematic,"output":args.output,"dry_run":args.dry_run,"total_violations":targeted,"pwr_flag_inserted":pwr,"no_connect_inserted":nc,"wires_removed":wires,"skipped_unknown":skipped,"skipped_duplicate":duplicates,"total_fixed":actions.len(),"remaining":remaining,"actions":actions,"success":remaining==0});
    if args.format == "json" {
        println!("{}", serde_json::to_string_pretty(&document)?);
    } else if args.format == "summary" {
        println!(
            "ERC fixes: {} targeted, {} {}, {remaining} remaining",
            targeted,
            actions.len(),
            if args.dry_run { "planned" } else { "applied" }
        );
    } else if targeted == 0 {
        println!("No auto-fixable ERC violations found. Nothing to fix.");
    } else {
        println!(
            "{} {} ERC fix(es): {pwr} PWR_FLAG, {nc} no-connect, {wires} dangling wire",
            if args.dry_run {
                "Would apply"
            } else {
                "Applied"
            },
            actions.len()
        );
        if !args.dry_run && !actions.is_empty() {
            println!("Saved to: {}", args.output.as_ref().unwrap().display())
        }
    }
    Ok(if remaining == 0 { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_round_to_hundredth() {
        assert_eq!(at_key(1.234, 5.678), at_key(1.23, 5.68));
    }
}
