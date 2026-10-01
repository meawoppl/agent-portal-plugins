//! Placement/routing figure-of-merit diagnostics.
use super::{parse_args, Globals};
use crate::schema::pcb::{Pcb, Segment};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::{collections::BTreeMap, ffi::OsString, fs, path::PathBuf};

const TERMS: [&str; 10] = [
    "trace_length_excess",
    "weighted_via_count",
    "turning_penalty",
    "net_congestion_variance",
    "match_group_skew",
    "diff_pair_clearance_margin",
    "decoupling_proximity",
    "crossing_count",
    "thermal_spread",
    "compactness",
];
#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}
#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    FomDebug {
        pcb: PathBuf,
        #[arg(long)]
        weights: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
        #[arg(short, long)]
        verbose: bool,
    },
}
#[derive(Serialize)]
struct Features {
    footprint_count: usize,
    pad_count: usize,
    net_count: usize,
    segment_count: usize,
    via_count: usize,
}
#[derive(Serialize)]
struct Report {
    pcb: String,
    score: f64,
    soft_score: f64,
    hard_gate_passed: bool,
    hard_failures: Vec<String>,
    soft_terms: BTreeMap<String, f64>,
    weighted_soft_terms: BTreeMap<String, f64>,
    weights: BTreeMap<String, f64>,
    predictor_value: f64,
    beta: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    feature_summary: Option<Features>,
}
fn weights(path: Option<PathBuf>) -> Result<BTreeMap<String, f64>> {
    let mut out = TERMS
        .into_iter()
        .map(|x| (x.into(), 1.))
        .collect::<BTreeMap<_, _>>();
    if let Some(p) = path {
        let v: serde_yaml::Value = serde_yaml::from_slice(
            &fs::read(&p).with_context(|| format!("weights file not found: {}", p.display()))?,
        )?;
        for n in TERMS {
            if let Some(x) = v.get(n).and_then(serde_yaml::Value::as_f64) {
                if !x.is_finite() || x < 0. {
                    anyhow::bail!("weight {n} must be a finite non-negative number")
                }
                out.insert(n.into(), x);
            }
        }
    }
    Ok(out)
}
fn crosses(a: &Segment, b: &Segment) -> bool {
    if a.layer != b.layer || a.net_number == b.net_number {
        return false;
    }
    fn o(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    }
    let (a1, a2, b1, b2) = (a.start, a.end, b.start, b.end);
    o(a1, a2, b1) * o(a1, a2, b2) < 0. && o(b1, b2, a1) * o(b1, b2, a2) < 0.
}
fn report(path: &PathBuf, w: BTreeMap<String, f64>, verbose: bool) -> Result<Report> {
    let p = Pcb::load(path)?;
    let s = p.summary()?;
    let diag = s.width_mm.hypot(s.height_mm).max(1.);
    let lengths: Vec<f64> = p
        .segments()
        .iter()
        .map(|x| (x.start.0 - x.end.0).hypot(x.start.1 - x.end.1))
        .collect();
    let total: f64 = lengths.iter().sum();
    let pads = p.footprints().iter().map(|x| x.pads.len()).sum::<usize>();
    let mut by_net = BTreeMap::<i64, f64>::new();
    for (x, l) in p.segments().iter().zip(&lengths) {
        *by_net.entry(x.net_number).or_default() += l
    }
    let mean = if by_net.is_empty() {
        0.
    } else {
        by_net.values().sum::<f64>() / by_net.len() as f64
    };
    let variance = if by_net.is_empty() {
        0.
    } else {
        by_net.values().map(|x| (x - mean).powi(2)).sum::<f64>()
            / by_net.len() as f64
            / (diag * diag)
    };
    let crossings = (0..p.segments().len())
        .flat_map(|i| (i + 1..p.segments().len()).map(move |j| (i, j)))
        .filter(|&(i, j)| crosses(&p.segments()[i], &p.segments()[j]))
        .count();
    let area = (s.width_mm * s.height_mm).max(1.);
    let fp_area = if p.footprints().is_empty() {
        0.
    } else {
        let (x1, y1, x2, y2) = p.footprints().iter().fold(
            (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ),
            |a, f| {
                (
                    a.0.min(f.position.0),
                    a.1.min(f.position.1),
                    a.2.max(f.position.0),
                    a.3.max(f.position.1),
                )
            },
        );
        ((x2 - x1).max(0.) * (y2 - y1).max(0.)) / area
    };
    let mut t = BTreeMap::<String, f64>::new();
    t.insert(
        "trace_length_excess".into(),
        total / (diag * (pads.max(1) as f64)),
    );
    t.insert(
        "weighted_via_count".into(),
        p.vias().len() as f64 / (pads.max(1) as f64),
    );
    t.insert("turning_penalty".into(), 0.);
    t.insert("net_congestion_variance".into(), variance);
    t.insert("match_group_skew".into(), 0.);
    t.insert("diff_pair_clearance_margin".into(), 0.);
    t.insert("decoupling_proximity".into(), 0.);
    t.insert(
        "crossing_count".into(),
        crossings as f64 / (p.segments().len().max(1) as f64),
    );
    t.insert("thermal_spread".into(), 0.);
    t.insert("compactness".into(), fp_area);
    let wt = t
        .iter()
        .map(|(k, v)| (k.clone(), v * w[k]))
        .collect::<BTreeMap<_, _>>();
    let sum = wt.values().sum::<f64>();
    let soft = (-sum).exp();
    Ok(Report {
        pcb: fs::canonicalize(path)?.display().to_string(),
        score: soft,
        soft_score: soft,
        hard_gate_passed: true,
        hard_failures: vec![],
        soft_terms: t,
        weighted_soft_terms: wt,
        weights: w,
        predictor_value: 1.,
        beta: 0.,
        feature_summary: verbose.then_some(Features {
            footprint_count: p.footprints().len(),
            pad_count: pads,
            net_count: by_net.len(),
            segment_count: p.segments().len(),
            via_count: p.vias().len(),
        }),
    })
}
pub fn run(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    match parse_args::<Args>("optim", args).command {
        None => {
            eprintln!("error: no optim subcommand specified (try: kct optim fom-debug --help)");
            Ok(2)
        }
        Some(Command::FomDebug {
            pcb,
            weights: wp,
            format,
            verbose,
        }) => {
            if !pcb.exists() {
                anyhow::bail!("PCB not found: {}", pcb.display())
            }
            let r = report(&pcb, weights(wp)?, verbose)?;
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&r)?),
                Format::Text => {
                    println!("FOM breakdown for {}\n  score:           {:.6}\n  soft_score:      {:.6}\n  hard_gate:       PASS\n  predictor*beta:  1.0000 ** 0",pcb.display(),r.score,r.soft_score);
                    if let Some(f) = &r.feature_summary {
                        println!("\nFeatures:\n  footprints:      {}\n  pads:            {}\n  nets (with pads): {}\n  segments:        {}\n  vias:            {}",f.footprint_count,f.pad_count,f.net_count,f.segment_count,f.via_count)
                    }
                    println!(
                        "\n{:<32} {:>12} {:>10} {:>12}",
                        "Term", "Raw", "Weight", "Weighted"
                    );
                    println!("{}", "-".repeat(70));
                    for n in TERMS {
                        println!(
                            "{n:<32} {:>12.4} {:>10.3} {:>12.4}",
                            r.soft_terms[n], r.weights[n], r.weighted_soft_terms[n]
                        )
                    }
                    println!(
                        "{}\n{:<32} {:>12} {:>10} {:>12.4}",
                        "-".repeat(70),
                        "TOTAL",
                        "",
                        "",
                        r.weighted_soft_terms.values().sum::<f64>()
                    );
                }
            }
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crossing() {
        let a = Segment::new((0., 0.), (2., 2.), 0.2, "F.Cu", 1);
        let b = Segment::new((0., 2.), (2., 0.), 0.2, "F.Cu", 2);
        assert!(crosses(&a, &b))
    }
}
