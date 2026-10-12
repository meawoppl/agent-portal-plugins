//! Board-derived documentation. Only declared geometry fields are overwritten.
use crate::schema::pcb::Pcb;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedFile {
    pub path: PathBuf,
    pub kind: Kind,
}
#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Placements,
    Testpoints,
    Pin1Markers,
}
pub struct Planned {
    pub path: PathBuf,
    pub content: Vec<u8>,
    pub stale: bool,
}
fn safe(root: &Path, path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        bail!("derived path must stay within project root")
    }
    let full = root.join(path);
    let canonical = root.canonicalize()?;
    let mut parent = full.as_path();
    while !parent.exists() {
        parent = parent.parent().context("invalid output parent")?;
    }
    if !parent.canonicalize()?.starts_with(canonical) {
        bail!("derived path escapes project through a symlink")
    }
    Ok(full)
}
/// Calculate records without mutating the board or existing docs.
pub fn records(pcb: &Pcb, kind: Kind, previous: &Value) -> Result<Value> {
    let old = previous
        .as_array()
        .context("derived JSON must be an array of records")?;
    let mut out = Vec::new();
    for fp in pcb.footprints() {
        if matches!(kind, Kind::Testpoints)
            && !(fp.reference.starts_with("TP") || fp.name.contains("TestPoint"))
        {
            continue;
        }
        let base = || {
            old.iter()
                .find(|v| v["reference"] == fp.reference)
                .cloned()
                .unwrap_or_else(|| json!({}))
        };
        if matches!(kind, Kind::Pin1Markers) {
            for gr in fp
                .graphics
                .iter()
                .filter(|g| g.graphic_type == "circle" && g.layer.ends_with("SilkS"))
            {
                let Some(center) = gr.center else { continue };
                let Some(pad) = fp.pads.iter().find(|p| p.number == "1" || p.number == "A1") else {
                    continue;
                };
                if (center.0 - pad.position.0).hypot(center.1 - pad.position.1) > 2.5 {
                    continue;
                }
                let (sin, cos) = (-fp.rotation.to_radians()).sin_cos();
                let mut row = base();
                row["reference"] = json!(fp.reference);
                row["position"] = json!([
                    fp.position.0 + pcb.board_origin().0 + center.0 * cos - center.1 * sin,
                    fp.position.1 + pcb.board_origin().1 + center.0 * sin + center.1 * cos
                ]);
                row["local_position"] = json!(center);
                row["side"] = json!(if fp.layer.starts_with('B') {
                    "back"
                } else {
                    "front"
                });
                out.push(row);
            }
            continue;
        }
        let mut row = base();
        if !row.is_object() {
            bail!("previous record must be an object")
        }
        row["reference"] = json!(fp.reference);
        row["value"] = json!(fp.value);
        row["position"] = json!([
            fp.position.0 + pcb.board_origin().0,
            fp.position.1 + pcb.board_origin().1
        ]);
        row["rotation"] = json!(fp.rotation);
        row["side"] = json!(if fp.layer.starts_with('B') {
            "back"
        } else {
            "front"
        });
        if matches!(kind, Kind::Testpoints) {
            row["label"] = json!(fp.value);
            let mut nets: Vec<_> = fp
                .pads
                .iter()
                .map(|p| p.net_name.clone())
                .filter(|n| !n.is_empty())
                .collect();
            nets.sort();
            nets.dedup();
            row["nets"] = json!(nets);
        }
        out.push(row);
    }
    out.sort_by_key(|r| format!("{}:{}", r["reference"], r["local_position"]));
    Ok(json!(out))
}
pub fn plan(root: &Path, pcb: &Pcb, files: &[DerivedFile]) -> Result<Vec<Planned>> {
    let mut plans = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for file in files {
        let path = safe(root, &file.path)?;
        if !seen.insert(path.clone()) {
            bail!("duplicate derived output {}", path.display())
        }
        let existing = std::fs::read(&path).ok();
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let previous = if ext == "json" {
            existing
                .as_ref()
                .map(|b| serde_json::from_slice(b))
                .transpose()?
                .unwrap_or_else(|| json!([]))
        } else {
            json!([])
        };
        let rows = records(pcb, file.kind, &previous)?;
        let text = match ext {
            "json" => format!("{}\n", serde_json::to_string_pretty(&rows)?),
            "csv" => {
                let mut w = csv::Writer::from_writer(Vec::new());
                w.write_record([
                    "reference",
                    "value",
                    "x_mm",
                    "y_mm",
                    "rotation_deg",
                    "side",
                    "nets",
                ])?;
                for r in rows.as_array().unwrap() {
                    w.write_record([
                        r["reference"].as_str().unwrap_or("").to_owned(),
                        r["value"].as_str().unwrap_or("").to_owned(),
                        r["position"][0].to_string(),
                        r["position"][1].to_string(),
                        r["rotation"].to_string(),
                        r["side"].as_str().unwrap_or("").to_owned(),
                        r["nets"].to_string(),
                    ])?;
                }
                String::from_utf8(w.into_inner()?)?
            }
            "md" => {
                let mut s=String::from("<!-- generated by kct docs sync -->\n| Reference | Value | Position (mm) | Rotation | Side | Nets |\n|---|---|---|---|---|---|\n");
                for r in rows.as_array().unwrap() {
                    let clean = |v: &Value| {
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string())
                            .replace('|', "\\|")
                            .replace('\n', " ")
                    };
                    s += &format!(
                        "| {} | {} | {} | {} | {} | {} |\n",
                        clean(&r["reference"]),
                        clean(&r["value"]),
                        clean(&r["position"]),
                        clean(&r["rotation"]),
                        clean(&r["side"]),
                        clean(&r["nets"])
                    );
                }
                s
            }
            _ => bail!("derived output must be .json, .csv, or .md"),
        };
        let content = text.into_bytes();
        let stale = existing.as_deref() != Some(content.as_slice());
        plans.push(Planned {
            path,
            content,
            stale,
        });
    }
    Ok(plans)
}
