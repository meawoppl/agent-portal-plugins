use super::{parse_args, Globals};
use anyhow::Result;
use clap::{Parser, Subcommand};
use regex::Regex;
use serde::Serialize;
use std::{collections::BTreeSet, ffi::OsString, path::PathBuf};
#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Alternatives {
        schematic: PathBuf,
        #[arg(long)]
        bom: bool,
        #[arg(short = 'n', long, default_value_t = 3)]
        max_alternatives: usize,
        #[arg(short = 'f', long, default_value = "text")]
        format: String,
        #[arg(long)]
        no_cache: bool,
        #[arg(long)]
        show_all: bool,
        #[arg(short, long)]
        verbose: bool,
    },
}
#[derive(Serialize)]
struct Suggestion {
    part: String,
    status: String,
    stock: i64,
    alternatives: Vec<crate::parts::Part>,
    diagnostic: Option<String>,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let Command::Alternatives {
        schematic,
        bom: _,
        max_alternatives,
        format,
        no_cache,
        show_all,
        verbose: _,
    } = parse_args::<Args>("suggest", args).command;
    let text = std::fs::read_to_string(&schematic)?;
    let re = Regex::new(r"(?i)\bC\d{2,10}\b")?;
    let ids = re
        .find_iter(&text)
        .map(|m| m.as_str().to_ascii_uppercase())
        .collect::<BTreeSet<_>>();
    let mut out = vec![];
    for id in ids {
        let part = if no_cache {
            None
        } else {
            crate::parts::cache_get(&id, 7)?
        }
        .or_else(|| crate::parts::lookup(&id).ok().flatten());
        match part {
            Some(p) => {
                let problematic = p.stock <= 0;
                let alternatives = if problematic || show_all {
                    crate::parts::search(
                        &format!("{} {}", p.description, p.package),
                        max_alternatives as u64,
                        true,
                        false,
                    )
                    .map(|r| {
                        r.parts
                            .into_iter()
                            .filter(|x| x.lcsc_part != p.lcsc_part)
                            .take(max_alternatives)
                            .collect()
                    })
                    .unwrap_or_default()
                } else {
                    vec![]
                };
                out.push(Suggestion {
                    part: id,
                    status: if problematic {
                        "unavailable"
                    } else {
                        "available"
                    }
                    .into(),
                    stock: p.stock,
                    alternatives,
                    diagnostic: None,
                })
            }
            None => out.push(Suggestion {
                part: id,
                status: "unknown".into(),
                stock: 0,
                alternatives: vec![],
                diagnostic: Some("lookup unavailable or part not found".into()),
            }),
        }
    }
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&out)?)
    } else {
        for s in out {
            println!("{}: {} (stock {})", s.part, s.status, s.stock);
            for p in s.alternatives {
                println!(
                    "  -> {} {} {} stock {}",
                    p.lcsc_part, p.mfr_part, p.package, p.stock
                )
            }
            if let Some(d) = s.diagnostic {
                println!("  {d}")
            }
        }
    }
    Ok(0)
}
