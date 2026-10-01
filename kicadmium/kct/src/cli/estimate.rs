use super::{parse_args, Globals};
use crate::{cost, schema::pcb::Pcb};
use anyhow::Result;
use clap::{Args as ClapArgs, Parser, Subcommand};
use std::{ffi::OsString, path::PathBuf};
#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Cost(Cost),
}
#[derive(ClapArgs)]
struct Cost {
    pcb: PathBuf,
    #[arg(long)]
    bom: Option<PathBuf>,
    #[arg(short, long, default_value_t = 10)]
    quantity: usize,
    #[arg(short, long, default_value = "jlcpcb")]
    mfr: String,
    #[arg(long, default_value = "hasl", value_parser = ["hasl", "hasl_lead_free", "enig", "osp"])]
    finish: String,
    #[arg(long, default_value = "green", value_parser = ["green", "red", "blue", "black", "white", "yellow"])]
    color: String,
    #[arg(short, long)]
    layers: Option<u8>,
    #[arg(long, default_value_t = 1.6)]
    thickness: f64,
    #[arg(long)]
    no_lcsc: bool,
    #[arg(short, long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
    #[arg(short, long)]
    verbose: bool,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let Args {
        command: Command::Cost(a),
    } = parse_args::<Args>("estimate", args);
    let p = Pcb::load(&a.pcb)?;
    let v = cost::estimate(&p, &a.mfr, a.quantity, &a.finish, &a.color, a.thickness);
    if a.format == "json" {
        println!("{}", serde_json::to_string_pretty(&v)?)
    } else {
        println!("Manufacturing Cost Estimate ({}, qty: {}):\n  TOTAL: ${:.2}/unit (${:.2} for {} units)",a.mfr.to_uppercase(),a.quantity,v["summary"]["total_per_unit"].as_f64().unwrap_or(0.),v["summary"]["total_for_quantity"].as_f64().unwrap_or(0.),a.quantity)
    }
    let _ = (a.bom, a.layers, a.no_lcsc, a.verbose);
    Ok(0)
}
