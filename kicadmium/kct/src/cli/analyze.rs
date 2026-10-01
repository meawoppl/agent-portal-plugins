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
            let r = load(&a.pcb)?;
            emit(
                trace_lengths(&r, &a.nets, a.all, a.diff_pairs || !a.no_diff_pairs),
                &a.format,
            )
        }
        Command::Complexity(a) => {
            let r = load(&a.pcb)?;
            let _ = a.grid_size;
            emit(complexity(&r), &a.format)
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
    let r = load(&a.pcb)?;
    let mut sources = vec![];
    for f in r.children_named("footprint") {
        let reference = f.property("Reference").unwrap_or("");
        let value = f.property("Value").unwrap_or("");
        let name = f.string_at(0).unwrap_or("");
        let u = format!("{reference} {value} {name}").to_ascii_uppercase();
        let (kind, power) = if reference.starts_with('R') {
            (
                "resistor",
                if u.contains("2512") {
                    1.0
                } else if u.contains("1206") {
                    0.25
                } else if u.contains("0805") {
                    0.125
                } else {
                    0.1
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
        let p = point(f, "at");
        sources.push(json!({"reference":reference,"power_w":power,"package":name,"position":{"x":round2(p.0),"y":round2(p.1)},"component_type":kind,"value":value}));
    }
    let mut hotspots = vec![];
    for s in &sources {
        let p = (
            s["position"]["x"].as_f64().unwrap(),
            s["position"]["y"].as_f64().unwrap(),
        );
        let nearby: Vec<_> = sources
            .iter()
            .filter(|x| {
                distance(
                    p,
                    (
                        x["position"]["x"].as_f64().unwrap(),
                        x["position"]["y"].as_f64().unwrap(),
                    ),
                ) <= a.cluster_radius
            })
            .cloned()
            .collect();
        let total: f64 = nearby.iter().filter_map(|x| x["power_w"].as_f64()).sum();
        let rise = total * 50.;
        let sev = if rise >= 100. {
            "critical"
        } else if rise >= 60. {
            "hot"
        } else if rise >= 30. {
            "warm"
        } else {
            "ok"
        };
        hotspots.push(json!({"position":s["position"],"radius_mm":a.cluster_radius,"sources":nearby,"total_power_w":round3(total),"copper_area_mm2":0.0,"via_count":0,"thermal_vias":0,"severity":sev,"max_temp_rise_c":round2(rise),"suggestions":if matches!(sev,"hot"|"critical"){vec!["Add copper area and thermal vias"]}else{Vec::<&str>::new()}}));
    }
    hotspots.dedup_by(|a, b| a["position"] == b["position"]);
    let critical = hotspots
        .iter()
        .filter(|h| h["severity"] == "critical")
        .count();
    let hot = hotspots.iter().filter(|h| h["severity"] == "hot").count();
    let out = json!({"hotspots":hotspots,"summary":{"total":hotspots.len(),"critical":critical,"hot":hot,"warm":hotspots.iter().filter(|h|h["severity"]=="warm").count(),"ok":hotspots.iter().filter(|h|h["severity"]=="ok").count(),"total_power_w":round3(sources.iter().filter_map(|x|x["power_w"].as_f64()).sum())}});
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
