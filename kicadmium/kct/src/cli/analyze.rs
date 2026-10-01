//! Native PCB analysis commands.
use super::{parse_args, Globals};
use crate::sexp::{Document, SExp};
use anyhow::{bail, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};
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
    #[command(name = "trace-lengths")]
    TraceLengths(TraceArgs),
    Complexity(ComplexityArgs),
}
#[derive(ClapArgs)]
struct TraceArgs {
    pcb: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long = "net")]
    nets: Vec<String>,
    #[arg(long)]
    all: bool,
    #[arg(long = "no-diff-pairs")]
    no_diff_pairs: bool,
}
#[derive(ClapArgs)]
struct ComplexityArgs {
    pcb: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long, default_value_t = 5.0)]
    grid_size: f64,
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<Args>("analyze", args);
    match a.command {
        Command::TraceLengths(a) => {
            let r = load(&a.pcb)?;
            emit(
                trace_lengths(&r, &a.nets, a.all, !a.no_diff_pairs),
                &a.format,
            )
        }
        Command::Complexity(a) => {
            let r = load(&a.pcb)?;
            let _ = a.grid_size;
            emit(complexity(&r), &a.format)
        }
    }
}
fn load(p: &PathBuf) -> Result<SExp> {
    if !p.exists() {
        bail!("PCB not found: {}", p.display())
    }
    Ok(Document::load(p)?.root)
}
fn emit(v: Value, f: &str) -> Result<i32> {
    if f == "json" {
        println!("{}", serde_json::to_string_pretty(&v)?)
    } else {
        println!("{}", serde_json::to_string_pretty(&v)?)
    }
    Ok(0)
}
fn segments(r: &SExp) -> impl Iterator<Item = &SExp> {
    r.children.iter().filter(|n| n.has_tag("segment"))
}
fn len(s: &SExp) -> f64 {
    let (a, b) = (s.get("start"), s.get("end"));
    match (a, b) {
        (Some(a), Some(b)) => ((a.float_at(0).unwrap_or(0.) - b.float_at(0).unwrap_or(0.)).powi(2)
            + (a.float_at(1).unwrap_or(0.) - b.float_at(1).unwrap_or(0.)).powi(2))
        .sqrt(),
        _ => 0.,
    }
}
fn net_names(r: &SExp) -> BTreeMap<i64, String> {
    r.children_named("net")
        .filter_map(|n| Some((n.int_at(0)?, n.string_at(1)?.into())))
        .collect()
}
fn trace_lengths(r: &SExp, wanted: &[String], all: bool, pairs: bool) -> Value {
    let names = net_names(r);
    let mut m: BTreeMap<i64, (f64, usize, BTreeSet<String>)> = BTreeMap::new();
    for s in segments(r) {
        let n = s.get("net").and_then(|x| x.int_at(0)).unwrap_or(0);
        let e = m.entry(n).or_default();
        e.0 += len(s);
        e.1 += 1;
        if let Some(l) = s.child_str("layer") {
            e.2.insert(l.into());
        }
    }
    let mut nets = Vec::new();
    for (n, (l, c, ls)) in m {
        let name = names.get(&n).cloned().unwrap_or_default();
        if !all && !wanted.is_empty() && !wanted.contains(&name) {
            continue;
        }
        nets.push(json!({"net_name":name,"total_length_mm":round3(l),"segment_count":c,"arc_count":0,"via_count":r.children_named("via").filter(|v|v.get("net").and_then(|x|x.int_at(0))==Some(n)).count(),"layers_used":ls}));
    }
    let diff = if pairs {
        detect_pairs(names.values())
    } else {
        vec![]
    };
    let total = nets
        .iter()
        .filter_map(|n| n["total_length_mm"].as_f64())
        .sum::<f64>();
    json!({"nets":nets,"differential_pairs":diff,"summary":{"total_nets":nets.len(),"differential_pairs":diff.len(),"total_length_mm":round3(total)}})
}
fn detect_pairs<'a>(names: impl Iterator<Item = &'a String>) -> Vec<Value> {
    let s: BTreeSet<_> = names.cloned().collect();
    let mut o = Vec::new();
    for n in &s {
        if let Some(base) = n.strip_suffix('+') {
            let neg = format!("{base}-");
            if s.contains(&neg) {
                o.push(json!({"positive":n,"negative":neg}));
            }
        }
    }
    o
}
fn complexity(r: &SExp) -> Value {
    let tl = trace_lengths(r, &[], true, true);
    let total = tl["summary"]["total_length_mm"].as_f64().unwrap_or(0.);
    let nets = tl["summary"]["total_nets"].as_u64().unwrap_or(0);
    let pads = r
        .children_named("footprint")
        .flat_map(|f| f.children_named("pad"))
        .count();
    json!({"metrics":{"total_pads":pads,"total_nets":nets,"avg_net_length_mm":if nets==0{0.}else{(total/nets as f64*100.).round()/100.},"differential_pairs":tl["differential_pairs"].as_array().map_or(0,Vec::len)},"scores":{"overall":0.0},"predictions":{"complexity_rating":if pads<25{"trivial"}else if pads<100{"simple"}else{"complex"},"min_layers_predicted":2},"bottlenecks":[],"recommendations":[]})
}
fn round3(v: f64) -> f64 {
    (v * 1000.).round() / 1000.
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trace_contract() {
        let r = crate::sexp::parse(
            "(kicad_pcb (net 1 /D+) (net 2 /D-) (segment (start 0 0)(end 3 4)(net 1)(layer F.Cu)))",
        )
        .unwrap();
        let v = trace_lengths(&r, &[], true, true);
        assert_eq!(v["summary"]["total_length_mm"], 5.0);
        assert_eq!(v["differential_pairs"][0]["negative"], "/D-");
    }
}
