use super::{parse_args, Globals};
use anyhow::{Context, Result};
use clap::Parser;
use std::{ffi::OsString, path::PathBuf};
#[derive(Parser)]
struct Args {
    board: PathBuf,
    #[arg(long)]
    plan: PathBuf,
    #[arg(long)]
    quality_profile: Option<PathBuf>,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: Args = parse_args("place-check", args);
    let pcb = crate::schema::pcb::Pcb::load(a.board)?;
    let plan = serde_yaml::from_slice(&std::fs::read(a.plan)?)?;
    let (max, same_side) = if let Some(path) = a.quality_profile {
        let profile: serde_yaml::Value = serde_yaml::from_slice(&std::fs::read(path)?)?;
        let max = profile["decoupling"]
            .get("max_pin_distance_mm")
            .map(|v| v.as_f64().context("decoupling distance must be numeric"))
            .transpose()?
            .unwrap_or(2.);
        let same_side = profile["decoupling"]
            .get("same_side_as_ic")
            .map(|v| v.as_bool().context("same_side_as_ic must be boolean"))
            .transpose()?
            .unwrap_or(false);
        (max, same_side)
    } else {
        (2., false)
    };
    let report = crate::place_check::check_profile(&pcb, &plan, max, same_side)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(if report["ok"] == true { 0 } else { 2 })
}
