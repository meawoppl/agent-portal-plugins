//! Practical placement-constraint conflict detection.
use super::{parse_args, Globals};
use crate::schema::pcb::{Footprint, Pcb};
use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use serde::Serialize;
use serde_yaml::Value;
use std::{ffi::OsString, fs, path::PathBuf};

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Table,
    Json,
    Summary,
}
#[derive(Parser)]
struct Args {
    pcb: PathBuf,
    #[arg(long, value_enum, default_value = "table")]
    format: Format,
    #[arg(long)]
    keepout: Option<PathBuf>,
    #[arg(long)]
    constraints: Option<PathBuf>,
    #[arg(long)]
    auto_keepout: bool,
    #[arg(short, long)]
    verbose: bool,
}
#[derive(Clone, Serialize)]
struct Zone {
    name: String,
    kind: String,
    polygon: Vec<(f64, f64)>,
    layer: Option<String>,
}
#[derive(Serialize)]
struct Conflict {
    constraint1: Label,
    constraint2: Label,
    conflict_type: String,
    description: String,
    location: Option<(f64, f64)>,
    priority_winner: Option<String>,
    resolutions: Vec<Resolution>,
}
#[derive(Serialize)]
struct Label {
    #[serde(rename = "type")]
    kind: String,
    name: String,
}
#[derive(Serialize)]
struct Resolution {
    action: String,
    description: String,
    trade_off: String,
    priority: i32,
}

fn points(v: &Value) -> Vec<(f64, f64)> {
    v.as_sequence()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let p = p.as_sequence()?;
            Some((p.first()?.as_f64()?, p.get(1)?.as_f64()?))
        })
        .collect()
}
fn load_zones(path: &PathBuf) -> Result<Vec<Zone>> {
    let root: Value = serde_yaml::from_slice(&fs::read(path)?)
        .with_context(|| format!("invalid keepout YAML: {}", path.display()))?;
    let seq = root
        .get("keepouts")
        .or_else(|| root.get("zones"))
        .and_then(Value::as_sequence)
        .cloned()
        .unwrap_or_default();
    Ok(seq
        .iter()
        .map(|z| Zone {
            name: z
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unnamed")
                .into(),
            kind: z
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("clearance")
                .into(),
            polygon: points(z.get("polygon").unwrap_or(&Value::Null)),
            layer: z.get("layer").and_then(Value::as_str).map(str::to_owned),
        })
        .collect())
}
fn footprint_box(fp: &Footprint, pad: f64) -> (f64, f64, f64, f64) {
    if fp.pads.is_empty() {
        return (
            fp.position.0 - pad,
            fp.position.1 - pad,
            fp.position.0 + pad,
            fp.position.1 + pad,
        );
    }
    let mut x1 = f64::INFINITY;
    let mut y1 = f64::INFINITY;
    let mut x2 = f64::NEG_INFINITY;
    let mut y2 = f64::NEG_INFINITY;
    for p in &fp.pads {
        let x = fp.position.0 + p.position.0;
        let y = fp.position.1 + p.position.1;
        let hx = p.size.0 / 2.0 + pad;
        let hy = p.size.1 / 2.0 + pad;
        x1 = x1.min(x - hx);
        y1 = y1.min(y - hy);
        x2 = x2.max(x + hx);
        y2 = y2.max(y + hy)
    }
    (x1, y1, x2, y2)
}
fn auto_zones(p: &Pcb) -> Vec<Zone> {
    p.footprints()
        .iter()
        .filter(|f| {
            f.reference.starts_with('J')
                || f.reference.starts_with("MH")
                || f.name.to_ascii_lowercase().contains("mountinghole")
        })
        .map(|f| {
            let b = footprint_box(f, 1.0);
            Zone {
                name: format!("auto_{}", f.reference),
                kind: "mechanical".into(),
                polygon: vec![(b.0, b.1), (b.2, b.1), (b.2, b.3), (b.0, b.3)],
                layer: Some(f.layer.clone()),
            }
        })
        .collect()
}
fn bbox(z: &Zone) -> Option<(f64, f64, f64, f64)> {
    if z.polygon.len() < 3 {
        return None;
    }
    Some(z.polygon.iter().fold(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        |(x1, y1, x2, y2), p| (x1.min(p.0), y1.min(p.1), x2.max(p.0), y2.max(p.1)),
    ))
}
fn overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> Option<(f64, f64)> {
    let x1 = a.0.max(b.0);
    let y1 = a.1.max(b.1);
    let x2 = a.2.min(b.2);
    let y2 = a.3.min(b.3);
    (x1 < x2 && y1 < y2).then_some(((x1 + x2) / 2.0, (y1 + y2) / 2.0))
}
fn keepout_conflicts(z: &[Zone]) -> Vec<Conflict> {
    let mut out = vec![];
    for i in 0..z.len() {
        for j in i + 1..z.len() {
            if z[i].layer.is_some() && z[j].layer.is_some() && z[i].layer != z[j].layer {
                continue;
            }
            if let (Some(a), Some(b)) = (bbox(&z[i]), bbox(&z[j])) {
                if let Some(at) = overlap(a, b) {
                    out.push(Conflict {
                        constraint1: Label {
                            kind: "keepout".into(),
                            name: z[i].name.clone(),
                        },
                        constraint2: Label {
                            kind: "keepout".into(),
                            name: z[j].name.clone(),
                        },
                        conflict_type: "overlap".into(),
                        description: format!(
                            "Keepout zones '{}' and '{}' overlap",
                            z[i].name, z[j].name
                        ),
                        location: Some(at),
                        priority_winner: None,
                        resolutions: vec![Resolution {
                            action: "shrink_keepout".into(),
                            description:
                                "Reduce one keepout boundary or move the protected feature".into(),
                            trade_off: "Reduces mechanical clearance".into(),
                            priority: 0,
                        }],
                    })
                }
            }
        }
    }
    out
}
fn grouping_conflicts(path: &PathBuf, p: &Pcb) -> Result<Vec<Conflict>> {
    let root: Value = serde_yaml::from_slice(&fs::read(path)?)?;
    let mut out = vec![];
    for g in root
        .get("groups")
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
    {
        let name = g.get("name").and_then(Value::as_str).unwrap_or("unnamed");
        let members: Vec<_> = g
            .get("members")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        for c in g
            .get("constraints")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
        {
            if c.get("type").and_then(Value::as_str) != Some("max_distance") {
                continue;
            }
            let r = c.get("radius_mm").and_then(Value::as_f64).unwrap_or(10.0);
            let anchor = c
                .get("anchor")
                .and_then(Value::as_str)
                .or_else(|| members.first().copied())
                .unwrap_or("");
            let Some(a) = p.footprints().iter().find(|f| f.reference == anchor) else {
                continue;
            };
            for m in &members {
                let Some(b) = p.footprints().iter().find(|f| f.reference == *m) else {
                    continue;
                };
                let d = (a.position.0 - b.position.0).hypot(a.position.1 - b.position.1);
                if d > r {
                    out.push(Conflict {
                        constraint1: Label {
                            kind: "grouping".into(),
                            name: name.into(),
                        },
                        constraint2: Label {
                            kind: "placement".into(),
                            name: (*m).into(),
                        },
                        conflict_type: "impossible".into(),
                        description: format!("{m} is {d:.2} mm from {anchor}, exceeding {r:.2} mm"),
                        location: Some(b.position),
                        priority_winner: Some(name.into()),
                        resolutions: vec![Resolution {
                            action: "move_component".into(),
                            description: format!("Move {m} within {r:.2} mm of {anchor}"),
                            trade_off: "May increase other ratsnest lengths".into(),
                            priority: 1,
                        }],
                    })
                }
            }
        }
    }
    Ok(out)
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<Args>("constraints", args);
    if !a.pcb.exists() {
        anyhow::bail!("PCB file not found: {}", a.pcb.display())
    }
    if a.pcb.extension().and_then(|x| x.to_str()) != Some("kicad_pcb") {
        anyhow::bail!("Expected .kicad_pcb file")
    };
    let pcb = Pcb::load(&a.pcb)?;
    let mut zones = if let Some(k) = &a.keepout {
        load_zones(k)?
    } else {
        vec![]
    };
    if a.auto_keepout {
        zones.extend(auto_zones(&pcb))
    }
    let mut conflicts = keepout_conflicts(&zones);
    if let Some(c) = &a.constraints {
        conflicts.extend(grouping_conflicts(c, &pcb)?)
    }
    match a.format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"file":a.pcb,"summary":{"total_conflicts":conflicts.len()},"conflicts":conflicts,"keepout_zones":zones.len()})
            )?
        ),
        Format::Summary => println!(
            "{}: {} constraint conflict(s)",
            a.pcb.display(),
            conflicts.len()
        ),
        Format::Table => {
            println!("\n============================================================\nCONSTRAINT CONFLICT CHECK\n============================================================\nFile: {}\nConflicts: {}",a.pcb.file_name().unwrap_or_default().to_string_lossy(),conflicts.len());
            if conflicts.is_empty() {
                println!("\n============================================================\nNO CONFLICTS FOUND")
            } else {
                for (i, c) in conflicts.iter().enumerate() {
                    println!(
                        "\n  [!] Conflict #{}: {}\n      {}\n      {}:{} vs {}:{}",
                        i + 1,
                        c.conflict_type.to_uppercase(),
                        c.description,
                        c.constraint1.kind,
                        c.constraint1.name,
                        c.constraint2.kind,
                        c.constraint2.name
                    );
                    if a.verbose {
                        for r in &c.resolutions {
                            println!("        {}: {} ({})", r.action, r.description, r.trade_off)
                        }
                    }
                }
            }
        }
    }
    Ok(if conflicts.is_empty() { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_overlap() {
        let z = vec![
            Zone {
                name: "a".into(),
                kind: "x".into(),
                polygon: vec![(0., 0.), (2., 0.), (2., 2.)],
                layer: None,
            },
            Zone {
                name: "b".into(),
                kind: "x".into(),
                polygon: vec![(1., 1.), (3., 1.), (3., 3.)],
                layer: None,
            },
        ];
        assert_eq!(keepout_conflicts(&z).len(), 1)
    }
}
