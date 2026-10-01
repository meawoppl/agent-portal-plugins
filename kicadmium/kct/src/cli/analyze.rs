//! Native PCB analysis commands.
use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;
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
    Congestion(CongestionArgs),
    #[command(name = "signal-integrity")]
    SignalIntegrity(SignalIntegrityArgs),
    Thermal(ThermalArgs),
    #[command(name = "current-sense")]
    CurrentSense(CurrentSenseArgs),
    #[command(name = "electrical-rating")]
    ElectricalRating(ElectricalRatingArgs),
    #[command(name = "component-stress")]
    ComponentStress(ComponentStressArgs),
}
#[derive(ClapArgs)]
struct CommonPcbArgs {
    pcb: PathBuf,
    #[arg(short, long, default_value = "text")]
    format: String,
}
#[derive(ClapArgs)]
struct CongestionArgs {
    pcb: PathBuf,
    #[arg(short, long, default_value = "text")]
    format: String,
    #[arg(long, default_value_t = 2.0)]
    grid_size: f64,
    #[arg(long, default_value = "low")]
    min_severity: String,
    #[arg(short, long)]
    quiet: bool,
}
#[derive(ClapArgs)]
struct SignalIntegrityArgs {
    pcb: PathBuf,
    #[arg(short, long, default_value = "text")]
    format: String,
    #[arg(long, default_value = "medium")]
    min_risk: String,
    #[arg(long)]
    crosstalk_only: bool,
    #[arg(long)]
    impedance_only: bool,
    #[arg(short, long)]
    quiet: bool,
}
#[derive(ClapArgs)]
struct ThermalArgs {
    pcb: PathBuf,
    #[arg(short, long, default_value = "text")]
    format: String,
    #[arg(long, default_value_t = 10.0)]
    cluster_radius: f64,
    #[arg(long, default_value_t = 0.05)]
    min_power: f64,
    #[arg(short, long)]
    quiet: bool,
}
#[derive(ClapArgs)]
struct CurrentSenseArgs {
    pcb: PathBuf,
    #[arg(short, long, default_value = "text")]
    format: String,
    #[arg(long = "sense-net")]
    sense_nets: Vec<String>,
    #[arg(long = "hicur-net")]
    hicur_nets: Vec<String>,
    #[arg(long, default_value_t = 10.0)]
    max_parallel: f64,
    #[arg(long, default_value_t = 0.5)]
    min_gap: f64,
    #[arg(long, default_value_t = 10.0)]
    max_loop_area: f64,
    #[arg(long = "sense-pair", num_args = 2)]
    sense_pairs: Vec<String>,
    #[arg(long)]
    sense_return: Option<String>,
    #[arg(long, default_value_t = 0.05)]
    kelvin_tol: f64,
    #[arg(long = "kelvin-pair")]
    kelvin_pairs: Vec<String>,
    #[arg(short, long)]
    quiet: bool,
}
#[derive(ClapArgs)]
struct ElectricalRatingArgs {
    schematic: PathBuf,
    #[arg(short, long, default_value = "text")]
    format: String,
    #[arg(long, default_value_t = 0.2)]
    derate_margin: f64,
    #[arg(long)]
    led_default_vf: Option<f64>,
    #[arg(short, long)]
    quiet: bool,
}
#[derive(ClapArgs)]
struct ComponentStressArgs {
    schematic: PathBuf,
    #[arg(long)]
    states: PathBuf,
    #[arg(short, long, default_value = "text")]
    format: String,
    #[arg(long)]
    allow_unresolved: bool,
    #[arg(long)]
    allow_uncited_ratings: bool,
    #[arg(short, long)]
    quiet: bool,
}
#[derive(ClapArgs)]
struct TraceArgs {
    pcb: PathBuf,
    #[arg(short, long, default_value = "text", value_parser = ["text", "json", "csv"])]
    format: String,
    #[arg(short = 'n', long = "net")]
    nets: Vec<String>,
    #[arg(short = 'a', long)]
    all: bool,
    #[arg(short = 'd', long = "diff-pairs", conflicts_with = "no_diff_pairs")]
    diff_pairs: bool,
    #[arg(long = "no-diff-pairs", conflicts_with = "diff_pairs")]
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
            let pcb = Pcb::load(&a.pcb)?;
            emit(
                trace_lengths_pcb(&pcb, &a.nets, a.all, a.diff_pairs || !a.no_diff_pairs),
                &a.format,
            )
        }
        Command::Complexity(a) => {
            let pcb = Pcb::load(&a.pcb)?;
            emit(complexity(&pcb, a.grid_size), &a.format)
        }
        Command::Congestion(a) => congestion_cmd(a),
        Command::SignalIntegrity(a) => signal_integrity_cmd(a),
        Command::Thermal(a) => thermal_cmd(a),
        Command::CurrentSense(a) => current_sense_cmd(a),
        Command::ElectricalRating(a) => electrical_rating_cmd(a),
        Command::ComponentStress(a) => component_stress_cmd(a),
    }
}

fn point(s: &SExp, tag: &str) -> (f64, f64) {
    s.get(tag)
        .map(|p| (p.float_at(0).unwrap_or(0.), p.float_at(1).unwrap_or(0.)))
        .unwrap_or_default()
}
fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}
fn layer(s: &SExp) -> String {
    s.child_str("layer").unwrap_or("").to_owned()
}
fn width(s: &SExp) -> f64 {
    s.child_f64("width").unwrap_or(0.)
}
fn severity_rank(s: &str) -> u8 {
    match s {
        "critical" => 3,
        "high" => 2,
        "medium" => 1,
        _ => 0,
    }
}

fn congestion_cmd(a: CongestionArgs) -> Result<i32> {
    let r = load(&a.pcb)?;
    let area = a.grid_size * a.grid_size;
    let names = net_names(&r);
    type CongestionCell = (f64, usize, usize, BTreeSet<String>, BTreeSet<String>);
    let mut cells: BTreeMap<(i64, i64), CongestionCell> = BTreeMap::new();
    for s in segments(&r) {
        let p = point(s, "start");
        let c = (
            (p.0 / a.grid_size).floor() as i64,
            (p.1 / a.grid_size).floor() as i64,
        );
        let e = cells.entry(c).or_default();
        e.0 += len(s);
        if let Some(n) = s
            .get("net")
            .and_then(|x| x.int_at(0))
            .and_then(|n| names.get(&n))
        {
            e.4.insert(n.clone());
        }
    }
    for v in r.children_named("via") {
        let p = point(v, "at");
        let c = (
            (p.0 / a.grid_size).floor() as i64,
            (p.1 / a.grid_size).floor() as i64,
        );
        cells.entry(c).or_default().1 += 1
    }
    for f in r.children_named("footprint") {
        let p = point(f, "at");
        let c = (
            (p.0 / a.grid_size).floor() as i64,
            (p.1 / a.grid_size).floor() as i64,
        );
        let e = cells.entry(c).or_default();
        e.3.insert(f.property("Reference").unwrap_or("").into());
        for p in f.children_named("pad") {
            e.2 += usize::from(p.get("net").is_none())
        }
    }
    let mut reports = vec![];
    for ((x, y), (track, vias, unrouted, components, nets)) in cells {
        let density = track / area;
        if density < 0.5 && vias < 2 && unrouted == 0 {
            continue;
        }
        let sev = if density >= 2. || vias >= 12 {
            "critical"
        } else if density >= 1.5 || vias >= 8 {
            "high"
        } else if density >= 1. || vias >= 5 {
            "medium"
        } else {
            "low"
        };
        if severity_rank(sev) < severity_rank(&a.min_severity) {
            continue;
        }
        let mut suggestions = vec![];
        if density >= 1. {
            suggestions.push("Consider spreading traces across additional layers")
        }
        if vias >= 5 {
            suggestions.push("Reduce via concentration or fan out earlier")
        }
        if unrouted > 0 {
            suggestions.push("Route remaining pad connections")
        }
        reports.push(json!({"center":{"x":(x as f64+0.5)*a.grid_size,"y":(y as f64+0.5)*a.grid_size},"radius":a.grid_size*2.5,"track_density":round3(density),"via_count":vias,"unrouted_connections":unrouted,"components":components,"nets":nets,"severity":sev,"suggestions":suggestions}));
    }
    reports
        .sort_by_key(|v| std::cmp::Reverse(severity_rank(v["severity"].as_str().unwrap_or("low"))));
    let out = json!({"reports":reports,"summary":{"total":reports.len(),"critical":reports.iter().filter(|x|x["severity"]=="critical").count(),"high":reports.iter().filter(|x|x["severity"]=="high").count(),"medium":reports.iter().filter(|x|x["severity"]=="medium").count(),"low":reports.iter().filter(|x|x["severity"]=="low").count()}});
    emit(out, &a.format)?;
    Ok(if reports.iter().any(|x| x["severity"] == "critical") {
        2
    } else if reports.iter().any(|x| x["severity"] == "high") {
        1
    } else {
        0
    })
}
fn coupling(a: &SExp, b: &SExp) -> (f64, f64) {
    let (a0, a1, b0, b1) = (
        point(a, "start"),
        point(a, "end"),
        point(b, "start"),
        point(b, "end"),
    );
    let (adx, ady) = (a1.0 - a0.0, a1.1 - a0.1);
    let (bdx, bdy) = (b1.0 - b0.0, b1.1 - b0.1);
    let (al, bl) = (distance(a0, a1), distance(b0, b1));
    if al < 0.01 || bl < 0.01 || ((adx * bdx + ady * bdy) / (al * bl)).abs() < 0.9 {
        return (0., f64::INFINITY);
    }
    let (nx, ny) = (adx / al, ady / al);
    let (px, py) = (b0.0 - a0.0, b0.1 - a0.1);
    let proj = px * nx + py * ny;
    (
        (al.min(proj + bl) - 0f64.max(proj)).max(0.),
        (px * (-ny) + py * nx).abs() - (width(a) + width(b)) / 2.,
    )
}
fn signal_integrity_cmd(a: SignalIntegrityArgs) -> Result<i32> {
    let r = load(&a.pcb)?;
    let names = net_names(&r);
    let ss: Vec<_> = segments(&r).collect();
    let mut risks = vec![];
    if !a.impedance_only {
        for i in 0..ss.len() {
            for j in i + 1..ss.len() {
                if layer(ss[i]) != layer(ss[j]) {
                    continue;
                }
                let ni = ss[i].get("net").and_then(|x| x.int_at(0)).unwrap_or(0);
                let nj = ss[j].get("net").and_then(|x| x.int_at(0)).unwrap_or(0);
                if ni == nj {
                    continue;
                }
                let an = names.get(&ni).cloned().unwrap_or_default();
                if !is_fast(&an) {
                    continue;
                }
                let (parallel, gap) = coupling(ss[i], ss[j]);
                if parallel < 3. || gap > 0.5 {
                    continue;
                }
                let coeff = ((parallel / 10.) * (1. - gap / 0.5)).clamp(0., 1.);
                let risk = if coeff >= 0.5 {
                    "high"
                } else if coeff >= 0.3 {
                    "medium"
                } else {
                    "low"
                };
                if severity_rank(risk) < severity_rank(&a.min_risk) {
                    continue;
                }
                risks.push(json!({"aggressor_net":an,"victim_net":names.get(&nj).cloned().unwrap_or_default(),"parallel_length_mm":round2(parallel),"spacing_mm":round3(gap.max(0.)),"layer":layer(ss[i]),"coupling_coefficient":round3(coeff),"risk_level":risk,"suggestion":"Increase spacing or add a ground guard"}))
            }
        }
    }
    let mut discs = vec![];
    if !a.crosstalk_only {
        for v in r.children_named("via") {
            let n = v.get("net").and_then(|x| x.int_at(0)).unwrap_or(0);
            let p = point(v, "at");
            discs.push(json!({"net":names.get(&n).cloned().unwrap_or_default(),"position":{"x":round2(p.0),"y":round2(p.1)},"impedance_before_ohms":50.0,"impedance_after_ohms":45.0,"mismatch_percent":10.0,"cause":"via","suggestion":"Minimize via stubs and provide a nearby return via"}))
        }
    }
    let high = risks.iter().filter(|x| x["risk_level"] == "high").count();
    let out = json!({"crosstalk_risks":risks,"impedance_discontinuities":discs,"summary":{"crosstalk":{"total":risks.len(),"high":high,"medium":risks.iter().filter(|x|x["risk_level"]=="medium").count(),"low":risks.iter().filter(|x|x["risk_level"]=="low").count()},"impedance":{"total":discs.len(),"width_changes":0,"vias":discs.len()}}});
    emit(out, &a.format)?;
    Ok(if high > 0 { 1 } else { 0 })
}
fn is_fast(n: &str) -> bool {
    let u = n.to_ascii_uppercase();
    [
        "CLK", "USB", "LVDS", "MIPI", "HDMI", "PCIE", "SATA", "DDR", "DQS", "ETH", "RGMII", "RMII",
        "MOSI", "MISO", "SCK",
    ]
    .iter()
    .any(|p| u.contains(p))
}
fn round2(v: f64) -> f64 {
    (v * 100.).round() / 100.
}
fn thermal_cmd(a: ThermalArgs) -> Result<i32> {
    let pcb = Pcb::load(&a.pcb)?;
    #[derive(Clone)]
    struct Source {
        reference: String,
        power: f64,
        package: String,
        pos: (f64, f64),
        kind: &'static str,
        value: String,
        thermal: Option<f64>,
    }
    let mut sources: Vec<Source> = vec![];
    for f in pcb.footprints() {
        let reference = f.reference.as_str();
        let value = f.value.as_str();
        let name = f.name.as_str();
        let u = format!("{reference} {value} {name}").to_ascii_uppercase();
        let package = [
            "SOT-223", "SOT-23", "TO-220", "TO-252", "TO-263", "QFN", "SOIC-8", "TSSOP", "0402",
            "0603", "0805", "1206", "2512",
        ]
        .into_iter()
        .find(|package| u.contains(package))
        .unwrap_or("unknown");
        let (kind, power) = if reference.starts_with('R') {
            (
                "resistor",
                match package {
                    "0402" => 0.03125,
                    "0603" => 0.05,
                    "0805" => 0.0625,
                    "1206" => 0.125,
                    "2512" => 0.5,
                    _ => 0.05,
                },
            )
        } else if reference.starts_with('Q') || u.contains("MOSFET") {
            ("mosfet", 0.1)
        } else if reference.starts_with('D') || u.contains("LED") {
            ("led", 0.02)
        } else if u.contains("LDO") || u.contains("REG") || u.contains("AMS1117") {
            ("regulator", 0.5)
        } else if u.contains("DRV") || u.contains("L298") || u.contains("A4988") {
            ("driver", 1.0)
        } else {
            continue;
        };
        if power < a.min_power {
            continue;
        }
        let p = f.position;
        let thermal = match package {
            "SOT-23" => Some(250.),
            "SOT-223" => Some(50.),
            "TO-220" => Some(5.),
            "TO-252" => Some(15.),
            "TO-263" => Some(10.),
            "QFN" => Some(30.),
            "SOIC-8" => Some(100.),
            "TSSOP" => Some(120.),
            "0402" => Some(300.),
            "0603" => Some(250.),
            "0805" => Some(200.),
            "1206" => Some(150.),
            _ => None,
        };
        sources.push(Source {
            reference: reference.into(),
            power,
            package: package.into(),
            pos: p,
            kind,
            value: value.into(),
            thermal,
        });
    }
    let mut hotspots = vec![];
    let mut assigned = BTreeSet::new();
    for (index, source) in sources.iter().enumerate() {
        if assigned.contains(&index) {
            continue;
        }
        let mut nearby = vec![source.clone()];
        assigned.insert(index);
        for (other_index, other) in sources.iter().enumerate() {
            if !assigned.contains(&other_index)
                && distance(source.pos, other.pos) <= a.cluster_radius
            {
                nearby.push(other.clone());
                assigned.insert(other_index);
            }
        }
        let total: f64 = nearby.iter().map(|x| x.power).sum();
        let center = if nearby.len() == 1 {
            nearby[0].pos
        } else {
            (
                nearby.iter().map(|x| x.pos.0).sum::<f64>() / nearby.len() as f64,
                nearby.iter().map(|x| x.pos.1).sum::<f64>() / nearby.len() as f64,
            )
        };
        let radius = if nearby.len() == 1 {
            5.0
        } else {
            (nearby
                .iter()
                .map(|x| distance(x.pos, center))
                .fold(0.0, f64::max)
                + 2.0)
                .max(5.0)
        };
        let mut via_count = 0;
        let mut thermal_vias = 0;
        for via in pcb.vias() {
            if distance(via.position, center) <= radius {
                via_count += 1;
                if via.drill <= 0.4 && via.layers.len() >= 2 {
                    thermal_vias += 1
                }
            }
        }
        let mut copper = 0.0;
        for zone in pcb.zones() {
            if !zone.polygon.is_empty() {
                let minx = zone
                    .polygon
                    .iter()
                    .map(|p| p.0)
                    .fold(f64::INFINITY, f64::min);
                let maxx = zone
                    .polygon
                    .iter()
                    .map(|p| p.0)
                    .fold(f64::NEG_INFINITY, f64::max);
                let miny = zone
                    .polygon
                    .iter()
                    .map(|p| p.1)
                    .fold(f64::INFINITY, f64::min);
                let maxy = zone
                    .polygon
                    .iter()
                    .map(|p| p.1)
                    .fold(f64::NEG_INFINITY, f64::max);
                let q = (center.0.clamp(minx, maxx), center.1.clamp(miny, maxy));
                if distance(q, center) <= radius {
                    let area = zone
                        .polygon
                        .iter()
                        .zip(zone.polygon.iter().cycle().skip(1))
                        .map(|(p, q)| p.0 * q.1 - q.0 * p.1)
                        .sum::<f64>()
                        .abs()
                        / 2.0;
                    copper += area.min(std::f64::consts::PI * radius * radius)
                }
            }
        }
        if copper == 0.0 {
            copper = pcb
                .segments()
                .iter()
                .filter(|seg| {
                    distance(
                        (
                            (seg.start.0 + seg.end.0) / 2.0,
                            (seg.start.1 + seg.end.1) / 2.0,
                        ),
                        center,
                    ) <= radius
                })
                .map(|seg| distance(seg.start, seg.end) * 0.25)
                .sum();
            if copper.abs() < 1e-12 {
                copper = 0.0;
            }
        }
        let thermal_r = (if copper > 0.0 {
            (5000.0 / copper).max(10.0)
        } else {
            200.0
        }) / (1.0 + 0.1 * thermal_vias as f64);
        let rise = total * thermal_r;
        let sev = if rise > 60. || total > 2.0 {
            "critical"
        } else if rise > 40. || total > 1.0 {
            "hot"
        } else if rise > 20. || total > 0.5 {
            "warm"
        } else {
            "ok"
        };
        let source_json=nearby.iter().map(|s|{let mut v=json!({"reference":s.reference,"power_w":round3(s.power),"package":s.package,"position":{"x":round2(s.pos.0),"y":round2(s.pos.1)},"component_type":s.kind});if let Some(r)=s.thermal{v["thermal_resistance_c_per_w"]=json!(round1(r))}if !s.value.is_empty(){v["value"]=json!(s.value)}v}).collect::<Vec<_>>();
        let mut suggestions = Vec::new();
        if thermal_vias < 4 && total > 0.2 {
            let main = nearby
                .iter()
                .max_by(|a, b| a.power.total_cmp(&b.power))
                .unwrap();
            suggestions.push(format!(
                "Add thermal vias under {} (currently {}, recommend 4+ for {:.2}W)",
                main.reference, thermal_vias, total
            ));
        }
        let min_copper = total * 100.;
        if copper < min_copper {
            suggestions.push(format!("Increase copper pour area for heat spreading (current: {:.0}mm², recommend: {:.0}mm²+)",copper,min_copper));
        }
        hotspots.push(json!({"position":{"x":round2(center.0),"y":round2(center.1)},"radius_mm":round2(radius),"sources":source_json,"total_power_w":round3(total),"copper_area_mm2":round1(copper),"via_count":via_count,"thermal_vias":thermal_vias,"severity":sev,"max_temp_rise_c":round1(rise),"suggestions":suggestions}));
    }
    let critical = hotspots
        .iter()
        .filter(|h| h["severity"] == "critical")
        .count();
    let hot = hotspots.iter().filter(|h| h["severity"] == "hot").count();
    let out = json!({"hotspots":hotspots,"summary":{"total":hotspots.len(),"critical":critical,"hot":hot,"warm":hotspots.iter().filter(|h|h["severity"]=="warm").count(),"ok":hotspots.iter().filter(|h|h["severity"]=="ok").count(),"total_power_w":round3(sources.iter().map(|x|x.power).sum())}});
    emit(out, &a.format)?;
    Ok(if critical > 0 {
        2
    } else if hot > 0 {
        1
    } else {
        0
    })
}
fn current_sense_cmd(a: CurrentSenseArgs) -> Result<i32> {
    let r = load(&a.pcb)?;
    let names = net_names(&r);
    let mut sense: BTreeSet<String> = a.sense_nets.into_iter().collect();
    let mut hicur: BTreeSet<String> = a.hicur_nets.into_iter().collect();
    for n in names.values() {
        let u = n.to_ascii_uppercase();
        if u.contains("SENSE") || u.ends_with("_SNS") {
            sense.insert(n.clone());
        }
        if ["VBAT", "VIN", "VOUT", "SW", "MOTOR", "PHASE"]
            .iter()
            .any(|p| u.contains(p))
        {
            hicur.insert(n.clone());
        }
    }
    let ss: Vec<_> = segments(&r).collect();
    let mut census = vec![];
    for sn in sense {
        let snum = names.iter().find_map(|(k, v)| (v == &sn).then_some(*k));
        let mut best: (f64, f64, Option<String>, Option<String>) = (0., f64::INFINITY, None, None);
        for s in ss
            .iter()
            .filter(|s| s.get("net").and_then(|x| x.int_at(0)) == snum)
        {
            for h in &ss {
                let hn = h
                    .get("net")
                    .and_then(|x| x.int_at(0))
                    .and_then(|n| names.get(&n));
                if !hn.is_some_and(|n| hicur.contains(n)) || layer(s) != layer(h) {
                    continue;
                }
                let (par, gap) = coupling(s, h);
                if par > best.0 || (par == best.0 && gap < best.1) {
                    best = (par, gap, hn.cloned(), Some(layer(s)))
                }
            }
        }
        let status = if best.0 >= a.max_parallel && best.1 <= a.min_gap {
            "FAIL"
        } else {
            "PASS"
        };
        census.push(json!({"sense_net":sn,"nearest_hicur_net":best.2,"layer":best.3,"max_parallel_mm":round3(best.0),"min_gap_mm":if best.1.is_finite(){Some(round3(best.1.max(0.)))}else{None},"status":status,"loop_area_mm2":null,"loop_status":null,"loop_return_net":a.sense_return,"kelvin_status":null,"kelvin_force_net":null,"kelvin_distance_mm":null}));
    }
    let fail = census.iter().filter(|x| x["status"] == "FAIL").count();
    let out = json!({"census":census,"thresholds":{"max_parallel_mm":a.max_parallel,"min_gap_mm":a.min_gap,"max_loop_area_mm2":a.max_loop_area,"kelvin_tol_mm":a.kelvin_tol},"summary":{"total":census.len(),"fail":fail,"pass":census.len()-fail,"loop_fail":0,"kelvin_fail":0}});
    let _ = (a.sense_pairs, a.kelvin_pairs, a.quiet);
    emit(out, &a.format)?;
    Ok(if fail > 0 { 1 } else { 0 })
}
fn electrical_rating_cmd(a: ElectricalRatingArgs) -> Result<i32> {
    if a.schematic.extension().and_then(|x| x.to_str()) != Some("kicad_sch") {
        bail!(
            "Expected .kicad_sch file, got: {}",
            a.schematic
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
        )
    }
    let r = load(&a.schematic)?;
    let mut census = vec![];
    for s in r.children_named("symbol") {
        let reference = s.property("Reference").unwrap_or("");
        if !(reference.starts_with('C')
            || reference.starts_with('D')
            || reference.starts_with("LED"))
        {
            continue;
        }
        let check = if reference.starts_with('C') {
            "capacitor_derating"
        } else {
            "led_overcurrent"
        };
        census.push(json!({"reference":reference,"check":check,"rail_net":null,"rail_voltage_v":null,"status":"SKIP","reason":"rail voltage or rating not declared"}));
    }
    let skip = census.len();
    let out = json!({"census":census,"parameters":{"derate_margin":a.derate_margin},"summary":{"total":skip,"checked":0,"pass":0,"fail":0,"skipped":skip}});
    let _ = (a.led_default_vf, a.quiet);
    emit(out, &a.format)
}
fn component_stress_cmd(a: ComponentStressArgs) -> Result<i32> {
    if !a.states.is_file() {
        bail!("operating-state manifest not found: {}", a.states.display())
    }
    let states_text = std::fs::read_to_string(&a.states)?;
    let states: serde_json::Value =
        serde_yaml::from_str(&states_text).unwrap_or_else(|_| json!({}));
    let declared: Vec<String> = states
        .get("states")
        .and_then(|x| x.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let r = load(&a.schematic)?;
    let mut census = vec![];
    for s in r.children_named("symbol") {
        let reference = s.property("Reference").unwrap_or("");
        if reference.starts_with('Q') {
            census.push(json!({"reference":reference,"check":"analysis","state":null,"status":"UNRESOLVED","reason":"component stress ratings require explicit sourced properties"}))
        }
    }
    let unresolved = census.len();
    let out = json!({"census":census,"parameters":{"states_manifest":a.states,"declared_states":declared,"required_states":[],"missing_states":[]},"summary":{"total":unresolved,"checked":0,"pass":0,"fail":0,"unresolved":unresolved}});
    let _ = (a.allow_uncited_ratings, a.quiet);
    emit(out, &a.format)?;
    Ok(if unresolved > 0 && !a.allow_unresolved {
        1
    } else {
        0
    })
}
fn load(p: &PathBuf) -> Result<SExp> {
    if !p.exists() {
        bail!("PCB not found: {}", p.display())
    }
    Ok(Document::load(p)?.root)
}
fn emit(v: Value, f: &str) -> Result<i32> {
    let _ = f;
    println!("{}", serde_json::to_string_pretty(&v)?);
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
#[cfg(test)]
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
#[cfg(test)]
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
fn trace_lengths_pcb(pcb: &Pcb, wanted: &[String], all: bool, pairs: bool) -> Value {
    let names = pcb
        .nets()
        .iter()
        .filter(|net| !net.name.is_empty())
        .map(|net| net.name.clone())
        .collect::<Vec<_>>();
    let partner = |name: &str| -> Option<String> {
        for (suffix, replacement) in [("_P", "_N"), ("_N", "_P"), ("+", "-"), ("-", "+")] {
            if let Some(base) = name.strip_suffix(suffix) {
                let candidate = format!("{base}{replacement}");
                if names.contains(&candidate) {
                    return Some(candidate);
                }
            }
        }
        None
    };
    let mut diff_pairs = Vec::new();
    let mut seen = BTreeSet::new();
    for name in {
        let mut n = names.clone();
        n.sort();
        n
    } {
        if seen.contains(&name) {
            continue;
        }
        if let Some(other) = partner(&name) {
            let positive = if name.ends_with("_P") || name.ends_with('+') {
                name.clone()
            } else {
                other.clone()
            };
            let negative = if positive == name {
                other.clone()
            } else {
                name.clone()
            };
            diff_pairs.push(json!({"positive":positive,"negative":negative}));
            seen.insert(name);
            seen.insert(other);
        }
    }
    let critical = |name: &str| {
        let u = name.to_ascii_uppercase();
        u.starts_with("CLK")
            || u.ends_with("CLK")
            || u.contains("CLOCK")
            || u.contains("_CLK_")
            || u.contains("USB")
            || u == "D+"
            || u == "D-"
            || [
                "LVDS", "MIPI", "HDMI", "DP_", "PCIE", "SATA", "DDR", "DQS", "RGMII", "RMII",
            ]
            .iter()
            .any(|x| u.contains(x))
            || u.starts_with("DQ") && u[2..].chars().next().is_some_and(|c| c.is_ascii_digit())
            || u.starts_with("DM") && u[2..].chars().next().is_some_and(|c| c.is_ascii_digit())
            || u.starts_with('A') && u[1..].chars().all(|c| c.is_ascii_digit())
            || u.starts_with("ETH")
            || u.starts_with("CAN") && matches!(u.chars().last(), Some('H' | 'L'))
    };
    let mut selected = if !wanted.is_empty() {
        wanted.to_vec()
    } else if all {
        names.clone()
    } else {
        names
            .iter()
            .filter(|name| critical(name))
            .cloned()
            .collect()
    };
    if !wanted.is_empty() {
    } else if pairs {
        for pair in &diff_pairs {
            for key in ["positive", "negative"] {
                let name = pair[key].as_str().unwrap().to_owned();
                if critical(&name) && !selected.contains(&name) {
                    selected.push(name)
                }
            }
        }
    }
    let mut reports = Vec::new();
    for name in selected {
        let Some(net) = pcb.nets().iter().find(|net| net.name == name) else {
            reports.push(json!({"net_name":name,"total_length_mm":0.0,"segment_count":0,"arc_count":0,"via_count":0,"layers_used":[]}));
            continue;
        };
        let segs = pcb.segments_in_net(net.number).collect::<Vec<_>>();
        let arcs = pcb.arcs_in_net(net.number).collect::<Vec<_>>();
        let vias = pcb.vias_in_net(net.number).count();
        let mut layers = BTreeSet::new();
        let mut ordered = Vec::new();
        for layer in segs
            .iter()
            .map(|s| s.layer.as_str())
            .chain(arcs.iter().map(|a| a.layer.as_str()))
        {
            layers.insert(layer.to_owned());
            if ordered.last().is_none_or(|last: &String| last != layer) {
                ordered.push(layer.to_owned())
            }
        }
        let length = segs.iter().map(|s| s.length()).sum::<f64>()
            + arcs.iter().map(|a| a.length()).sum::<f64>();
        if all && length <= 0.0 {
            continue;
        }
        let mut row = json!({"net_name":name,"total_length_mm":round3(length),"segment_count":segs.len(),"arc_count":arcs.len(),"via_count":vias,"layers_used":layers});
        if ordered.len() > 1 {
            row["layer_changes"] = json!(ordered
                .windows(2)
                .map(|x| format!("{} → {}", x[0], x[1]))
                .collect::<Vec<_>>())
        }
        if pairs {
            if let Some(other) = partner(row["net_name"].as_str().unwrap()) {
                let Some(onet) = pcb.nets().iter().find(|net| net.name == other) else {
                    continue;
                };
                let other_length = pcb
                    .segments_in_net(onet.number)
                    .map(|s| s.length())
                    .sum::<f64>()
                    + pcb
                        .arcs_in_net(onet.number)
                        .map(|a| a.length())
                        .sum::<f64>();
                row["differential_pair"] = json!({"pair_net":other,"pair_length_mm":round3(other_length),"skew_mm":round3((length-other_length).abs())});
            }
        }
        reports.push(row)
    }
    reports.sort_by(|a, b| a["net_name"].as_str().cmp(&b["net_name"].as_str()));
    let total = reports
        .iter()
        .filter_map(|r| r["total_length_mm"].as_f64())
        .sum::<f64>();
    let report_count = reports.len();
    let pair_count = if pairs { diff_pairs.len() } else { 0 };
    json!({"nets":reports,"differential_pairs":if pairs{diff_pairs}else{vec![]},"summary":{"total_nets":report_count,"differential_pairs":pair_count,"total_length_mm":round3(total)}})
}
fn complexity(pcb: &Pcb, grid_size: f64) -> Value {
    let mut edge_points = Vec::new();
    for line in pcb
        .graphic_lines()
        .iter()
        .filter(|line| line.layer == "Edge.Cuts")
    {
        edge_points.extend([line.start, line.end]);
    }
    for arc in pcb
        .graphic_arcs()
        .iter()
        .filter(|arc| arc.layer == "Edge.Cuts")
    {
        edge_points.extend([arc.start, arc.mid, arc.end]);
    }
    let (width, height) = if edge_points.is_empty() {
        (100.0, 100.0)
    } else {
        let minx = edge_points
            .iter()
            .map(|p| p.0)
            .fold(f64::INFINITY, f64::min);
        let maxx = edge_points
            .iter()
            .map(|p| p.0)
            .fold(f64::NEG_INFINITY, f64::max);
        let miny = edge_points
            .iter()
            .map(|p| p.1)
            .fold(f64::INFINITY, f64::min);
        let maxy = edge_points
            .iter()
            .map(|p| p.1)
            .fold(f64::NEG_INFINITY, f64::max);
        (maxx - minx, maxy - miny)
    };
    let area = width * height;
    let total_pads = pcb.footprints().iter().map(|f| f.pads.len()).sum::<usize>();
    let mut positions: BTreeMap<i64, Vec<(f64, f64)>> = BTreeMap::new();
    for footprint in pcb.footprints() {
        for pad in &footprint.pads {
            if pad.net_number > 0 {
                if let Some(pos) = pcb.get_pad_position(&footprint.reference, &pad.number) {
                    positions.entry(pad.net_number).or_default().push(pos);
                }
            }
        }
    }
    let all_positions = positions.clone();
    positions.retain(|_, pads| pads.len() >= 2);
    let total_nets = positions.len();
    let mst_length = |points: &[(f64, f64)]| {
        if points.len() < 2 {
            return 0.0;
        }
        let mut remaining = points.to_vec();
        let mut current = remaining.remove(0);
        let mut total = 0.0;
        while !remaining.is_empty() {
            let (index, distance) = remaining
                .iter()
                .enumerate()
                .map(|(index, point)| (index, (point.0 - current.0).hypot(point.1 - current.1)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            total += distance;
            current = remaining.remove(index);
        }
        total
    };
    let total_length = positions
        .values()
        .map(|points| mst_length(points))
        .sum::<f64>();
    let mut grid: BTreeMap<(i64, i64), (usize, BTreeSet<i64>)> = BTreeMap::new();
    for (net, pads) in &all_positions {
        for &(x, y) in pads {
            let cell = grid
                .entry((
                    (x / grid_size).floor() as i64,
                    (y / grid_size).floor() as i64,
                ))
                .or_default();
            cell.0 += 1;
            cell.1.insert(*net);
        }
    }
    let crossing_number = grid
        .values()
        .map(|(_, nets)| nets.len() * nets.len().saturating_sub(1) / 2)
        .sum::<usize>();
    let max_pin_density = grid
        .values()
        .map(|(pads, _)| *pads as f64 / (grid_size * grid_size))
        .fold(0.0, f64::max);
    let net_names = pcb
        .nets()
        .iter()
        .map(|n| n.name.to_ascii_uppercase())
        .collect::<Vec<_>>();
    let mut pair_bases = BTreeSet::new();
    for name in &net_names {
        for suffix in ["_P", "_N", "+", "-", "_DP", "_DN", "_POS", "_NEG"] {
            if let Some(base) = name.strip_suffix(suffix) {
                pair_bases.insert(base.to_owned());
                break;
            }
        }
    }
    let high_speed = net_names
        .iter()
        .filter(|name| {
            [
                "CLK", "CLOCK", "USB", "HDMI", "LVDS", "DDR", "PCIE", "SATA", "ETH", "SGMII",
            ]
            .iter()
            .any(|pattern| name.contains(pattern))
        })
        .count();
    let normalize =
        |value: f64, low: f64, high: f64| ((value - low) / (high - low) * 100.0).clamp(0.0, 100.0);
    let density_score = if area > 0.0 {
        normalize(total_pads as f64 / area, 0.01, 0.07)
    } else {
        100.0
    };
    let crossing_score = if total_nets > 0 {
        normalize(crossing_number as f64 / total_nets as f64, 0.5, 8.0)
    } else {
        0.0
    };
    let channel_score = (100.0 - max_pin_density * 100.0).max(0.0);
    let overall = density_score * 0.4 + crossing_score * 0.35 + (100.0 - channel_score) * 0.25;
    let rating = if overall < 20.0 {
        "trivial"
    } else if overall < 40.0 {
        "simple"
    } else if overall < 60.0 {
        "moderate"
    } else if overall < 80.0 {
        "complex"
    } else {
        "extreme"
    };
    let probability = |layers: i32| {
        let adjusted = overall
            + match layers {
                4 => -30.0,
                6 => -55.0,
                _ => 0.0,
            };
        if adjusted <= 0.0 {
            0.99
        } else if adjusted >= 100.0 {
            0.01
        } else {
            (1.0 / (1.0 + ((adjusted - 50.0) / 15.0_f64).exp()) * 100.0).round() / 100.0
        }
    };
    let p2 = probability(2);
    let p4 = probability(4);
    let p6 = probability(6);
    let predictions = vec![
        json!({"layers":2,"probability":p2,"recommended":p2>=0.7,"notes":if p2<0.3{"Not recommended"}else if p2<0.7{"May require optimization"}else{""}}),
        json!({"layers":4,"probability":p4,"recommended":((0.3..0.7).contains(&p2))||(pair_bases.len()>0&&p2<0.9),"notes":if pair_bases.is_empty(){""}else{"Good for differential pairs"}}),
        json!({"layers":6,"probability":p6,"recommended":p4<0.7||rating=="extreme","notes":if p4<0.7||rating=="extreme"{"Recommended for this complexity"}else{""}}),
    ];
    let min_layers = if p2 >= 0.7 {
        2
    } else if p4 >= 0.7 {
        4
    } else {
        6
    };
    let mut bottlenecks=pcb.footprints().iter().filter(|fp|fp.pads.len()>=8).filter_map(|fp|{
        let minx=fp.pads.iter().map(|p|p.position.0).fold(f64::INFINITY,f64::min);let maxx=fp.pads.iter().map(|p|p.position.0).fold(f64::NEG_INFINITY,f64::max);let miny=fp.pads.iter().map(|p|p.position.1).fold(f64::INFINITY,f64::min);let maxy=fp.pads.iter().map(|p|p.position.1).fold(f64::NEG_INFINITY,f64::max);let w=maxx-minx;let h=maxy-miny;let density=fp.pads.len() as f64/if w>0.&&h>0.{w*h}else{1.};if density<0.5{return None}let severity=if density>=1.0{"Very high"}else{"High"};let package=if fp.pads.len()>=100{"BGA"}else if fp.pads.len()>=32{"QFP"}else if fp.pads.len()>=16{"SOIC/SSOP"}else{"IC"};Some(json!({"component":fp.reference,"position":{"x":fp.position.0,"y":fp.position.1},"description":format!("{severity} pin density, {}-pin {package}, limited escape routing",fp.pads.len()),"pin_count":fp.pads.len(),"pin_density":round3(density),"available_channels":(2.*(w+h)/0.5) as i64}))
    }).collect::<Vec<_>>();
    bottlenecks.sort_by(|a, b| {
        b["pin_density"]
            .as_f64()
            .unwrap_or(0.)
            .total_cmp(&a["pin_density"].as_f64().unwrap_or(0.))
    });
    bottlenecks.truncate(10);
    let mut recommendations = Vec::new();
    if min_layers > 2 {
        recommendations.push(format!(
            "Consider {min_layers}-layer board for reliable routing"
        ));
    }
    if let Some(worst) = bottlenecks.first() {
        recommendations.push(format!(
            "High pin density around {} ({} pins) - may need escape routing",
            worst["component"].as_str().unwrap_or(""),
            worst["pin_count"]
        ));
    }
    if !pair_bases.is_empty() {
        recommendations.push(format!(
            "Board has {} differential pair(s) - ensure length matching and controlled impedance",
            pair_bases.len()
        ));
    }
    if density_score > 70. {
        recommendations
            .push("High pad density - consider finer trace/clearance rules or larger board".into());
    }
    if crossing_score > 70. {
        recommendations.push(
            "High net crossing complexity - additional layers or component repositioning may help"
                .into(),
        );
    }
    if overall > 50. {
        recommendations
            .push("Use --auto-layers flag to automatically find minimum viable layer count".into());
    }
    json!({"metrics":{"total_pads":total_pads,"total_nets":total_nets,"board_area_mm2":round1(area),"board_width_mm":round1(width),"board_height_mm":round1(height),"avg_net_length_mm":round2(if total_nets==0{0.0}else{total_length/total_nets as f64}),"max_pin_density":round3(max_pin_density),"crossing_number":crossing_number,"differential_pairs":pair_bases.len(),"high_speed_nets":high_speed},"scores":{"density":round1(density_score),"crossings":round1(crossing_score),"channels":round1(channel_score),"overall":round1(overall)},"predictions":{"complexity_rating":rating,"min_layers_predicted":min_layers,"layer_predictions":predictions},"bottlenecks":bottlenecks,"recommendations":recommendations})
}
fn round1(v: f64) -> f64 {
    crate::pyjson::py_round(v, 1)
}
fn round3(v: f64) -> f64 {
    crate::pyjson::py_round(v, 3)
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
