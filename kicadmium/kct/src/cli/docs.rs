use super::{parse_args, Globals};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::Value;
use std::{ffi::OsString, path::PathBuf};
#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Sync {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = ".kicad-pcb.json")]
        manifest: PathBuf,
        #[arg(long)]
        check: bool,
    },
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let args: Args = parse_args("docs", args);
    match args.command {
        Command::Sync {
            project,
            manifest,
            check,
        } => {
            let config: Value = serde_json::from_slice(&std::fs::read(&manifest)?)?;
            let id = project
                .as_deref()
                .or_else(|| config["defaultProject"].as_str());
            let projects = config["projects"]
                .as_array()
                .context("manifest requires projects")?;
            let p = if let Some(id) = id {
                projects
                    .iter()
                    .find(|p| p["id"] == id)
                    .context("unknown project")?
            } else if projects.len() == 1 {
                &projects[0]
            } else {
                bail!("select --project")
            };
            let root = manifest
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(p["root"].as_str().unwrap_or("."));
            let board = root.join(
                p["kicad"]["pcb"]
                    .as_str()
                    .context("project requires kicad.pcb")?,
            );
            let files: Vec<crate::derived_docs::DerivedFile> =
                serde_json::from_value(p["derivedFiles"].clone())
                    .context("project requires derivedFiles array")?;
            let pcb = crate::schema::pcb::Pcb::load(board)?;
            let plans = crate::derived_docs::plan(&root, &pcb, &files)?;
            let stale = plans.iter().filter(|p| p.stale).count();
            for plan in plans {
                if plan.stale && !check {
                    if let Some(parent) = plan.path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    crate::fsutil::atomic_write(&plan.path, &plan.content)?;
                }
                println!(
                    "{} {}",
                    if plan.stale {
                        if check {
                            "stale"
                        } else {
                            "updated"
                        }
                    } else {
                        "current"
                    },
                    plan.path.display()
                );
            }
            Ok(if check && stale > 0 { 1 } else { 0 })
        }
    }
}
