//! Query design-decision JSON sidecars associated with boards.
use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
    Tree,
}
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}
#[derive(Subcommand)]
enum Cmd {
    Show {
        pcb: PathBuf,
        #[arg(short, long)]
        component: Option<String>,
        #[arg(short, long)]
        net: Option<String>,
        #[arg(short,long,value_parser=["place","route","move","reroute","delete"])]
        action: Option<String>,
        #[arg(short, long, value_enum, default_value = "text")]
        format: Format,
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },
    List {
        pcb: PathBuf,
        #[arg(short, long, value_enum, default_value = "text")]
        format: Format,
    },
    ExplainPlacement {
        pcb: PathBuf,
        component: String,
        #[arg(short, long, value_enum, default_value = "text")]
        format: Format,
    },
    ExplainRoute {
        pcb: PathBuf,
        net: String,
        #[arg(short, long, value_enum, default_value = "text")]
        format: Format,
    },
}
fn sidecar(p: &Path) -> PathBuf {
    p.with_extension("decisions.json")
}
fn load(p: &Path) -> Result<Vec<Value>> {
    if !p.exists() {
        anyhow::bail!("PCB file not found: {}", p.display())
    }
    let s = sidecar(p);
    if !s.exists() {
        anyhow::bail!("No decisions file found at: {}", s.display())
    }
    let v: Value = serde_json::from_slice(&fs::read(&s)?)
        .with_context(|| format!("invalid decisions file: {}", s.display()))?;
    Ok(v["decisions"].as_array().cloned().unwrap_or_default())
}
fn vals<'a>(v: &'a Value, k: &str) -> impl Iterator<Item = &'a str> {
    v[k].as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}
fn latest<'a>(ds: &'a [Value], key: &str, item: &str, actions: &[&str]) -> Option<&'a Value> {
    ds.iter()
        .filter(|d| {
            actions.contains(&d["action"].as_str().unwrap_or("")) && vals(d, key).any(|x| x == item)
        })
        .max_by_key(|d| d["timestamp"].as_str())
}
fn print_one(d: &Value, tree: bool) {
    println!(
        "{}{}: {} {}",
        if tree { "├─ " } else { "" },
        d["action"].as_str().unwrap_or("unknown"),
        d["id"].as_str().unwrap_or(""),
        d["timestamp"].as_str().unwrap_or("")
    );
    if let Some(s) = d["rationale"].as_str().filter(|s| !s.is_empty()) {
        println!("   {s}")
    }
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    match parse_args::<Cli>("decisions", args).command {
        None => {
            println!("Usage: kct decisions <show|list|explain-placement|explain-route>");
            Ok(0)
        }
        Some(Cmd::Show {
            pcb,
            component,
            net,
            action,
            format,
            limit,
        }) => {
            let all = load(&pcb)?;
            let total = all.len();
            let mut out: Vec<_> = all
                .into_iter()
                .filter(|d| {
                    component
                        .as_deref()
                        .is_none_or(|x| vals(d, "components").any(|v| v == x))
                })
                .filter(|d| {
                    net.as_deref()
                        .is_none_or(|x| vals(d, "nets").any(|v| v == x))
                })
                .filter(|d| {
                    action
                        .as_deref()
                        .is_none_or(|x| d["action"].as_str() == Some(x))
                })
                .collect();
            out.sort_by(|a, b| b["timestamp"].as_str().cmp(&a["timestamp"].as_str()));
            out.truncate(limit);
            if out.is_empty() {
                println!("No decisions found matching the specified filters.")
            } else if matches!(format, Format::Json) {
                println!("{}", serde_json::to_string_pretty(&out)?)
            } else {
                for d in &out {
                    print_one(d, matches!(format, Format::Tree))
                }
                if out.len() < total {
                    println!("\n(Showing {} of {total} total decisions)", out.len())
                }
            }
            Ok(0)
        }
        Some(Cmd::List { pcb, format }) => {
            let all = load(&pcb)?;
            let mut a = BTreeMap::<String, usize>::new();
            let mut by = BTreeMap::<String, usize>::new();
            let mut cs = BTreeSet::new();
            let mut ns = BTreeSet::new();
            for d in &all {
                *a.entry(d["action"].as_str().unwrap_or("unknown").into())
                    .or_default() += 1;
                *by.entry(d["decided_by"].as_str().unwrap_or("unknown").into())
                    .or_default() += 1;
                cs.extend(vals(d, "components").map(str::to_owned));
                ns.extend(vals(d, "nets").map(str::to_owned));
            }
            let j = json!({"total_decisions":all.len(),"by_action":a,"by_decided_by":by,"unique_components":cs.len(),"unique_nets":ns.len()});
            if matches!(format, Format::Json) {
                println!("{}", serde_json::to_string_pretty(&j)?)
            } else {
                println!("Decision Summary\n========================================\nTotal decisions: {}\n\nBy action:",all.len());
                for (k, v) in a {
                    println!("  {k}: {v}")
                }
                println!("\nBy decided_by:");
                for (k, v) in by {
                    println!("  {k}: {v}")
                }
                println!(
                    "\nUnique components: {}\nUnique nets: {}",
                    cs.len(),
                    ns.len()
                )
            }
            Ok(0)
        }
        Some(Cmd::ExplainPlacement {
            pcb,
            component,
            format,
        }) => {
            let ds = load(&pcb).unwrap_or_default();
            let d = latest(&ds, "components", &component, &["place", "move"]);
            let b = Pcb::load(&pcb)?;
            let fp = b
                .footprints()
                .iter()
                .find(|f| f.reference == component)
                .with_context(|| format!("Component {component} not found in PCB."))?;
            let j = json!({"component":component,"position":[fp.position.0,fp.position.1],"rationale":d.and_then(|x|x["rationale"].as_str()).unwrap_or("No decision recorded for this placement"),"decided_by":d.and_then(|x|x["decided_by"].as_str()).unwrap_or("unknown"),"timestamp":d.and_then(|x|x["timestamp"].as_str()).unwrap_or(""),"alternatives":d.map(|x|x["alternatives"].clone()).unwrap_or_else(||json!([])),"constraints":d.map(|x|x["constraints_satisfied"].clone()).unwrap_or_else(||json!([])),"decision_id":d.map(|x|x["id"].clone()).unwrap_or(Value::Null)});
            if matches!(format, Format::Json) {
                println!("{}", serde_json::to_string_pretty(&j)?)
            } else {
                println!("Component: {component}\nPosition: ({:.2}, {:.2})\nDecided by: {}\n\nRationale: {}",fp.position.0,fp.position.1,j["decided_by"].as_str().unwrap(),j["rationale"].as_str().unwrap())
            }
            Ok(0)
        }
        Some(Cmd::ExplainRoute { pcb, net, format }) => {
            let ds = load(&pcb).unwrap_or_default();
            let d = latest(&ds, "nets", &net, &["route", "reroute"]);
            let j = json!({"net":net,"rationale":d.and_then(|x|x["rationale"].as_str()).unwrap_or("No decision recorded for this route"),"decided_by":d.and_then(|x|x["decided_by"].as_str()).unwrap_or("unknown"),"timestamp":d.and_then(|x|x["timestamp"].as_str()).unwrap_or(""),"alternatives":d.map(|x|x["alternatives"].clone()).unwrap_or_else(||json!([])),"constraints":d.map(|x|x["constraints_satisfied"].clone()).unwrap_or_else(||json!([])),"metrics":d.map(|x|x["metrics"].clone()).unwrap_or_else(||json!({})),"decision_id":d.map(|x|x["id"].clone()).unwrap_or(Value::Null)});
            if matches!(format, Format::Json) {
                println!("{}", serde_json::to_string_pretty(&j)?)
            } else {
                println!(
                    "Net: {net}\nDecided by: {}\n\nRationale: {}",
                    j["decided_by"].as_str().unwrap(),
                    j["rationale"].as_str().unwrap()
                )
            }
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sidecar_name() {
        assert_eq!(
            sidecar(Path::new("x.kicad_pcb")),
            Path::new("x.decisions.json")
        );
    }
}
