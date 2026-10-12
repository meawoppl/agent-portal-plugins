//! Transactional native edits with exact pad coordinates.
use crate::{
    schema::pcb::{Pcb, TraceOptions, ViaOptions},
    sexp::SExp,
    validate::{DRCChecker, DRCCheckerOptions},
};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub operations: Vec<Operation>,
}
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Endpoint {
    Pad(String),
    Point([f64; 2]),
}
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Operation {
    Move {
        reference: String,
        at: [f64; 2],
        rotation: Option<f64>,
    },
    DeleteTracks {
        net: String,
        layer: String,
        path: Vec<Endpoint>,
        #[serde(default = "tolerance")]
        tolerance: f64,
    },
    AddTrack {
        net: String,
        layer: String,
        width: f64,
        path: Vec<Endpoint>,
    },
    AddVia {
        net: String,
        at: Endpoint,
        size: f64,
        drill: f64,
        #[serde(default = "via_layers")]
        layers: Vec<String>,
    },
    AddZone {
        net: String,
        layer: String,
        priority: i64,
        rect: [f64; 4],
    },
}
fn tolerance() -> f64 {
    0.000001
}
fn via_layers() -> Vec<String> {
    vec!["F.Cu".into(), "B.Cu".into()]
}
fn finite(v: &[f64]) -> Result<()> {
    if !v.iter().all(|n| n.is_finite()) {
        bail!("coordinates must be finite")
    }
    Ok(())
}
fn positive(v: f64) -> Result<()> {
    if !v.is_finite() || v <= 0. {
        bail!("dimensions must be positive and finite")
    }
    Ok(())
}
fn layer(p: &Pcb, l: &str) -> Result<()> {
    if !p.copper_layers().iter().any(|a| a.name == l) {
        bail!("unknown copper layer {l}")
    }
    Ok(())
}
fn net(p: &Pcb, n: &str) -> Result<()> {
    if n.is_empty() || p.get_net_by_name(n).is_none() {
        bail!("unknown net {n}")
    }
    Ok(())
}
pub fn resolve(p: &Pcb, e: &Endpoint, n: &str) -> Result<(f64, f64)> {
    match e {
        Endpoint::Point([x, y]) => {
            finite(&[*x, *y])?;
            Ok((*x, *y))
        }
        Endpoint::Pad(s) => {
            let (reference, pin) = s
                .rsplit_once('.')
                .context("pad reference must be REF.PAD")?;
            let fp = p.get_footprint(reference).context("unknown footprint")?;
            let pad = fp
                .pads
                .iter()
                .find(|pad| pad.number == pin)
                .context("unknown pad")?;
            if pad.net_name != n {
                bail!("pad {s} belongs to {}, not {n}", pad.net_name)
            }
            p.get_pad_position(reference, pin)
                .context("pad position unavailable")
        }
    }
}
fn path(p: &Pcb, ends: &[Endpoint], n: &str) -> Result<Vec<(f64, f64)>> {
    if ends.len() < 2 {
        bail!("path needs at least two endpoints")
    }
    ends.iter().map(|e| resolve(p, e, n)).collect()
}
fn simplify(points: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for c in points {
        if out.last() == Some(&c) {
            continue;
        }
        while out.len() >= 2 {
            let a = out[out.len() - 2];
            let b = out[out.len() - 1];
            let cross = (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0);
            let dot = (b.0 - a.0) * (c.0 - b.0) + (b.1 - a.1) * (c.1 - b.1);
            if cross.abs() > 1e-12 || dot < 0. {
                break;
            }
            out.pop();
        }
        out.push(c);
    }
    out
}
/// Return a separate candidate; failures never modify the input board.
pub fn apply(original: &Pcb, plan: &Plan) -> Result<Pcb> {
    if plan.version != 1 {
        bail!("unsupported edit plan version {}", plan.version)
    }
    let mut p = Pcb::new(
        original.sexp().clone(),
        original.path().map(std::path::Path::to_path_buf),
    )?;
    for op in &plan.operations {
        match op {
            Operation::Move {
                reference,
                at,
                rotation,
            } => {
                finite(at)?;
                if let Some(r) = rotation {
                    finite(&[*r])?;
                }
                if !p.update_footprint_position(reference, at[0], at[1], *rotation) {
                    bail!("unknown footprint {reference}")
                }
            }
            Operation::DeleteTracks {
                net: n,
                layer: l,
                path: ends,
                tolerance: t,
            } => {
                net(&p, n)?;
                layer(&p, l)?;
                positive(*t)?;
                let points = path(&p, ends, n)?;
                let close = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1) <= *t;
                let mut removed = Vec::new();
                for pair in points.windows(2) {
                    let matches: Vec<_> = p
                        .segments()
                        .iter()
                        .filter(|s| {
                            s.net_name == *n
                                && s.layer == *l
                                && ((close(s.start, pair[0]) && close(s.end, pair[1]))
                                    || (close(s.start, pair[1]) && close(s.end, pair[0])))
                        })
                        .cloned()
                        .collect();
                    if matches.is_empty() {
                        bail!("no track matches requested deletion endpoints")
                    }
                    removed.extend(matches);
                }
                p.remove_segments(&removed);
            }
            Operation::AddTrack {
                net: n,
                layer: l,
                width,
                path: ends,
            } => {
                net(&p, n)?;
                layer(&p, l)?;
                positive(*width)?;
                let points = simplify(path(&p, ends, n)?);
                if points.len() < 2 {
                    bail!("track has zero length")
                }
                p.add_trace(
                    points[0],
                    *points.last().unwrap(),
                    TraceOptions {
                        net: Some(n.clone()),
                        layer: l.clone(),
                        width: *width,
                        waypoints: points[1..points.len() - 1].to_vec(),
                        dedupe: true,
                    },
                )?;
            }
            Operation::AddVia {
                net: n,
                at,
                size,
                drill,
                layers,
            } => {
                net(&p, n)?;
                positive(*size)?;
                positive(*drill)?;
                if drill >= size {
                    bail!("via drill must be smaller than its diameter")
                }
                if layers.len() != 2 || layers[0] == layers[1] {
                    bail!("via needs two distinct copper layers")
                }
                for l in layers {
                    layer(&p, l)?;
                }
                let at = resolve(&p, at, n)?;
                p.add_via(
                    at.0,
                    at.1,
                    ViaOptions {
                        net: Some(n.clone()),
                        size: *size,
                        drill: *drill,
                        layers: layers.clone(),
                        dedupe: true,
                    },
                );
            }
            Operation::AddZone {
                net: n,
                layer: l,
                priority,
                rect,
            } => {
                net(&p, n)?;
                layer(&p, l)?;
                finite(rect)?;
                if rect[0] >= rect[2] || rect[1] >= rect[3] || *priority < 0 {
                    bail!("invalid zone rectangle or priority")
                }
                let number = p.get_net_by_name(n).unwrap().number;
                let origin = p.board_origin();
                let [x0, y0, x1, y1] = *rect;
                let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| {
                    SExp::list("xy", [SExp::atom(x + origin.0), SExp::atom(y + origin.1)])
                });
                let net_node = if p.net_name_only_dialect() {
                    SExp::pair("net", n.clone())
                } else {
                    SExp::pair("net", number)
                };
                p.sexp_mut().push(SExp::list(
                    "zone",
                    [
                        net_node,
                        SExp::pair("net_name", n.clone()),
                        SExp::pair("layer", l.clone()),
                        SExp::pair("uuid", uuid::Uuid::new_v4().to_string()),
                        SExp::pair("priority", *priority),
                        SExp::list("hatch", [SExp::symbol("edge"), SExp::atom(0.5)]),
                        SExp::list("connect_pads", [SExp::pair("clearance", 0.2)]),
                        SExp::pair("min_thickness", 0.25),
                        SExp::list(
                            "fill",
                            [
                                SExp::symbol("yes"),
                                SExp::pair("thermal_gap", 0.3),
                                SExp::pair("thermal_bridge_width", 0.3),
                            ],
                        ),
                        SExp::list("polygon", [SExp::list("pts", pts)]),
                    ],
                ));
                p.reload_from_tree()?;
            }
        }
    }
    Ok(p)
}
pub fn clearance_findings(p: &Pcb) -> Result<Vec<Value>> {
    let checker = DRCChecker::new(p, DRCCheckerOptions::default())?;
    let mut r = checker.check_clearances();
    r.violations
        .extend(checker.check_segment_zone_clearances().violations);
    r.violations
        .extend(checker.check_via_zone_clearances().violations);
    Ok(r.violations.iter().map(|v|json!({"rule":v.rule_id,"severity":v.severity,"message":v.message,"at":v.location,"nets":v.nets,"items":v.items,"actual":v.actual_value,"required":v.required_value})).collect())
}
pub fn added(before: &[Value], after: Vec<Value>) -> Vec<Value> {
    after.into_iter().filter(|f| !before.contains(f)).collect()
}
