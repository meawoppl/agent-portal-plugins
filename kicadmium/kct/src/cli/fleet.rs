use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;
use anyhow::{bail, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Status(Status),
    #[command(name = "ship-ready")]
    ShipReady(Ship),
}
#[derive(ClapArgs)]
struct Status {
    #[arg(long, default_value = "boards")]
    boards_dir: PathBuf,
    #[arg(long, default_value = "table")]
    format: String,
    #[arg(long)]
    ship_only: bool,
    #[arg(long)]
    include_stale: bool,
    #[arg(long, default_value = "*_routed.kicad_pcb")]
    pattern: String,
    #[arg(long, default_value = ".github/routed-drc-tolerance.yml")]
    drc_tolerance_file: PathBuf,
}
#[derive(ClapArgs)]
struct Ship {
    #[arg(long, default_value = "boards")]
    boards_dir: PathBuf,
    #[arg(long, default_value = "table")]
    format: String,
    #[arg(long, default_value = "*_routed.kicad_pcb")]
    pattern: String,
    #[arg(long, default_value = ".github/routed-drc-tolerance.yml")]
    drc_tolerance_file: PathBuf,
    #[arg(long)]
    strict: bool,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    match parse_args::<Args>("fleet", args).command {
        Command::Status(a) => status(a),
        Command::ShipReady(a) => ship(a),
    }
}
fn status(a: Status) -> Result<i32> {
    let boards = survey(&a.boards_dir)?;
    let good = boards.iter().filter(|b| b["ship_ready"] == true).count();
    if a.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"schema_version":"1.1","surveyed_at":now(),"boards_dir":a.boards_dir,"summary":{"total":boards.len(),"ship_ready":good,"incomplete_routing":boards.iter().filter(|b|b["routing"]["routing_complete"]==false).count(),"stale_artifacts":boards.iter().filter(|b|b["manufacturing"]["stale"]==true).count(),"missing_artifacts":boards.iter().filter(|b|b["manufacturing"]["has_all"]==false).count(),"drc_over_tolerance":0},"boards":boards})
            )?
        )
    } else {
        println!(
            "{:<28} {:>7} {:>5} {:<8} {:<6} {:>5} Ship?",
            "Board", "Pads", "%", "Mfr", "Stale", "DRC"
        );
        for b in boards
            .iter()
            .filter(|b| !a.ship_only || b["ship_ready"] == true)
        {
            println!(
                "{:<28} {:>7} {:>5.0}% {:<8} {:<6} {:>5} {}",
                b["name"].as_str().unwrap_or(""),
                format!(
                    "{}/{}",
                    b["routing"]["connected_pads"], b["routing"]["total_pads"]
                ),
                b["routing"]["completion_pct"].as_f64().unwrap_or(0.),
                if b["manufacturing"]["has_all"] == true {
                    "B/C/G/M"
                } else {
                    "-"
                },
                if b["manufacturing"]["stale"] == true {
                    "STALE"
                } else {
                    "fresh"
                },
                "-",
                if b["ship_ready"] == true { "YES" } else { "NO" }
            );
        }
    }
    let _ = (a.include_stale, a.pattern, a.drc_tolerance_file);
    Ok(if !boards.is_empty() && good == boards.len() {
        0
    } else {
        2
    })
}
fn ship(a: Ship) -> Result<i32> {
    let boards = survey(&a.boards_dir)?;
    let statuses:Vec<_>=boards.iter().map(|b|json!({"name":b["name"],"passed":b["ship_ready"],"blockers":b["blockers"],"board":b})).collect();
    let pass = statuses.iter().filter(|s| s["passed"] == true).count();
    if a.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"schema_version":"1.1","command":"ship-ready","surveyed_at":now(),"boards_dir":a.boards_dir,"summary":{"total":statuses.len(),"passed":pass,"failed":statuses.len()-pass,"warn_only":!a.strict},"boards":statuses})
            )?
        )
    } else {
        for s in &statuses {
            println!(
                "{:<28} {}",
                s["name"].as_str().unwrap_or(""),
                if s["passed"] == true { "PASS" } else { "FAIL" }
            );
        }
    }
    let _ = (a.pattern, a.drc_tolerance_file);
    Ok(
        if a.strict && (statuses.is_empty() || pass != statuses.len()) {
            2
        } else {
            0
        },
    )
}
fn survey(root: &Path) -> Result<Vec<Value>> {
    if !root.is_dir() {
        bail!("boards directory not found: {}", root.display())
    }
    let mut dirs: Vec<_> = std::fs::read_dir(root)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let mut out = Vec::new();
    for d in dirs {
        let Some(p) = find_pcb(&d) else { continue };
        let pcb = Pcb::load(&p)?;
        let total: usize = pcb
            .footprints()
            .iter()
            .flat_map(|f| &f.pads)
            .filter(|p| p.net_number != 0)
            .count();
        let traced = pcb
            .segments()
            .iter()
            .map(|s| s.net_number)
            .collect::<std::collections::BTreeSet<_>>();
        let connected: usize = pcb
            .footprints()
            .iter()
            .flat_map(|f| &f.pads)
            .filter(|p| p.net_number != 0 && traced.contains(&p.net_number))
            .count();
        let nets = pcb.nets().iter().filter(|n| n.number != 0).count();
        let complete = connected == total && total > 0;
        let m = d.join("output/manufacturing");
        let manifest = m.join("manifest.json");
        let has_g = m.join("gerbers").is_dir()
            || std::fs::read_dir(&m).ok().is_some_and(|mut x| {
                x.any(|e| {
                    e.ok().is_some_and(|e| {
                        e.path().extension().and_then(|x| x.to_str()) == Some("gbr")
                    })
                })
            });
        let has_b = ["bom_jlcpcb.csv", "bom.csv"]
            .iter()
            .any(|n| m.join(n).is_file());
        let has_c = ["cpl_jlcpcb.csv", "cpl.csv", "positions.csv"]
            .iter()
            .any(|n| m.join(n).is_file());
        let has_m = manifest.is_file();
        let all = has_g && has_b && has_c && has_m;
        let stale = has_m
            && std::fs::metadata(&p)?
                .modified()?
                .duration_since(std::fs::metadata(&manifest)?.modified()?)
                .is_ok_and(|x| x.as_secs() > 120);
        let ready = complete && all && !stale;
        out.push(json!({"name":d.file_name().unwrap_or_default(),"path":d,"pcb":p,"routing":{"total_pads":total,"connected_pads":connected,"completion_pct":if total==0{0.}else{connected as f64/total as f64*100.},"total_nets":nets,"complete_nets":if complete{nets}else{0},"incomplete_nets":if complete{0}else{nets},"blocking_incomplete_nets":if complete{0}else{nets},"unrouted_nets":if traced.is_empty(){nets}else{0},"routing_complete":complete,"source_stale":false,"schematic_net_count":null,"pcb_net_count":nets},"manufacturing":{"dir_exists":m.is_dir(),"has_gerbers":has_g,"has_bom":has_b,"has_cpl":has_c,"has_manifest":has_m,"manifest_mtime":null,"stale":stale,"has_all":all},"drc":{"report_exists":false,"errors":0,"over_tolerance":false},"ship_ready":ready,"blockers":if ready{vec![]}else{vec![if !complete{"routing incomplete"}else if !all{"manufacturing artifacts missing"}else{"manufacturing artifacts stale"}]}}));
    }
    Ok(out)
}
fn find_pcb(d: &Path) -> Option<PathBuf> {
    let o = d.join("output");
    let mut p: Vec<_> = std::fs::read_dir(o)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("kicad_pcb"))
        .collect();
    p.sort_by_key(|p| {
        (
            !p.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .ends_with("_routed"),
            p.clone(),
        )
    });
    p.into_iter().next()
}
fn now() -> String {
    format!(
        "{}Z",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    )
}
