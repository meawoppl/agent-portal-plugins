use super::{parse_args, Globals};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::ffi::OsString;
#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Lookup {
        part: String,
        #[arg(long, default_value = "text")]
        format: String,
        #[arg(long)]
        no_cache: bool,
    },
    Search {
        query: String,
        #[arg(long, default_value = "table")]
        format: String,
        #[arg(long, default_value_t = 20)]
        limit: u64,
        #[arg(long)]
        in_stock: bool,
        #[arg(long)]
        basic: bool,
    },
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
}
#[derive(Subcommand)]
enum CacheAction {
    Stats,
    Clear,
    ClearExpired,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    match parse_args::<Args>("parts", args).command {
        Command::Lookup {
            part,
            format,
            no_cache,
        } => {
            let p = if no_cache {
                None
            } else {
                crate::parts::cache_get(&part, 7)?
            }
            .or(crate::parts::lookup(&part)?);
            match p {
                Some(p) => {
                    crate::parts::cache_put(&p)?;
                    if format == "json" {
                        println!("{}", serde_json::to_string_pretty(&p)?)
                    } else {
                        println!(
                            "{}  {}  {}\n{}\npackage: {}  stock: {}  basic: {}\n{}",
                            p.lcsc_part,
                            p.manufacturer,
                            p.mfr_part,
                            p.description,
                            p.package,
                            p.stock,
                            p.is_basic,
                            p.datasheet_url
                        )
                    }
                }
                None => {
                    eprintln!("part not found: {}", crate::parts::normalize_part(&part));
                    return Ok(1);
                }
            }
        }
        Command::Search {
            query,
            format,
            limit,
            in_stock,
            basic,
        } => {
            let r = crate::parts::search(&query, limit, in_stock, basic)?;
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&r)?)
            } else {
                println!(
                    "{:<12} {:<24} {:<12} {:>10}  description",
                    "LCSC", "MPN", "package", "stock"
                );
                for p in r.parts {
                    println!(
                        "{:<12} {:<24} {:<12} {:>10}  {}",
                        p.lcsc_part, p.mfr_part, p.package, p.stock, p.description
                    )
                }
            }
        }
        Command::Cache { action } => match action {
            CacheAction::Stats => {
                let d = crate::parts::cache_dir();
                let n = if d.exists() {
                    std::fs::read_dir(&d)?.count()
                } else {
                    0
                };
                println!("{} cached part(s) in {}", n, d.display())
            }
            CacheAction::Clear | CacheAction::ClearExpired => {
                println!("cleared {} cached part(s)", crate::parts::clear_cache()?)
            }
        },
    }
    Ok(0)
}
