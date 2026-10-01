//! Footprint-local pad spacing validation and conservative two-pad repair.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Serialize;

use super::{parse_args, Globals};
use crate::schema::pcb::{Footprint, Pad, Pcb};

#[derive(Parser)]
struct ValidateArgs {
    pcb: PathBuf,
    #[arg(long = "min-pad-gap", default_value_t = 0.15)]
    min_pad_gap: f64,
    #[arg(long, default_value = "text", value_parser = ["text", "json", "summary"])]
    format: String,
    #[arg(long = "errors-only")]
    errors_only: bool,
    #[arg(long = "compare-standard")]
    compare_standard: bool,
    #[arg(long, default_value_t = 0.05)]
    tolerance: f64,
    #[arg(long = "kicad-library-path")]
    kicad_library_path: Option<PathBuf>,
}

#[derive(Parser)]
struct FixArgs {
    pcb: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long = "min-pad-gap", default_value_t = 0.2)]
    min_pad_gap: f64,
    #[arg(long, default_value = "text", value_parser = ["text", "json", "summary"])]
    format: String,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Serialize)]
struct Issue {
    reference: String,
    footprint: String,
    #[serde(rename = "type")]
    kind: &'static str,
    severity: &'static str,
    message: String,
    details: serde_json::Value,
}

#[derive(Clone, Serialize)]
struct Adjustment {
    footprint_ref: String,
    pad_number: String,
    old_position: (f64, f64),
    new_position: (f64, f64),
    reason: String,
}

#[derive(Clone, Serialize)]
struct Fix {
    footprint_ref: String,
    footprint_name: String,
    adjustments: Vec<Adjustment>,
    old_pad_spacing: f64,
    new_pad_spacing: f64,
}

fn pad_gap(a: &Pad, b: &Pad) -> f64 {
    let gap_x = (b.position.0 - a.position.0).abs() - (a.size.0 + b.size.0) / 2.0;
    let gap_y = (b.position.1 - a.position.1).abs() - (a.size.1 + b.size.1) / 2.0;
    match (gap_x > 0.0, gap_y > 0.0) {
        (true, true) => gap_x.hypot(gap_y),
        (true, false) => gap_x,
        (false, true) => gap_y,
        (false, false) => gap_x.max(gap_y),
    }
}

fn issues_for(fp: &Footprint, min: f64) -> Vec<Issue> {
    let mut out = Vec::new();
    for (i, a) in fp.pads.iter().enumerate() {
        for b in &fp.pads[i + 1..] {
            let gap = pad_gap(a, b);
            let (kind, severity, message) = if gap < 0.0 {
                (
                    "pad_overlap",
                    "error",
                    format!(
                        "Pad {} and Pad {} are overlapping ({:.3}mm overlap)",
                        a.number,
                        b.number,
                        gap.abs()
                    ),
                )
            } else if gap < 0.001 {
                (
                    "pad_touching",
                    "warning",
                    format!(
                        "Pad {} and Pad {} are touching ({gap:.3}mm gap)",
                        a.number, b.number
                    ),
                )
            } else if gap < min {
                (
                    "pad_spacing",
                    "warning",
                    format!(
                        "Pad {} and Pad {} have insufficient spacing ({gap:.3}mm gap, need {min:.3}mm)",
                        a.number, b.number
                    ),
                )
            } else {
                continue;
            };
            out.push(Issue {
                reference: fp.reference.clone(),
                footprint: fp.name.clone(),
                kind,
                severity,
                message,
                details: serde_json::json!({"pad1":a.number,"pad2":b.number,"gap_mm":gap,"required_gap_mm":min,"pad1_position":a.position,"pad2_position":b.position,"pad1_size":a.size,"pad2_size":b.size}),
            });
        }
    }
    out
}

fn standard_path(root: &std::path::Path, name: &str) -> Option<PathBuf> {
    let (library, footprint) = name.split_once(':')?;
    let candidates = [
        root.join(format!("{library}.pretty/{footprint}.kicad_mod")),
        root.join("footprints")
            .join(format!("{library}.pretty/{footprint}.kicad_mod")),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn standard_issues(pcb: &Pcb, root: &std::path::Path, tolerance: f64) -> Result<Vec<Issue>> {
    let mut out = Vec::new();
    for fp in pcb.footprints() {
        let Some(path) = standard_path(root, &fp.name) else {
            out.push(Issue {
                reference: fp.reference.clone(),
                footprint: fp.name.clone(),
                kind: "standard_not_found",
                severity: "warning",
                message: "Footprint not found in selected KiCad library".into(),
                details: serde_json::json!({"library_root":root}),
            });
            continue;
        };
        let root_node = crate::parse(&std::fs::read_to_string(&path)?)?;
        let standard: Vec<Pad> = root_node
            .children_named("pad")
            .map(Pad::from_sexp)
            .collect();
        let ours: BTreeMap<_, _> = fp.pads.iter().map(|p| (&p.number, p)).collect();
        let theirs: BTreeMap<_, _> = standard.iter().map(|p| (&p.number, p)).collect();
        let numbers: BTreeSet<_> = ours
            .keys()
            .chain(theirs.keys())
            .map(|s| (*s).clone())
            .collect();
        for number in numbers {
            let Some((a, b)) = ours.get(&number).zip(theirs.get(&number)) else {
                out.push(Issue {
                    reference: fp.reference.clone(),
                    footprint: fp.name.clone(),
                    kind: "pad_count_mismatch",
                    severity: "error",
                    message: format!("Pad {number} exists in only one footprint"),
                    details: serde_json::json!({"pad":number,"standard":path}),
                });
                continue;
            };
            let delta = (a.position.0 - b.position.0).hypot(a.position.1 - b.position.1);
            if delta > tolerance {
                out.push(Issue { reference:fp.reference.clone(), footprint:fp.name.clone(), kind:"pad_position_mismatch", severity:"error", message:format!("Pad {number} position differs from standard by {delta:.3}mm"), details:serde_json::json!({"pad":number,"ours":a.position,"standard":b.position,"delta_mm":delta,"tolerance_mm":tolerance}) });
            }
            let size_delta = (a.size.0 - b.size.0).abs().max((a.size.1 - b.size.1).abs());
            if size_delta > tolerance {
                out.push(Issue { reference:fp.reference.clone(), footprint:fp.name.clone(), kind:"pad_size_mismatch", severity:"warning", message:format!("Pad {number} size differs from standard by {size_delta:.3}mm"), details:serde_json::json!({"pad":number,"ours":a.size,"standard":b.size,"delta_mm":size_delta,"tolerance_mm":tolerance}) });
            }
            if a.shape != b.shape {
                out.push(Issue {
                    reference: fp.reference.clone(),
                    footprint: fp.name.clone(),
                    kind: "pad_shape_mismatch",
                    severity: "warning",
                    message: format!(
                        "Pad {number} shape {:?} differs from standard {:?}",
                        a.shape, b.shape
                    ),
                    details: serde_json::json!({"pad":number,"ours":a.shape,"standard":b.shape}),
                });
            }
        }
    }
    Ok(out)
}

fn output_issues(issues: &[Issue], format: &str) -> Result<()> {
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(issues)?);
    } else if format == "summary" {
        let errors = issues.iter().filter(|i| i.severity == "error").count();
        let warnings = issues.len() - errors;
        let footprints: BTreeSet<_> = issues.iter().map(|i| &i.reference).collect();
        println!("Total issues: {}\nFootprints with issues: {}\n\nBy severity:\n  error: {errors}\n  warning: {warnings}",issues.len(),footprints.len());
    } else if issues.is_empty() {
        println!("No footprint issues found.");
    } else {
        for i in issues {
            println!(
                "{} ({}): {} - {}",
                i.reference,
                i.footprint,
                i.severity.to_ascii_uppercase(),
                i.message
            );
        }
        println!("\nFound {} footprint issue(s)", issues.len());
    }
    Ok(())
}

pub fn validate(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<ValidateArgs>("validate-footprints", args);
    if args.min_pad_gap < 0.0 || args.tolerance < 0.0 {
        bail!("gap and tolerance must be non-negative")
    }
    let pcb = Pcb::load(&args.pcb)?;
    let mut issues = if args.compare_standard {
        let root = args
            .kicad_library_path
            .or_else(|| std::env::var_os("KICAD_FOOTPRINT_DIR").map(PathBuf::from))
            .context("--compare-standard requires --kicad-library-path or KICAD_FOOTPRINT_DIR")?;
        standard_issues(&pcb, &root, args.tolerance)?
    } else {
        pcb.footprints()
            .iter()
            .flat_map(|f| issues_for(f, args.min_pad_gap))
            .collect()
    };
    if args.errors_only {
        issues.retain(|i| i.severity == "error")
    };
    output_issues(&issues, &args.format)?;
    Ok(if issues.iter().any(|i| i.severity == "error") {
        1
    } else {
        0
    })
}

pub fn fix(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<FixArgs>("fix-footprints", args);
    if args.min_pad_gap < 0.0 {
        bail!("--min-pad-gap must be non-negative")
    }
    if !args.dry_run && args.output.is_none() {
        bail!("fix-footprints is a design edit; pass --output (or use --dry-run)")
    }
    let mut pcb = Pcb::load(&args.pcb)?;
    let mut fixes = Vec::new();
    let refs: Vec<_> = pcb
        .footprints()
        .iter()
        .map(|f| f.reference.clone())
        .collect();
    for reference in refs {
        let Some(fp) = pcb.get_footprint(&reference).cloned() else {
            continue;
        };
        if fp.pads.len() != 2 || issues_for(&fp, args.min_pad_gap).is_empty() {
            continue;
        }
        let (a, b) = (&fp.pads[0], &fp.pads[1]);
        let (dx, dy) = (b.position.0 - a.position.0, b.position.1 - a.position.1);
        let horizontal = dx.abs() > dy.abs();
        let current = if horizontal { dx.abs() } else { dy.abs() };
        let required = if horizontal {
            args.min_pad_gap + (a.size.0 + b.size.0) / 2.0
        } else {
            args.min_pad_gap + (a.size.1 + b.size.1) / 2.0
        };
        if current >= required {
            continue;
        }
        let center = (
            (a.position.0 + b.position.0) / 2.0,
            (a.position.1 + b.position.1) / 2.0,
        );
        let (p1, p2) = if horizontal {
            let sign = if a.position.0 <= b.position.0 {
                1.0
            } else {
                -1.0
            };
            (
                (center.0 - sign * required / 2.0, a.position.1),
                (center.0 + sign * required / 2.0, b.position.1),
            )
        } else {
            let sign = if a.position.1 <= b.position.1 {
                1.0
            } else {
                -1.0
            };
            (
                (a.position.0, center.1 - sign * required / 2.0),
                (b.position.0, center.1 + sign * required / 2.0),
            )
        };
        let adjustments = vec![
            Adjustment {
                footprint_ref: reference.clone(),
                pad_number: a.number.clone(),
                old_position: a.position,
                new_position: p1,
                reason: format!("Increase pad gap to {}mm", args.min_pad_gap),
            },
            Adjustment {
                footprint_ref: reference.clone(),
                pad_number: b.number.clone(),
                old_position: b.position,
                new_position: p2,
                reason: format!("Increase pad gap to {}mm", args.min_pad_gap),
            },
        ];
        if !args.dry_run {
            let mut fm = pcb.footprint_mut(&reference).unwrap();
            fm.pad_mut(0).unwrap().set_position(p1);
            fm.pad_mut(1).unwrap().set_position(p2);
        }
        fixes.push(Fix {
            footprint_ref: reference,
            footprint_name: fp.name,
            adjustments,
            old_pad_spacing: current,
            new_pad_spacing: required,
        });
    }
    if !args.dry_run && !fixes.is_empty() {
        pcb.save(args.output.as_deref())?
    }
    if args.format == "json" {
        println!("{}", serde_json::to_string_pretty(&fixes)?)
    } else if args.format == "summary" {
        println!(
            "Total footprints {}: {}\nTotal pads adjusted: {}",
            if args.dry_run { "to fix" } else { "fixed" },
            fixes.len(),
            fixes.len() * 2
        )
    } else if fixes.is_empty() {
        println!("No fixable two-pad footprint issues found.")
    } else {
        for f in &fixes {
            println!(
                "{} {} ({}): {:.3} -> {:.3}mm center spacing",
                if args.dry_run { "Would fix" } else { "Fixed" },
                f.footprint_ref,
                f.footprint_name,
                f.old_pad_spacing,
                f.new_pad_spacing
            )
        }
        if !args.dry_run {
            println!("\nSaved to: {}", args.output.as_ref().unwrap().display())
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gap_axis_and_diagonal() {
        let mut a =
            Pad::from_sexp(&crate::parse("(pad \"1\" smd rect (at 0 0) (size 1 1))").unwrap());
        let mut b =
            Pad::from_sexp(&crate::parse("(pad \"2\" smd rect (at 2 0) (size 1 1))").unwrap());
        assert_eq!(pad_gap(&a, &b), 1.0);
        b.position = (2.0, 2.0);
        assert!((pad_gap(&a, &b) - 2f64.sqrt()).abs() < 1e-9);
        a.size = (2.0, 2.0);
        b.position = (1.0, 0.0);
        assert_eq!(pad_gap(&a, &b), -0.5);
    }
}
