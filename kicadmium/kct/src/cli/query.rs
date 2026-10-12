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
    /// Rank legal through-via sites across all copper layers.
    ViaSites {
        board: PathBuf,
        #[arg(long)]
        net: String,
        #[arg(long, value_delimiter = ',', num_args = 2, allow_hyphen_values = true)]
        near: Vec<f64>,
        #[arg(long)]
        radius: f64,
        #[arg(long, default_value_t = 0.6)]
        size: f64,
        #[arg(long, default_value_t = 0.3)]
        drill: f64,
        #[arg(long, default_value_t = 0.25)]
        step: f64,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Probe straight and 45-degree corridors, including blocking objects.
    Corridor {
        board: PathBuf,
        #[arg(long)]
        net: String,
        #[arg(long)]
        layer: String,
        #[arg(long, value_delimiter = ',', num_args = 2, allow_hyphen_values = true)]
        from: Vec<f64>,
        #[arg(long, value_delimiter = ',', num_args = 2, allow_hyphen_values = true)]
        to: Vec<f64>,
        #[arg(long)]
        width: f64,
    },
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
        Query::ViaSites {
            board,
            net,
            near,
            radius,
            size,
            drill,
            step,
            limit,
        } => {
            let probe = crate::routing_query::Probe::load(&board)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&probe.via_sites(
                    &net,
                    crate::lint::model::Point {
                        x: near[0],
                        y: near[1]
                    },
                    radius,
                    size,
                    drill,
                    step,
                    limit
                )?)?
            );
        }
        Query::Corridor {
            board,
            net,
            layer,
            from,
            to,
            width,
        } => {
            let probe = crate::routing_query::Probe::load(&board)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&probe.corridor(
                    &net,
                    &layer,
                    crate::lint::model::Point {
                        x: from[0],
                        y: from[1]
                    },
                    crate::lint::model::Point { x: to[0], y: to[1] },
                    width
                )?)?
            );
        }
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
