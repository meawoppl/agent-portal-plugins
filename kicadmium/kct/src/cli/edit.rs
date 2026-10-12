use super::{parse_args, Globals};
use anyhow::{bail, Result};
use clap::Parser;
use std::{ffi::OsString, path::PathBuf};
#[derive(Parser)]
#[command(about = "Apply an atomic native JSON/YAML edit plan")]
struct Args {
    board: PathBuf,
    #[arg(long)]
    plan: PathBuf,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, conflicts_with = "output")]
    in_place: bool,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    allow_violations: bool,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: Args = parse_args("edit", args);
    if !a.dry_run && !a.in_place && a.output.is_none() {
        bail!("choose --dry-run, --in-place, or --output")
    }
    let source = std::fs::read(&a.board)?;
    let original = crate::schema::pcb::Pcb::load(&a.board)?;
    let plan: crate::edit_plan::Plan = serde_yaml::from_slice(&std::fs::read(a.plan)?)?;
    let mut candidate = crate::edit_plan::apply(&original, &plan)?;
    let before = crate::edit_plan::clearance_findings(&original)?;
    let added = crate::edit_plan::added(&before, crate::edit_plan::clearance_findings(&candidate)?);
    let allowed = added.is_empty() || a.allow_violations;
    if !a.dry_run && allowed {
        if std::fs::read(&a.board)? != source {
            bail!("board changed during preflight; retry against current source")
        }
        candidate.save(Some(a.output.as_deref().unwrap_or(&a.board)))?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"schema":"kct.edit/v1","dry_run":a.dry_run,"written":!a.dry_run&&allowed,"operations":plan.operations.len(),"added_clearance_findings":added,"requires_full_drc":true})
        )?
    );
    Ok(if allowed { 0 } else { 2 })
}
