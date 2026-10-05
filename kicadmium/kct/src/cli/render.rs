//! Native render orchestration; KiCad itself remains the rendering engine.
use super::{parse_args, Globals};
use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Serialize;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser)]
struct Args {
    path: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    no_3d: bool,
    #[arg(long, default_value = "text")]
    format: String,
}
#[derive(Serialize)]
struct BoardResult {
    board: String,
    pcb: Option<PathBuf>,
    status: String,
    outputs: std::collections::BTreeMap<String, PathBuf>,
    errors: Vec<String>,
    model_check: serde_json::Value,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<Args>("render", args);
    if !a.path.exists() {
        bail!("path not found: {}", a.path.display())
    }
    let cli = kicad_cli()?;
    let targets = discover(&a.path)?;
    let mut results = Vec::new();
    for (pcb, out) in targets {
        results.push(render_one(
            &cli,
            &pcb,
            a.output.as_deref().unwrap_or(&out),
            !a.no_3d,
        ));
    }
    if a.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"boards":results}))?
        )
    } else {
        for r in &results {
            for p in r.outputs.values() {
                println!("  {}: wrote {}", r.board, p.display())
            }
            for e in &r.errors {
                eprintln!("  {}: ERROR {e}", r.board)
            }
        }
    }
    Ok(if results.iter().any(|r| r.status == "error") {
        1
    } else {
        0
    })
}
fn discover(p: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    if p.is_file() {
        if p.extension().and_then(|x| x.to_str()) != Some("kicad_pcb") {
            bail!("Expected .kicad_pcb file, got: {}", p.display())
        }
        return Ok(vec![(
            p.into(),
            p.parent().unwrap_or(Path::new(".")).join("renders"),
        )]);
    }
    let mut dirs = Vec::new();
    if p.join("output").is_dir() {
        dirs.push(p.into())
    } else {
        for e in std::fs::read_dir(p)? {
            let c = e?.path();
            if c.join("output").is_dir() {
                dirs.push(c)
            } else if c.is_dir() {
                for g in std::fs::read_dir(c)? {
                    let g = g?.path();
                    if g.join("output").is_dir() {
                        dirs.push(g)
                    }
                }
            }
        }
    }
    dirs.sort();
    let mut out = Vec::new();
    for d in dirs {
        let od = d.join("output");
        let mut pcbs: Vec<_> = std::fs::read_dir(&od)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("kicad_pcb"))
            .collect();
        pcbs.sort_by_key(|p| {
            (
                !p.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .ends_with("_routed"),
                p.clone(),
            )
        });
        if let Some(pcb) = pcbs.into_iter().next() {
            out.push((pcb, od.join("renders")))
        }
    }
    if out.is_empty() {
        bail!("no board directories found under {}", p.display())
    }
    Ok(out)
}
fn render_one(cli: &Path, pcb: &Path, out: &Path, do3d: bool) -> BoardResult {
    let _ = std::fs::create_dir_all(out);
    let mut r = BoardResult {
        board: pcb.file_stem().unwrap_or_default().to_string_lossy().into(),
        pcb: Some(pcb.into()),
        status: "error".into(),
        outputs: Default::default(),
        errors: vec![],
        model_check: serde_json::json!({"status":if do3d{"not_run"}else{"skipped"},"checked_models":0,"unresolved_models":[]}),
    };
    for (key, layers) in [
        ("pcb-front", "F.Cu,F.Silkscreen,Edge.Cuts"),
        ("pcb-back", "B.Cu,B.Silkscreen,Edge.Cuts"),
    ] {
        let f = out.join(format!("{key}.svg"));
        let s = Command::new(cli)
            .args(["pcb", "export", "svg", "--mode-single", "--output"])
            .arg(&f)
            .args([
                "--layers",
                layers,
                "--page-size-mode",
                "2",
                "--fit-page-to-board",
            ])
            .arg(pcb)
            .output();
        record(&mut r, key, f, s)
    }
    if do3d {
        for (key, side, rotate) in [
            ("3d-front", "top", "-70,0,0"),
            ("3d-back", "bottom", "70,0,0"),
        ] {
            let f = out.join(format!("{key}.png"));
            let s = Command::new(cli)
                .args(["pcb", "render", "--output"])
                .arg(&f)
                .args([
                    "--side",
                    side,
                    "--quality",
                    "high",
                    "--rotate",
                    rotate,
                    "--perspective",
                ])
                .arg(pcb)
                .output();
            record(&mut r, key, f, s)
        }
    }
    let expected = if do3d { 4 } else { 2 };
    r.status = if r.outputs.len() == expected && r.errors.is_empty() {
        "ok"
    } else if r.outputs.is_empty() {
        "error"
    } else {
        "partial"
    }
    .into();
    r
}
fn record(r: &mut BoardResult, key: &str, file: PathBuf, o: std::io::Result<std::process::Output>) {
    match o {
        Ok(_o) if file.is_file() && std::fs::metadata(&file).is_ok_and(|m| m.len() > 0) => {
            r.outputs.insert(key.into(), file);
        }
        Ok(o) => r.errors.push(format!(
            "{key}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => r.errors.push(format!("{key}: {e}")),
    }
}
fn kicad_cli() -> Result<PathBuf> {
    for k in ["KICADMIUM_KICAD_CLI", "KICAD_CLI"] {
        if let Some(p) = std::env::var_os(k).map(PathBuf::from) {
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    std::env::var_os("PATH")
        .and_then(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("kicad-cli"))
                .find(|p| p.is_file())
        })
        .context("kicad-cli not found")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_discovery() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.kicad_pcb");
        std::fs::write(&p, "").unwrap();
        let x = discover(&p).unwrap();
        assert_eq!(x[0].1, d.path().join("renders"));
    }
}
