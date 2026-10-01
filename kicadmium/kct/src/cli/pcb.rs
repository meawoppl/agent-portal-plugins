//! Native PCB query commands (upstream `cli.commands.pcb` / `pcb_query`).
use super::{parse_args, Globals};
use crate::sexp::{Document, SExp};
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::PathBuf,
};

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Summary(Query),
    Footprints(FilterQuery),
    Nets(NetQuery),
    Padmap(PadmapQuery),
    Traces(TraceQuery),
    Stackup(Query),
    Zones(Query),
}
#[derive(clap::Args)]
struct Query {
    pcb: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
}
#[derive(clap::Args)]
struct FilterQuery {
    pcb: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long = "filter", alias = "pattern")]
    pattern: Option<String>,
    #[arg(long)]
    sorted: bool,
}
#[derive(clap::Args)]
struct NetQuery {
    pcb: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long = "filter", alias = "pattern")]
    pattern: Option<String>,
    #[arg(long)]
    sorted: bool,
    #[arg(long)]
    check_connectivity: bool,
}
#[derive(clap::Args)]
struct PadmapQuery {
    pcb: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long)]
    r#ref: Option<String>,
    #[arg(long)]
    net: Option<String>,
}
#[derive(clap::Args)]
struct TraceQuery {
    pcb: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long)]
    layer: Option<String>,
}

pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<Args>("pcb", args);
    match a.command {
        Command::Summary(q) => emit(query_summary(&load(&q.pcb)?), &q.format),
        Command::Footprints(q) => emit(
            filter(query_footprints(&load(&q.pcb)?), q.pattern, q.sorted),
            &q.format,
        ),
        Command::Nets(q) => {
            let _ = q.check_connectivity;
            emit(
                filter(query_nets(&load(&q.pcb)?), q.pattern, q.sorted),
                &q.format,
            )
        }
        Command::Padmap(q) => emit(
            query_padmap(&load(&q.pcb)?, q.r#ref.as_deref(), q.net.as_deref()),
            &q.format,
        ),
        Command::Traces(q) => emit(query_traces(&load(&q.pcb)?, q.layer.as_deref()), &q.format),
        Command::Stackup(q) => emit(query_stackup(&load(&q.pcb)?), &q.format),
        Command::Zones(q) => emit(query_zones(&load(&q.pcb)?), &q.format),
    }
}
fn load(p: &PathBuf) -> Result<SExp> {
    if !p.exists() {
        bail!("File not found: {}", p.display())
    }
    Ok(Document::load(p)?.root)
}
/// Structured board summary used by MCP without capturing process output.
pub fn summary_path(path: &std::path::Path) -> Result<Value> {
    let path = path.to_path_buf();
    Ok(query_summary(&load(&path)?))
}
fn emit(v: Value, format: &str) -> Result<i32> {
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        match v {
            Value::String(s) => println!("{s}"),
            _ => println!("{}", serde_json::to_string_pretty(&v)?),
        }
    }
    Ok(0)
}
fn filter(mut v: Value, pat: Option<String>, sorted: bool) -> Value {
    if let Value::Array(ref mut xs) = v {
        if let Some(p) = pat {
            xs.retain(|x| x.to_string().contains(&p));
        }
        if sorted {
            xs.sort_by_key(|x| x.to_string());
        }
    }
    v
}
fn nodes<'a>(r: &'a SExp, name: &'a str) -> impl Iterator<Item = &'a SExp> + 'a {
    r.children.iter().filter(move |n| n.has_tag(name))
}
fn layer(n: &SExp) -> String {
    n.child_str("layer").unwrap_or("").to_string()
}
fn zone_layers(zone: &SExp) -> Vec<String> {
    if let Some(layer) = zone.child_str("layer") {
        return vec![layer.to_owned()];
    }
    zone.get("layers")
        .map(|layers| {
            (0..layers.children.len())
                .filter_map(|i| layers.string_at(i).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
fn property<'a>(n: &'a SExp, k: &str) -> &'a str {
    n.property(k)
        .or_else(|| {
            let kind = if k == "Reference" {
                "reference"
            } else if k == "Value" {
                "value"
            } else {
                return None;
            };
            n.find_all("fp_text")
                .find(|text| text.string_at(0) == Some(kind))
                .and_then(|text| text.string_at(1))
        })
        .unwrap_or("")
}
fn query_summary(r: &SExp) -> Value {
    let (x0, y0, x1, y1) = edge_bounds(r).unwrap_or((0., 0., 0., 0.));
    let segs: Vec<_> = nodes(r, "segment").collect();
    let trace: f64 = segs
        .iter()
        .map(|s| distance(s.get("start"), s.get("end")))
        .sum();
    let nets = nodes(r, "net").filter(|n| n.int_at(0) != Some(0)).count();
    let title = r.get("title_block");
    let copper = r
        .get("layers")
        .map(|l| {
            l.children
                .iter()
                .filter(|x| x.string_at(0).is_some_and(|s| s.ends_with(".Cu")))
                .count()
        })
        .unwrap_or(0);
    json!({"title":title.and_then(|n|n.child_str("title")).unwrap_or(""),"revision":title.and_then(|n|n.child_str("rev")).unwrap_or(""),"width_mm":round2(x1-x0),"height_mm":round2(y1-y0),"area_mm2":round2((x1-x0)*(y1-y0)),"copper_layers":copper,"footprints":nodes(r,"footprint").count(),"nets":nets+1,"segments":segs.len(),"arcs":nodes(r,"arc").count(),"vias":nodes(r,"via").count(),"zones":nodes(r,"zone").count(),"trace_length_mm":round2(trace)})
}
fn query_footprints(r: &SExp) -> Value {
    let (ox, oy, _, _) = edge_bounds(r).unwrap_or_default();
    Value::Array(nodes(r,"footprint").map(|f|{let at=f.get("at");json!({"reference":property(f,"Reference"),"value":property(f,"Value"),"footprint":f.string_at(0).unwrap_or(""),"layer":layer(f),"position":{"x":at.and_then(|x|x.float_at(0)).unwrap_or(0.)-ox,"y":at.and_then(|x|x.float_at(1)).unwrap_or(0.)-oy},"rotation":at.and_then(|x|x.float_at(2)).unwrap_or(0.),"pads":nodes(f,"pad").count()})}).collect())
}
fn query_nets(r: &SExp) -> Value {
    let mut out = Vec::new();
    for n in nodes(r, "net").filter(|n| n.int_at(0) != Some(0)) {
        let no = n.int_at(0).unwrap_or(0);
        let seg = nodes(r, "segment")
            .filter(|s| s.get("net").and_then(|x| x.int_at(0)) == Some(no))
            .count();
        let vias = nodes(r, "via")
            .filter(|s| s.get("net").and_then(|x| x.int_at(0)) == Some(no))
            .count();
        let name = n.string_at(1).unwrap_or("");
        let zl: Vec<_> = nodes(r, "zone")
            .filter(|z| {
                z.get("net")
                    .is_some_and(|x| x.int_at(0) == Some(no) || x.string_at(0) == Some(name))
            })
            .flat_map(zone_layers)
            .collect();
        out.push(json!({"number":no,"name":n.string_at(1).unwrap_or(""),"segments":seg,"vias":vias,"zone_connected":!zl.is_empty(),"zone_layers":zl}));
    }
    Value::Array(out)
}
fn query_padmap(r: &SExp, rf: Option<&str>, net: Option<&str>) -> Value {
    let mut rows=nodes(r,"footprint").filter_map(|f|{let reference=property(f,"Reference");if rf.is_some_and(|x|x!=reference){return None}let pads:Vec<_>=nodes(f,"pad").filter_map(|p|{let nn=p.get("net").and_then(|n|n.string_at(1)).unwrap_or("");if net.is_some_and(|x|x!=nn){None}else{Some(json!({"number":p.string_at(0).unwrap_or(""),"net":nn}))}}).collect();if net.is_some()&&pads.is_empty(){return None}Some(json!({"reference":reference,"value":property(f,"Value"),"footprint":f.string_at(0).unwrap_or(""),"pads":pads}))}).collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        natural_ref(a["reference"].as_str().unwrap_or(""))
            .cmp(&natural_ref(b["reference"].as_str().unwrap_or("")))
    });
    Value::Array(rows)
}
fn natural_ref(s: &str) -> (&str, u64) {
    let i = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
    (&s[..i], s[i..].parse().unwrap_or(0))
}
fn query_traces(r: &SExp, wanted: Option<&str>) -> Value {
    let mut m: BTreeMap<String, (usize, f64, BTreeSet<String>)> = BTreeMap::new();
    for s in nodes(r, "segment") {
        let l = layer(s);
        if wanted.is_some_and(|x| x != l) {
            continue;
        }
        let e = m.entry(l).or_default();
        e.0 += 1;
        e.1 += distance(s.get("start"), s.get("end"));
        if let Some(w) = s.child_f64("width") {
            e.2.insert(format!("{w}"));
        }
    }
    Value::Object(m.into_iter().map(|(k,(n,len,ws))|(k,json!({"segments":n,"total_length_mm":round2(len),"widths_mm":ws.into_iter().filter_map(|x|x.parse::<f64>().ok()).collect::<Vec<_>>() }))).collect())
}
fn query_stackup(r: &SExp) -> Value {
    let Some(stack) = r.find("stackup") else {
        return Value::String("No stackup information available".into());
    };
    Value::Array(stack.children.iter().filter(|n|n.has_tag("layer")).map(|n|json!({"name":n.string_at(0).unwrap_or(""),"type":n.child_str("type"),"thickness_mm":n.child_f64("thickness")})).collect())
}
fn query_zones(r: &SExp) -> Value {
    let origin = edge_bounds(r).map(|x| (x.0, x.1)).unwrap_or_default();
    let netnos: BTreeMap<_, _> = nodes(r, "net")
        .filter_map(|n| Some((n.string_at(1)?.to_owned(), n.int_at(0)?)))
        .collect();
    let zs:Vec<_>=nodes(r,"zone").map(|z|{let pts:Vec<_>=z.find("polygon").map(|p|p.find_all("xy").collect()).unwrap_or_default();let xs:Vec<_>=pts.iter().filter_map(|p|p.float_at(0).map(|x|x-origin.0)).collect();let ys:Vec<_>=pts.iter().filter_map(|p|p.float_at(1).map(|y|y-origin.1)).collect();let bbox=if xs.is_empty(){Value::Null}else{json!({"min_x":xs.iter().copied().fold(f64::INFINITY,f64::min),"min_y":ys.iter().copied().fold(f64::INFINITY,f64::min),"max_x":xs.iter().copied().fold(f64::NEG_INFINITY,f64::max),"max_y":ys.iter().copied().fold(f64::NEG_INFINITY,f64::max)})};let nr=z.get("net");let name=nr.and_then(|n|n.string_at(0)).or_else(||z.child_str("net_name")).unwrap_or("");let no=nr.and_then(|n|n.int_at(0)).or_else(||netnos.get(name).copied()).unwrap_or(0);json!({"net_number":no,"net_name":name,"layer":layer(z),"priority":z.get("priority").and_then(|n|n.int_at(0)).unwrap_or(0),"clearance":z.find("clearance").and_then(|n|n.float_at(0)).unwrap_or(0.),"thermal_gap":z.find("thermal_gap").and_then(|n|n.float_at(0)).unwrap_or(0.),"thermal_bridge_width":z.find("thermal_bridge_width").and_then(|n|n.float_at(0)).unwrap_or(0.),"is_filled":z.get("filled_polygon").is_some(),"fill_type":"solid","boundary_points":pts.len(),"bounding_box":bbox})}).collect();
    json!({"zones":zs,"count":zs.len()})
}
fn distance(a: Option<&SExp>, b: Option<&SExp>) -> f64 {
    match (a, b) {
        (Some(a), Some(b)) => ((b.float_at(0).unwrap_or(0.) - a.float_at(0).unwrap_or(0.)).powi(2)
            + (b.float_at(1).unwrap_or(0.) - a.float_at(1).unwrap_or(0.)).powi(2))
        .sqrt(),
        _ => 0.,
    }
}
fn round2(v: f64) -> f64 {
    (v * 100.).round() / 100.
}
fn edge_bounds(r: &SExp) -> Option<(f64, f64, f64, f64)> {
    let mut p = Vec::new();
    for g in &r.children {
        if g.child_str("layer") != Some("Edge.Cuts") {
            continue;
        }
        for k in ["start", "mid", "end", "center"] {
            if let Some(n) = g.get(k) {
                if let (Some(x), Some(y)) = (n.float_at(0), n.float_at(1)) {
                    p.push((x, y));
                }
            }
        }
    }
    if p.is_empty() {
        None
    } else {
        Some(p.into_iter().fold(
            (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ),
            |a, (x, y)| (a.0.min(x), a.1.min(y), a.2.max(x), a.3.max(y)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn summary_shape() {
        let r=crate::sexp::parse("(kicad_pcb (net 0 \"\") (net 1 A) (footprint X (property Reference R1) (property Value 1k) (layer F.Cu) (at 1 2) (pad 1 thru_hole circle)))").unwrap();
        let v = query_summary(&r);
        assert_eq!(v["footprints"], 1);
        assert_eq!(v["nets"], 2);
    }
}
