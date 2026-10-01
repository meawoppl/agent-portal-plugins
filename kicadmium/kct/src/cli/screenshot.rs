//! Deterministic 2D board/schematic screenshot generation through KiCad's
//! native SVG exporter and an in-process Rust SVG rasterizer.
use super::{parse_args, Globals};
use anyhow::{Context, Result};
use clap::Parser;
use serde_json::json;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser)]
struct Args {
    input: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    layers: Option<String>,
    #[arg(long, default_value_t = 1568)]
    max_size: u32,
    #[arg(long = "bw", alias = "black-and-white")]
    black_and_white: bool,
    #[arg(long)]
    theme: Option<String>,
    #[arg(long, default_value = "text")]
    format: String,
}

pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<Args>("screenshot", args);
    if !a.input.is_file() {
        return failure(&a, "File not found");
    }
    let ext = a.input.extension().and_then(|x| x.to_str()).unwrap_or("");
    if !matches!(ext, "kicad_pcb" | "kicad_sch") {
        return failure(
            &a,
            &format!("Unsupported file type: .{ext} (expected .kicad_pcb or .kicad_sch)"),
        );
    }
    let output = a
        .output
        .clone()
        .unwrap_or_else(|| a.input.with_extension("png"));
    let temp = std::env::temp_dir().join(format!("kct-screenshot-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp)?;
    let layers = if ext == "kicad_pcb" {
        resolve_layers(a.layers.as_deref())
    } else {
        vec![]
    };
    let cli = kicad_cli()?;
    let mut command = Command::new(cli);
    if ext == "kicad_pcb" {
        command
            .args([
                "pcb",
                "export",
                "svg",
                "--mode-single",
                "--fit-page-to-board",
                "--page-size-mode",
                "2",
                "--output",
            ])
            .arg(temp.join("capture.svg"))
            .arg("--layers")
            .arg(layers.join(","));
        if a.black_and_white {
            command.arg("--black-and-white");
        }
        command.arg(&a.input);
    } else {
        command
            .args(["sch", "export", "svg", "--output"])
            .arg(&temp);
        if a.black_and_white {
            command.arg("--black-and-white");
        }
        if let Some(t) = &a.theme {
            command.args(["--theme", t]);
        }
        command.arg(&a.input);
    }
    let ran = command.output().context("run kicad-cli SVG export")?;
    if !ran.status.success() {
        let msg = String::from_utf8_lossy(&ran.stderr);
        let _ = std::fs::remove_dir_all(&temp);
        return failure(&a, msg.trim());
    }
    let svg = if ext == "kicad_pcb" {
        temp.join("capture.svg")
    } else {
        std::fs::read_dir(&temp)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().and_then(|x| x.to_str()) == Some("svg"))
            .context("KiCad wrote no SVG")?
    };
    let (w, h) = rasterize(&svg, &output, a.max_size)?;
    let _ = std::fs::remove_dir_all(&temp);
    if a.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"command":"screenshot","input":a.input,"output":output,"width_px":w,"height_px":h,"layers_rendered":layers,"success":true})
            )?
        )
    } else {
        println!(
            "Screenshot saved to {}\n  Size: {}x{} px",
            output.display(),
            w,
            h
        );
        if !layers.is_empty() {
            println!("  Layers: {}", layers.join(", "))
        }
    }
    Ok(0)
}
fn failure(a: &Args, msg: &str) -> Result<i32> {
    if a.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"command":"screenshot","input":a.input,"error":msg,"success":false})
            )?
        )
    } else {
        eprintln!("Error: {msg}")
    }
    Ok(1)
}
fn resolve_layers(s: Option<&str>) -> Vec<String> {
    let raw = match s.unwrap_or("default") {
        "default" => "F.Cu,B.Cu,F.Silkscreen,B.Silkscreen,Edge.Cuts",
        "copper" => "F.Cu,B.Cu,Edge.Cuts",
        "assembly" => "F.Fab,B.Fab,Edge.Cuts",
        "front" => "F.Cu,F.Silkscreen,Edge.Cuts",
        "back" => "B.Cu,B.Silkscreen,Edge.Cuts",
        x => x,
    };
    raw.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect()
}
fn rasterize(svg: &Path, out: &Path, max: u32) -> Result<(u32, u32)> {
    let data = std::fs::read(svg)?;
    let tree = resvg::usvg::Tree::from_data(&data, &Default::default())?;
    let size = tree.size();
    let scale = (max as f32 / size.width())
        .min(max as f32 / size.height())
        .min(1.0);
    let w = (size.width() * scale).round().max(1.) as u32;
    let h = (size.height() * scale).round().max(1.) as u32;
    let mut pix = resvg::tiny_skia::Pixmap::new(w, h).context("allocate screenshot")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pix.as_mut(),
    );
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?
    }
    pix.save_png(out)?;
    Ok((w, h))
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
    fn presets() {
        assert_eq!(
            resolve_layers(Some("front")),
            vec!["F.Cu", "F.Silkscreen", "Edge.Cuts"]
        );
        assert_eq!(resolve_layers(Some("X,Y")), vec!["X", "Y"]);
    }
}
