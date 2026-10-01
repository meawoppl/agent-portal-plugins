//! Conservative, dependency-free creepage/clearance census.

use std::ffi::OsString;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Serialize;

use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;

#[derive(Parser)]
#[command(about = "Measure conservative conductor-to-conductor clearances")]
struct Args {
    pcb: PathBuf,
    #[arg(long = "net-class", default_value = "HV")]
    net_class: String,
    #[arg(long)]
    min: Option<f64>,
    #[arg(long)]
    standard: Option<String>,
    #[arg(long = "pollution-degree")]
    pollution_degree: Option<u8>,
    #[arg(long = "working-voltage")]
    working_voltage: Option<f64>,
    #[arg(long = "material-group", default_value = "IIIa")]
    material_group: String,
    #[arg(long = "voltage-map")]
    voltage_map: Option<PathBuf>,
    #[arg(long = "census-threshold", default_value_t = 30.0)]
    census_threshold: f64,
    #[arg(long = "waive-same-footprint")]
    waive_same_footprint: bool,
    #[arg(long, default_value = "table", value_parser = ["table", "json"])]
    format: String,
}

#[derive(Clone)]
struct Conductor {
    net: String,
    reference: String,
    x: f64,
    y: f64,
    radius: f64,
}

#[derive(Serialize)]
struct Pair {
    hv_net: String,
    other_net: String,
    clearance_mm: f64,
    required_mm: f64,
    margin_mm: f64,
    passed: bool,
    waived: bool,
    relationship: &'static str,
    hv_reference: String,
    other_reference: String,
}

#[derive(Serialize)]
struct Report {
    command: &'static str,
    board: String,
    method: &'static str,
    limitation: &'static str,
    hv_nets: Vec<String>,
    required_creepage_mm: f64,
    pairs: Vec<Pair>,
    passed: bool,
}

fn voltage_map(path: &PathBuf) -> Result<BTreeMap<String, f64>> {
    let raw: serde_json::Value = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )?;
    let obj = raw
        .get("nets")
        .unwrap_or(&raw)
        .as_object()
        .context("voltage map must be an object or contain a 'nets' object")?;
    obj.iter()
        .map(|(name, value)| {
            let voltage = value
                .as_f64()
                .or_else(|| value.get("voltage").and_then(serde_json::Value::as_f64))
                .or_else(|| {
                    let lo = value.get("lo")?.as_f64()?;
                    let hi = value.get("hi")?.as_f64()?;
                    Some(lo.abs().max(hi.abs()))
                })
                .with_context(|| format!("invalid voltage for net {name}"))?;
            Ok((name.clone(), voltage))
        })
        .collect()
}

fn standard_min(standard: &str, voltage: f64, pd: u8, group: &str) -> Result<f64> {
    if !matches!(
        standard.to_ascii_lowercase().as_str(),
        "iec60664" | "iec62368"
    ) {
        bail!("unsupported standard {standard:?}; expected iec60664 or iec62368");
    }
    if !(1..=3).contains(&pd) || voltage < 0.0 || voltage > 1000.0 {
        bail!("standard lookup outside supported range (PD 1..3, 0..1000 V)");
    }
    let base = match voltage {
        v if v <= 32.0 => 0.53,
        v if v <= 50.0 => 1.2,
        v if v <= 100.0 => 1.4,
        v if v <= 160.0 => 1.6,
        v if v <= 250.0 => 2.5,
        v if v <= 400.0 => 4.0,
        v if v <= 630.0 => 6.3,
        _ => 10.0,
    };
    let pd_factor = [0.0, 0.5, 1.0, 1.6][pd as usize];
    let material = match group.to_ascii_lowercase().as_str() {
        "i" => 0.65,
        "ii" => 0.8,
        "iiia" | "iii" | "3a" => 1.0,
        "iiib" | "3b" => 1.25,
        _ => bail!("unknown material group {group:?}"),
    };
    Ok(base * pd_factor * material)
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<Args>("creepage", args);
    let map = args.voltage_map.as_ref().map(voltage_map).transpose()?;
    let derived = match (&args.standard, args.working_voltage, args.pollution_degree) {
        (Some(standard), Some(voltage), Some(pd)) => {
            Some(standard_min(standard, voltage, pd, &args.material_group)?)
        }
        (Some(_), _, _) if map.is_none() => {
            bail!("--standard requires --working-voltage and --pollution-degree")
        }
        (Some(_), _, None) => bail!("--standard with --voltage-map requires --pollution-degree"),
        _ => None,
    };
    let required = args.min.unwrap_or(0.0).max(derived.unwrap_or(0.0));
    if required <= 0.0 && map.is_none() {
        bail!("provide --min or --standard with voltage inputs");
    }
    let pcb = Pcb::load(&args.pcb)?;
    let mut conductors = Vec::new();
    for fp in pcb.footprints() {
        for pad in &fp.pads {
            if pad.net_number == 0 || pad.net_name.is_empty() {
                continue;
            }
            if let Some((x, y)) = pcb.get_pad_position(&fp.reference, &pad.number) {
                conductors.push(Conductor {
                    net: pad.net_name.clone(),
                    reference: fp.reference.clone(),
                    x,
                    y,
                    radius: pad.size.0.max(pad.size.1) / 2.0,
                });
            }
        }
    }
    for via in pcb.vias() {
        if via.net_number != 0 && !via.net_name.is_empty() {
            conductors.push(Conductor {
                net: via.net_name.clone(),
                reference: "via".into(),
                x: via.position.0,
                y: via.position.1,
                radius: via.size / 2.0,
            });
        }
    }
    let hv_nets: BTreeSet<String> = pcb
        .nets()
        .iter()
        .filter(|net| {
            net.name
                .to_ascii_uppercase()
                .contains(&args.net_class.to_ascii_uppercase())
                || map
                    .as_ref()
                    .and_then(|m| m.get(&net.name))
                    .is_some_and(|v| v.abs() >= args.census_threshold)
        })
        .map(|net| net.name.clone())
        .collect();
    let mut best: BTreeMap<(String, String), Pair> = BTreeMap::new();
    for a in conductors.iter().filter(|c| hv_nets.contains(&c.net)) {
        for b in conductors.iter().filter(|c| c.net != a.net) {
            let clearance = ((a.x - b.x).hypot(a.y - b.y) - a.radius - b.radius).max(0.0);
            let pair_required = if let (Some(standard), Some(map), Some(pd)) =
                (&args.standard, &map, args.pollution_degree)
            {
                standard_min(
                    standard,
                    (map.get(&a.net).copied().unwrap_or(0.0)
                        - map.get(&b.net).copied().unwrap_or(0.0))
                    .abs(),
                    pd,
                    &args.material_group,
                )?
                .max(args.min.unwrap_or(0.0))
            } else {
                required
            };
            let same = a.reference != "via" && a.reference == b.reference;
            let waived = same && args.waive_same_footprint;
            let pair = Pair {
                hv_net: a.net.clone(),
                other_net: b.net.clone(),
                clearance_mm: clearance,
                required_mm: pair_required,
                margin_mm: clearance - pair_required,
                passed: clearance >= pair_required,
                waived,
                relationship: if same { "same_footprint" } else { "board" },
                hv_reference: a.reference.clone(),
                other_reference: b.reference.clone(),
            };
            let key = (pair.hv_net.clone(), pair.other_net.clone());
            if best
                .get(&key)
                .is_none_or(|old| clearance < old.clearance_mm)
            {
                best.insert(key, pair);
            }
        }
    }
    let pairs: Vec<_> = best.into_values().collect();
    let passed = pairs.iter().all(|p| p.passed || p.waived) && !hv_nets.is_empty();
    let report=Report { command:"creepage", board:args.pcb.display().to_string(), method:"conservative pad/via envelope clearance", limitation:"Slots, routed copper, zones, and true surface geodesics are not modeled; use native DRC and engineering review for certification.", hv_nets:hv_nets.into_iter().collect(), required_creepage_mm:required, pairs, passed };
    if args.format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("kct creepage: {}", args.pcb.display());
        println!(
            "HV nets: {}",
            if report.hv_nets.is_empty() {
                "(none)".into()
            } else {
                report.hv_nets.join(", ")
            }
        );
        for p in &report.pairs {
            println!(
                "{:<20} -> {:<20} {:7.3} mm (need {:7.3}) {}",
                p.hv_net,
                p.other_net,
                p.clearance_mm,
                p.required_mm,
                if p.passed || p.waived { "PASS" } else { "FAIL" }
            );
        }
        println!("{}", report.limitation);
    }
    Ok(if passed { 0 } else { 1 })
}
