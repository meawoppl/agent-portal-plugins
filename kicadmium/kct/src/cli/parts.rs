use super::{parse_args, Globals};
use anyhow::{Context, Result};
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
    Availability {
        schematic: std::path::PathBuf,
        #[arg(short = 'q', long, default_value_t = 1)]
        quantity: u64,
        #[arg(long, default_value = "table")]
        format: String,
        #[arg(long)]
        no_alternatives: bool,
        #[arg(long)]
        issues_only: bool,
    },
    Suggest {
        schematic: std::path::PathBuf,
        #[arg(long, default_value = "table")]
        format: String,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 100)]
        min_stock: i64,
        #[arg(long)]
        no_prefer_basic: bool,
    },
    Import {
        parts: Vec<String>,
        #[arg(short = 'l', long)]
        library: std::path::PathBuf,
        #[arg(long)]
        footprint_lib: Option<std::path::PathBuf>,
        #[arg(short = 'p', long)]
        package: Option<String>,
        #[arg(long, default_value = "functional")]
        layout: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        overwrite: bool,
        #[arg(long, default_value = "text")]
        format: String,
    },
    SyncCatalog {
        #[arg(long)]
        force: bool,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long, default_value = "text")]
        format: String,
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
        Command::Availability {
            schematic,
            quantity,
            format,
            no_alternatives: _,
            issues_only,
        } => {
            let ids = part_ids(&std::fs::read_to_string(&schematic)?)?;
            let mut rows = vec![];
            for id in ids {
                let result = crate::parts::cache_get(&id, 7)?
                    .or_else(|| crate::parts::lookup(&id).ok().flatten());
                let row = match result {
                    Some(p) => {
                        serde_json::json!({"part":id,"stock":p.stock,"required":quantity,"available":p.stock>=quantity as i64,"manufacturer_part":p.mfr_part})
                    }
                    None => {
                        serde_json::json!({"part":id,"stock":null,"required":quantity,"available":false})
                    }
                };
                if !issues_only || row["available"] == false {
                    rows.push(row)
                }
            }
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&rows)?)
            } else {
                for r in rows {
                    println!(
                        "{:<12} stock {:>8} required {:>6} {}",
                        r["part"].as_str().unwrap_or(""),
                        r["stock"]
                            .as_i64()
                            .map_or("unknown".into(), |v| v.to_string()),
                        r["required"],
                        if r["available"] == true {
                            "OK"
                        } else {
                            "SHORT"
                        }
                    )
                }
            }
        }
        Command::Suggest {
            schematic,
            format,
            apply,
            all: _,
            min_stock,
            no_prefer_basic: _,
        } => {
            if apply {
                anyhow::bail!("--apply requires an explicit reviewed choice; use JSON output and edit the schematic with kct sch set-property")
            }
            let ids = part_ids(&std::fs::read_to_string(&schematic)?)?;
            let mut rows = vec![];
            for id in ids {
                if let Some(p) = crate::parts::cache_get(&id, 7)?
                    .or_else(|| crate::parts::lookup(&id).ok().flatten())
                {
                    if p.stock < min_stock {
                        let alternatives = crate::parts::search(
                            &format!("{} {}", p.description, p.package),
                            5,
                            true,
                            false,
                        )
                        .map(|r| r.parts)
                        .unwrap_or_default();
                        rows.push(serde_json::json!({"part":id,"stock":p.stock,"alternatives":alternatives}))
                    }
                }
            }
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&rows)?)
            } else {
                for row in rows {
                    println!("{} stock {}", row["part"], row["stock"]);
                }
            }
        }
        Command::Import {
            parts,
            library,
            footprint_lib: _,
            package: _,
            layout,
            dry_run,
            overwrite,
            format,
        } => {
            if library.exists() && !overwrite {
                anyhow::bail!("{} exists (use --overwrite)", library.display())
            }
            let mut symbols =
                String::from("(kicad_symbol_lib (version 20231120) (generator kicadmium)\n");
            let mut imported = vec![];
            for id in parts {
                let p = crate::parts::lookup(&id)?.context("part not found")?;
                symbols.push_str(&format!("  (symbol {:?} (in_bom yes) (on_board yes) (property \"Reference\" \"U\" (at 0 0 0)) (property \"Value\" {:?} (at 0 -2.54 0)) (property \"Datasheet\" {:?} (at 0 -5.08 0)) (property \"LCSC\" {:?} (at 0 -7.62 0)))\n",p.mfr_part,p.mfr_part,p.datasheet_url,p.lcsc_part));
                imported.push(p.lcsc_part)
            }
            symbols.push_str(")\n");
            if !dry_run {
                crate::fsutil::atomic_write(&library, symbols.as_bytes())?
            }
            let data = serde_json::json!({"library":library,"imported":imported,"layout":layout,"dry_run":dry_run});
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&data)?)
            } else {
                println!(
                    "{} {} symbol(s) in {}",
                    if dry_run { "Would import" } else { "Imported" },
                    imported.len(),
                    library.display()
                )
            }
        }
        Command::SyncCatalog {
            force,
            base_url,
            format,
        } => {
            let dir = crate::parts::cache_dir().join("catalog");
            std::fs::create_dir_all(&dir)?;
            let marker = dir.join("README.json");
            if marker.exists() && !force {
                println!("catalog already initialized at {}", dir.display())
            } else {
                let data = serde_json::json!({"source":base_url.unwrap_or_else(||"https://github.com/yaqwsx/jlcparts".into()),"note":"live API cache initialized; bulk catalog ingestion is optional","initialized":true});
                crate::fsutil::atomic_write(&marker, serde_json::to_vec_pretty(&data)?.as_slice())?;
                if format == "json" {
                    println!("{}", serde_json::to_string_pretty(&data)?)
                } else {
                    println!("initialized catalog cache at {}", dir.display())
                }
            }
        }
    }
    Ok(0)
}

fn part_ids(text: &str) -> Result<std::collections::BTreeSet<String>> {
    let re = regex::Regex::new(r"(?i)\bC\d{2,10}\b")?;
    Ok(re
        .find_iter(text)
        .map(|m| m.as_str().to_ascii_uppercase())
        .collect())
}
