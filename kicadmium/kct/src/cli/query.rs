use super::{parse_args, Globals};
use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::{ffi::OsString, path::PathBuf};
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Query,
}
#[derive(Subcommand)]
enum Query {
    /// Inventory geometry intersecting an axis-aligned rectangle in mm.
    Region {
        board: PathBuf,
        #[arg(allow_hyphen_values = true)]
        x0: f64,
        #[arg(allow_hyphen_values = true)]
        y0: f64,
        #[arg(allow_hyphen_values = true)]
        x1: f64,
        #[arg(allow_hyphen_values = true)]
        y1: f64,
        #[arg(long, value_delimiter = ',')]
        layers: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        nets: Vec<String>,
        #[arg(long, default_value = "50")]
        limit: usize,
        #[arg(long, value_enum, default_value = "table")]
        format: Format,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Table,
    Json,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    match parse_args::<Cli>("query", args).command {
        Query::Region {
            board,
            x0,
            y0,
            x1,
            y1,
            layers,
            nets,
            limit,
            format,
        } => {
            let source = std::fs::read_to_string(board)?;
            let inventory = crate::region::query(
                &source,
                crate::region::Bounds { x0, y0, x1, y1 },
                &layers,
                &nets,
                limit,
            )?;
            match format {
                Format::Table => println!("{}", inventory.table()),
                Format::Json => println!("{}", serde_json::to_string_pretty(&inventory)?),
            }
        }
    }
    Ok(0)
}
