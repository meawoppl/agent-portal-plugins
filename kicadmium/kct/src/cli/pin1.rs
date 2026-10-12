use super::{parse_args, Globals};
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::{ffi::OsString, path::PathBuf};
#[derive(Parser)]
#[command(about = "Attach board pin-1 dots to footprints so they follow all placement edits")]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Migrate explicitly identified board circles to footprint-local silk circles.
    Attach {
        board: PathBuf,
        #[arg(long)]
        markers: PathBuf,
        #[arg(long, default_value_t = 2.5)]
        max_distance: f64,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, conflicts_with = "output")]
        in_place: bool,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: Args = parse_args("pin1", args);
    match a.command {
        Command::Attach {
            board,
            markers,
            max_distance,
            dry_run,
            in_place,
            output,
        } => {
            if !dry_run && !in_place && output.is_none() {
                bail!("choose --dry-run, --in-place, or --output")
            }
            let mut pcb = crate::schema::pcb::Pcb::load(&board)?;
            let markers: Vec<crate::pin1::Marker> =
                serde_json::from_slice(&std::fs::read(markers)?)?;
            let report = crate::pin1::attach(&mut pcb, &markers, max_distance)?;
            if !dry_run {
                pcb.save(Some(output.as_deref().unwrap_or(&board)))?;
            }
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(0)
        }
    }
}
