use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;
use anyhow::{bail, Result};
use clap::Parser;
use serde_json::{json, Map, Value};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
#[derive(Parser)]
struct Args {
    board: Option<PathBuf>,
    #[arg(long)]
    all: bool,
    #[arg(long, default_value = "boards")]
    boards_dir: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, default_value = "text")]
    format: String,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<Args>("board-metrics", args);
    let dirs = if a.all {
        board_dirs(&a.boards_dir)?
    } else {
        vec![a
            .board
            .context("a board directory is required (or use --all)")?]
    };
    let mut entries = Vec::new();
    for d in dirs {
        let metrics = extract(&d)?;
        let out = a
            .output
            .clone()
            .unwrap_or_else(|| d.join("output/board.json"));
        if !a.dry_run {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?
            }
            crate::fsutil::atomic_write(&out, &serde_json::to_vec_pretty(&metrics)?)?
        }
        entries.push(json!({"slug":metrics["slug"],"status":metrics["status"],"output_path":if a.dry_run{Value::Null}else{json!(out)},"metrics":metrics}));
    }
    let env = json!({"command":"board-metrics","mode":if a.all{"all"}else{"single"},"dry_run":a.dry_run,"boards":entries,"success":true});
    if a.format == "json" {
        println!("{}", serde_json::to_string_pretty(&env)?)
    } else if a.dry_run {
        for e in entries {
            println!("{}", serde_json::to_string_pretty(&e["metrics"])?);
        }
    } else {
        for e in entries {
            println!(
                "{:<30} {:<13} -> {}",
                e["slug"].as_str().unwrap_or(""),
                e["status"].as_str().unwrap_or(""),
                e["output_path"].as_str().unwrap_or("")
            );
        }
    }
    Ok(0)
}
use anyhow::Context;
fn board_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.is_dir() {
        bail!("boards directory not found: {}", root.display())
    }
    let mut v: Vec<_> = std::fs::read_dir(root)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    if v.is_empty() {
        bail!("no board subdirectories found under {}", root.display())
    }
    Ok(v)
}
pub fn extract(d: &Path) -> Result<Value> {
    if !d.is_dir() {
        bail!("board directory not found: {}", d.display())
    }
    let slug = d
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let output = d.join("output");
    let mfg = output.join("manufacturing");
    let pcb = find_pcb(d);
    let mut o = Map::new();
    o.insert(
        "$schema".into(),
        json!("https://kicad-tools.org/schemas/board/v1.json"),
    );
    o.insert("schema_version".into(), json!(1));
    o.insert(
        "generated_at".into(),
        json!(format!(
            "{}Z",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs()
        )),
    );
    o.insert("slug".into(), json!(slug));
    if let Some(p) = pcb {
        let pcb = Pcb::load(p)?;
        let s = pcb.summary()?;
        o.insert("name".into(), json!(s.title));
        o.insert("layer_count".into(), json!(s.copper_layers));
        o.insert(
            "board_size_mm".into(),
            json!({"width":s.width_mm,"height":s.height_mm}),
        );
        o.insert("part_count".into(), json!(s.footprints));
    }
    let renders = output.join("renders");
    let mut rr = Map::new();
    for (k, n) in [
        ("pcb_front", "pcb-front.svg"),
        ("pcb_back", "pcb-back.svg"),
        ("3d_front", "3d-front.png"),
        ("3d_back", "3d-back.png"),
    ] {
        if renders.join(n).is_file() {
            rr.insert(k.into(), json!(format!("renders/{n}")));
        }
    }
    if !rr.is_empty() {
        o.insert("renders".into(), Value::Object(rr));
    }
    if mfg.join("kicad_project.zip").is_file() {
        o.insert(
            "manufacturing_package".into(),
            json!("manufacturing/kicad_project.zip"),
        );
    }
    o.insert(
        "status".into(),
        json!(if mfg.is_dir() {
            "partial"
        } else if o.contains_key("part_count") {
            "no_artifacts"
        } else {
            "no_artifacts"
        }),
    );
    Ok(Value::Object(o))
}
fn find_pcb(d: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for base in [d.to_path_buf(), d.join("output")] {
        if let Ok(rd) = std::fs::read_dir(base) {
            candidates.extend(
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("kicad_pcb")),
            );
        }
    }
    candidates.sort_by_key(|p| {
        (
            !p.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .ends_with("_routed"),
            p.clone(),
        )
    });
    candidates.into_iter().next()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_board() {
        let d = tempfile::tempdir().unwrap();
        let v = extract(d.path()).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["status"], "no_artifacts");
    }
}
