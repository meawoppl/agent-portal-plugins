//! `kct copper import`: add tracks, arcs and vias to a board from a JSON
//! list, so a parametric geometry generator (a coil, a fanout pattern, a
//! test structure) can live outside kicadmium without writing raw
//! s-expressions. With `--group NAME` the imported items are wrapped in a
//! KiCad group, and `--replace` removes that group's previous members first,
//! so regenerating is idempotent and hand-placed copper is never touched.
//!
//! Input (`--from`): either a bare array or `{"units": "mm", "items": [...]}`.
//! Each item is one of
//!
//! ```json
//! {"type": "polyline", "layer": "F.Cu", "net": "COIL_D0", "width": 0.5, "points": [[x, y], ...]}
//! {"type": "arc", "layer": "F.Cu", "net": "COIL_D0", "width": 0.5, "start": [x, y], "mid": [x, y], "end": [x, y]}
//! {"type": "via", "at": [x, y], "size": 0.8, "drill": 0.4, "net": "COIL_D0", "layers": ["F.Cu", "B.Cu"]}
//! ```
//!
//! `type` may be omitted (inferred from `points`, `mid` or `at`). `units`
//! is `mm` (default) or `m`. Coordinates are file coordinates as KiCad shows
//! them (`--coords absolute`), or relative to the board outline's origin
//! (`--coords board`).

use std::{collections::HashSet, ffi::OsString, path::PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use super::{parse_args, Globals};
use crate::schema::pcb::{Pcb, TraceOptions, ViaOptions};
use crate::sexp::SExp;

#[derive(Parser)]
#[command(about = "Board copper from structured input")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Add segments, arcs and vias listed in a JSON file.
    Import(ImportArgs),
}

#[derive(Parser)]
struct ImportArgs {
    pcb: PathBuf,
    /// JSON file with the items to add.
    #[arg(long)]
    from: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Put the imported items in a KiCad group of this name.
    #[arg(long)]
    group: Option<String>,
    /// First remove the members of any existing group with that name.
    #[arg(long, requires = "group")]
    replace: bool,
    /// `absolute`: KiCad file coordinates; `board`: relative to the outline origin.
    #[arg(long, default_value = "absolute", value_parser = ["absolute", "board"])]
    coords: String,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("copper", args);
    match args.command {
        Command::Import(a) => import(a),
    }
}

fn point(v: &Value, what: &str, scale: f64) -> Result<(f64, f64)> {
    let arr = v
        .as_array()
        .filter(|a| a.len() == 2)
        .with_context(|| format!("{what}: expected [x, y]"))?;
    let x = arr[0].as_f64().with_context(|| format!("{what}: x"))?;
    let y = arr[1].as_f64().with_context(|| format!("{what}: y"))?;
    if !(x.is_finite() && y.is_finite()) {
        bail!("{what}: coordinates must be finite")
    }
    Ok((round6(x * scale), round6(y * scale)))
}

/// Scaled values to the nanometre, so `0.0005 m` is written as `0.5`.
fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

fn number(item: &Value, key: &str, index: usize) -> Result<f64> {
    let v = item
        .get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("item {index}: {key} must be a number"))?;
    if !(v.is_finite() && v > 0.0) {
        bail!("item {index}: {key} must be positive")
    }
    Ok(v)
}

/// Like [`number`] but in the file's units, scaled to millimetres.
fn length(item: &Value, key: &str, index: usize, scale: f64) -> Result<f64> {
    number(item, key, index).map(|v| round6(v * scale))
}

fn string(item: &Value, key: &str, index: usize) -> Result<String> {
    item.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("item {index}: {key} is required"))
}

/// Members of every group named `name`, and the group nodes' own uuids.
fn group_members(pcb: &Pcb, name: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for g in pcb.sexp().children_named("group") {
        let named = g
            .atoms()
            .next()
            .and_then(|v| v.as_str())
            .is_some_and(|n| n == name);
        if !named {
            continue;
        }
        if let Some(u) = g
            .find("uuid")
            .and_then(|u| u.atoms().next())
            .and_then(|v| v.as_str())
        {
            out.insert(u.to_string());
        }
        if let Some(m) = g.find("members") {
            out.extend(m.atoms().filter_map(|v| v.as_str().map(str::to_owned)));
        }
    }
    out
}

fn import(a: ImportArgs) -> Result<i32> {
    if !a.dry_run && a.output.is_none() {
        bail!("copper import is a design edit; pass --output (or use --dry-run)")
    }
    let text =
        std::fs::read_to_string(&a.from).with_context(|| format!("read {}", a.from.display()))?;
    let doc: Value = serde_json::from_str(&text).context("parse JSON")?;
    let (items, units) = match &doc {
        Value::Array(items) => (items.clone(), "mm".to_string()),
        Value::Object(o) => (
            o.get("items")
                .and_then(Value::as_array)
                .cloned()
                .context("JSON object needs an \"items\" array")?,
            o.get("units")
                .and_then(Value::as_str)
                .unwrap_or("mm")
                .to_string(),
        ),
        _ => bail!("JSON must be an array of items or an object with \"items\""),
    };
    let scale = match units.as_str() {
        "mm" => 1.0,
        "m" => 1000.0,
        other => bail!("units must be mm or m, not {other:?}"),
    };
    let mut pcb = Pcb::load(&a.pcb)?;
    let origin = if a.coords == "absolute" {
        pcb.board_origin()
    } else {
        (0.0, 0.0)
    };
    let to_board = |p: (f64, f64)| (p.0 - origin.0, p.1 - origin.1);

    let removed = match (&a.group, a.replace) {
        (Some(name), true) => {
            let members = group_members(&pcb, name);
            pcb.remove_by_uuid(&members)
        }
        _ => 0,
    };

    let mut added: Vec<String> = Vec::new();
    let (mut segments, mut arcs, mut vias) = (0usize, 0usize, 0usize);
    for (index, item) in items.iter().enumerate() {
        let kind = item
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                if item.get("points").is_some() {
                    Some("polyline".into())
                } else if item.get("mid").is_some() {
                    Some("arc".into())
                } else if item.get("at").is_some() {
                    Some("via".into())
                } else {
                    None
                }
            })
            .with_context(|| format!("item {index}: cannot tell polyline, arc or via apart"))?;
        let net = item
            .get("net")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        match kind.as_str() {
            "polyline" | "segment" | "track" => {
                let pts = item
                    .get("points")
                    .and_then(Value::as_array)
                    .with_context(|| format!("item {index}: points"))?;
                if pts.len() < 2 {
                    bail!("item {index}: a polyline needs at least two points")
                }
                let pts: Vec<(f64, f64)> = pts
                    .iter()
                    .map(|p| point(p, &format!("item {index} point"), scale).map(to_board))
                    .collect::<Result<_>>()?;
                let opts = TraceOptions {
                    width: length(item, "width", index, scale)?,
                    layer: string(item, "layer", index)?,
                    net: net.clone(),
                    waypoints: pts[1..pts.len() - 1].to_vec(),
                    dedupe: false,
                };
                for s in pcb.add_trace(pts[0], pts[pts.len() - 1], opts)? {
                    added.push(s.uuid);
                    segments += 1;
                }
            }
            "arc" => {
                let p = |k: &str| {
                    point(
                        item.get(k).unwrap_or(&Value::Null),
                        &format!("item {index} {k}"),
                        scale,
                    )
                    .map(to_board)
                };
                let opts = TraceOptions {
                    width: length(item, "width", index, scale)?,
                    layer: string(item, "layer", index)?,
                    net: net.clone(),
                    waypoints: vec![],
                    dedupe: false,
                };
                let arc = pcb
                    .add_arc(p("start")?, p("mid")?, p("end")?, opts)
                    .with_context(|| format!("item {index}"))?;
                added.push(arc.uuid);
                arcs += 1;
            }
            "via" => {
                let at = to_board(point(
                    item.get("at").unwrap_or(&Value::Null),
                    &format!("item {index} at"),
                    scale,
                )?);
                let layers: Vec<String> = match item.get("layers").and_then(Value::as_array) {
                    Some(l) => l
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect(),
                    None => vec!["F.Cu".into(), "B.Cu".into()],
                };
                if layers.len() != 2 {
                    bail!("item {index}: via layers must name the two end layers")
                }
                let opts = ViaOptions {
                    size: length(item, "size", index, scale)?,
                    drill: length(item, "drill", index, scale)?,
                    layers,
                    net: net.clone(),
                    dedupe: false,
                };
                if opts.drill >= opts.size {
                    bail!("item {index}: via drill must be smaller than its size")
                }
                let via = pcb
                    .add_via(at.0, at.1, opts)
                    .with_context(|| format!("item {index}: via"))?;
                added.push(via.uuid);
                vias += 1;
            }
            other => bail!("item {index}: unknown type {other:?}"),
        }
    }

    let group_uuid = a.group.as_ref().map(|name| {
        let uuid = crate::schema::pcb::new_uuid();
        let node = SExp::list(
            "group",
            [
                SExp::quoted(name.clone()),
                SExp::list("uuid", [SExp::quoted(uuid.clone())]),
                SExp::list("members", added.iter().map(|u| SExp::quoted(u.clone()))),
            ],
        );
        pcb.sexp_mut().push(node);
        uuid
    });
    if !a.dry_run {
        pcb.save(a.output.as_deref())?;
    }
    let report = json!({
        "command": "copper import",
        "input": a.pcb.display().to_string(),
        "output": a.output.as_ref().map(|p| p.display().to_string()),
        "dry_run": a.dry_run,
        "units": units,
        "coords": a.coords,
        "group": a.group,
        "group_uuid": group_uuid,
        "removed": removed,
        "added": {"segments": segments, "arcs": arcs, "vias": vias, "total": added.len()},
        "success": true,
    });
    if a.format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Imported {} segments, {} arcs, {} vias{}{}{}",
            segments,
            arcs,
            vias,
            a.group
                .as_ref()
                .map_or(String::new(), |g| format!(" into group {g:?}")),
            if removed > 0 {
                format!(" (replaced {removed} previous items)")
            } else {
                String::new()
            },
            if a.dry_run {
                " [dry run]".to_string()
            } else {
                format!(" -> {}", a.output.as_ref().unwrap().display())
            },
        );
    }
    Ok(0)
}
