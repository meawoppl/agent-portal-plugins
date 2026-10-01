//! Native routing-support benchmarks and host calibration.

use std::ffi::OsString;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};
use serde::{Deserialize, Serialize};

use super::{parse_args, Globals};
use crate::schema::pcb::Pcb;

#[derive(Clone, Serialize, Deserialize)]
struct BenchmarkResult {
    case_name: String,
    strategy: String,
    board: Option<PathBuf>,
    nets_total: usize,
    nets_routed: usize,
    total_vias: usize,
    total_length_mm: f64,
    routing_time_sec: Option<f64>,
    operations: usize,
    throughput_ops_sec: f64,
    measurement_mode: String,
}

#[derive(Parser)]
struct BenchmarkArgs {
    #[command(subcommand)]
    command: BenchmarkCommand,
}
#[derive(Subcommand)]
enum BenchmarkCommand {
    Run(BenchmarkRun),
    List {
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
    Report {
        input: PathBuf,
        #[arg(long,default_value="text",value_parser=["text","json","markdown"])]
        format: String,
    },
    Compare {
        #[arg(long)]
        baseline: PathBuf,
        #[arg(long)]
        board: Option<PathBuf>,
        #[arg(long = "fail-on-warning")]
        fail_on_warning: bool,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
}
#[derive(ClapArgs)]
struct BenchmarkRun {
    #[arg(long)]
    board: Option<PathBuf>,
    #[arg(long, value_delimiter = ',')]
    cases: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    strategies: Vec<String>,
    #[arg(long,value_parser=["easy","medium","hard"])]
    difficulty: Option<String>,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    save: bool,
    #[arg(long, default_value_t = 20)]
    iterations: usize,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
}

const CASES: &[(&str, &str)] = &[
    ("pcb-parse", "easy"),
    ("ratsnest", "medium"),
    ("route-geometry", "hard"),
];

fn run_case(
    case: &str,
    strategy: &str,
    board: Option<&Path>,
    iterations: usize,
) -> Result<BenchmarkResult> {
    let passes = match strategy {
        "basic" => 1,
        "negotiated" => 2,
        "monte_carlo" => 4,
        _ => bail!("unknown strategy {strategy:?}; expected basic, negotiated, or monte_carlo"),
    };
    let count = iterations.max(1) * passes;
    let started = Instant::now();
    let mut metrics = (0, 0, 0, 0.0);
    match (case, board) {
        ("pcb-parse", Some(path)) => {
            for _ in 0..count {
                let pcb = Pcb::load(path)?;
                metrics = (
                    pcb.nets().len(),
                    pcb.routing_status().nets_with_traces.len(),
                    pcb.vias().len(),
                    pcb.routing_status().trace_length_mm,
                );
                black_box(&pcb);
            }
        }
        ("ratsnest", Some(path)) => {
            let pcb = Pcb::load(path)?;
            for _ in 0..count {
                let r = pcb.get_ratsnest();
                metrics = (
                    pcb.nets().len(),
                    pcb.routing_status().nets_with_traces.len(),
                    pcb.vias().len(),
                    pcb.routing_status().trace_length_mm,
                );
                black_box(r);
            }
        }
        ("route-geometry", Some(path)) => {
            let text = std::fs::read_to_string(path)?;
            let pcb = Pcb::load(path)?;
            for _ in 0..count {
                let census = crate::router::quantize::segment_angle_census_text(&text, 0.01);
                for s in pcb.segments() {
                    black_box(crate::router::quantize::dogleg(
                        s.start.0, s.start.1, s.end.0, s.end.1, false,
                    ));
                }
                black_box(census);
            }
            metrics = (
                pcb.nets().len(),
                pcb.routing_status().nets_with_traces.len(),
                pcb.vias().len(),
                pcb.routing_status().trace_length_mm,
            );
        }
        ("route-geometry", None) => {
            for i in 0..count * 1000 {
                black_box(crate::router::quantize::dogleg(
                    0.0,
                    0.0,
                    i as f64 % 100.0,
                    (i * 7) as f64 % 100.0,
                    i % 2 == 0,
                ));
            }
        }
        (_, None) => {
            for i in 0..count * 1000 {
                black_box(crate::core::geometry::segment_to_segment_distance(
                    0., 0., i as f64, 1., 1., 2., 3., 4.,
                ));
            }
        }
        _ => bail!("unknown benchmark case {case:?}"),
    }
    let elapsed = started.elapsed().as_secs_f64().max(f64::EPSILON);
    Ok(BenchmarkResult {
        case_name: case.into(),
        strategy: strategy.into(),
        board: board.map(Path::to_path_buf),
        nets_total: metrics.0,
        nets_routed: metrics.1,
        total_vias: metrics.2,
        total_length_mm: metrics.3,
        routing_time_sec: None,
        operations: count,
        throughput_ops_sec: count as f64 / elapsed,
        measurement_mode: "router-support workload (not an autoroute timing claim)".into(),
    })
}

fn run_suite(
    board: Option<&Path>,
    cases: &[String],
    strategies: &[String],
    difficulty: Option<&str>,
    iterations: usize,
) -> Result<Vec<BenchmarkResult>> {
    let mut selected: Vec<_> = if cases.is_empty() {
        CASES
            .iter()
            .filter(|(_, d)| difficulty.is_none_or(|want| want == *d))
            .map(|(n, _)| n.to_string())
            .collect()
    } else {
        cases.to_vec()
    };
    selected.sort();
    selected.dedup();
    let mut strategies = if strategies.is_empty() {
        vec!["basic".into(), "negotiated".into(), "monte_carlo".into()]
    } else {
        strategies.to_vec()
    };
    strategies.sort();
    strategies.dedup();
    let mut out = Vec::new();
    for case in selected {
        if !CASES.iter().any(|c| c.0 == case) {
            bail!("unknown benchmark case {case:?}")
        }
        for strategy in &strategies {
            out.push(run_case(&case, strategy, board, iterations)?);
        }
    }
    Ok(out)
}

pub fn benchmark(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<BenchmarkArgs>("benchmark", args);
    match args.command {
        BenchmarkCommand::List { format } => {
            let rows:Vec<_>=CASES.iter().map(|(n,d)|serde_json::json!({"name":n,"difficulty":d,"is_synthetic":true,"description":"native router-support workload; pass --board for real-board metrics"})).collect();
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&rows)?)
            } else {
                println!("Available Benchmark Cases\n=========================\n");
                for (n, d) in CASES {
                    println!("{n:<20} {d}")
                }
            }
        }
        BenchmarkCommand::Run(a) => {
            if (a.save || a.output.is_some()) && a.output.is_none() {
                bail!("saving benchmark results requires --output")
            };
            let results = run_suite(
                a.board.as_deref(),
                &a.cases,
                &a.strategies,
                a.difficulty.as_deref(),
                a.iterations,
            )?;
            if let Some(path) = &a.output {
                std::fs::write(path, serde_json::to_string_pretty(&results)? + "\n")?
            }
            if a.format == "json" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"results":results,"saved_to":a.output})
                    )?
                )
            } else {
                println!("Routing Support Benchmark Report\n===============================");
                for r in results {
                    println!(
                        "{:<16} {:<12} {:>10.1} ops/s  nets {}/{} vias {} length {:.1}mm",
                        r.case_name,
                        r.strategy,
                        r.throughput_ops_sec,
                        r.nets_routed,
                        r.nets_total,
                        r.total_vias,
                        r.total_length_mm
                    )
                }
                println!("Timing covers support workloads only; it is not published autorouter wall-clock timing.")
            }
        }
        BenchmarkCommand::Report { input, format } => {
            let results: Vec<BenchmarkResult> = serde_json::from_slice(&std::fs::read(&input)?)?;
            if format == "json" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"input":input,"results":results})
                    )?
                )
            } else if format == "markdown" {
                println!("# Routing Support Benchmark Report\n\n| Case | Strategy | Throughput | Nets routed | Vias | Length |\n|---|---|---:|---:|---:|---:|");
                for r in results {
                    println!(
                        "| {} | {} | {:.1} ops/s | {}/{} | {} | {:.1} mm |",
                        r.case_name,
                        r.strategy,
                        r.throughput_ops_sec,
                        r.nets_routed,
                        r.nets_total,
                        r.total_vias,
                        r.total_length_mm
                    )
                }
            } else {
                for r in results {
                    println!(
                        "{} / {}: {:.1} ops/s",
                        r.case_name, r.strategy, r.throughput_ops_sec
                    )
                }
            }
        }
        BenchmarkCommand::Compare {
            baseline,
            board,
            fail_on_warning,
            format,
        } => {
            let old: Vec<BenchmarkResult> = serde_json::from_slice(&std::fs::read(&baseline)?)?;
            let cases: Vec<_> = old.iter().map(|r| r.case_name.clone()).collect();
            let strategies: Vec<_> = old.iter().map(|r| r.strategy.clone()).collect();
            let current = run_suite(board.as_deref(), &cases, &strategies, None, 20)?;
            let mut changes = Vec::new();
            for now in &current {
                if let Some(was) = old
                    .iter()
                    .find(|r| r.case_name == now.case_name && r.strategy == now.strategy)
                {
                    let pct = (now.throughput_ops_sec / was.throughput_ops_sec - 1.0) * 100.0;
                    let severity = if pct < -25.0 {
                        "error"
                    } else if pct < -10.0 {
                        "warning"
                    } else {
                        "ok"
                    };
                    changes.push(serde_json::json!({"case_name":now.case_name,"strategy":now.strategy,"metric":"throughput_ops_sec","baseline_value":was.throughput_ops_sec,"current_value":now.throughput_ops_sec,"change_percent":pct,"severity":severity}));
                }
            }
            let errors = changes.iter().filter(|c| c["severity"] == "error").count();
            let warnings = changes
                .iter()
                .filter(|c| c["severity"] == "warning")
                .count();
            let ok = errors == 0 && (!fail_on_warning || warnings == 0);
            if format == "json" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"baseline":baseline,"regressions":changes,"errors":errors,"warnings":warnings,"ok":ok})
                    )?
                )
            } else {
                for c in changes {
                    println!(
                        "{} / {}: {:+.1}% {}",
                        c["case_name"].as_str().unwrap(),
                        c["strategy"].as_str().unwrap(),
                        c["change_percent"].as_f64().unwrap(),
                        c["severity"].as_str().unwrap()
                    )
                }
            }
            return Ok(if ok { 0 } else { 1 });
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct BenchArgs {
    #[command(subcommand)]
    command: BenchCommand,
}
#[derive(Subcommand)]
enum BenchCommand {
    External {
        #[arg(long = "board", required = true)]
        boards: Vec<PathBuf>,
        #[arg(long)]
        tuned: bool,
        #[arg(long = "output-dir")]
        output_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long = "mfr", alias = "manufacturer", default_value = "jlcpcb")]
        manufacturer: String,
        #[arg(short, long, default_value_t = 4)]
        layers: u8,
        #[arg(long = "skip-kicad-cli-drc")]
        skip_drc: bool,
        #[arg(long,default_value="text",value_parser=["text","json"])]
        format: String,
    },
}
pub fn bench(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<BenchArgs>("bench", args);
    match args.command {
        BenchCommand::External {
            boards,
            tuned,
            output_dir,
            seed,
            manufacturer,
            layers,
            skip_drc,
            format,
        } => {
            let protocol = if tuned { "tuned" } else { "zero-touch" };
            let mut reports = Vec::new();
            for path in boards {
                if !path.exists() {
                    bail!("external board path not found: {}", path.display())
                }
                let pcb = Pcb::load(&path)?;
                let status = pcb.routing_status();
                let pads = pcb
                    .footprints()
                    .iter()
                    .flat_map(|f| &f.pads)
                    .filter(|p| p.net_number != 0)
                    .count();
                let routed = status.nets_with_traces.len();
                let report = serde_json::json!({"board":path,"protocol":protocol,"seed":seed,"manufacturer":manufacturer,"layers":layers,"measurement_mode":"existing-board analysis; routing was not rerun","completion":{"netted_pads":pads,"nets_with_copper":routed},"copper":{"via_count":pcb.vias().len(),"wirelength_mm":status.trace_length_mm},"timing":{"valid":false,"reason":"support command does not claim autorouter timing"},"kicad_cli_drc":{"ran":false,"skipped":skip_drc}});
                if let Some(dir) = &output_dir {
                    std::fs::create_dir_all(dir)?;
                    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("board");
                    std::fs::write(
                        dir.join(format!("{name}.{protocol}.json")),
                        serde_json::to_string_pretty(&report)? + "\n",
                    )?
                }
                reports.push(report)
            }
            let document = serde_json::json!({"reports":reports,"errors":{},"output_dir":output_dir,"success":true});
            if format == "json" {
                println!("{}", serde_json::to_string_pretty(&document)?)
            } else {
                println!(
                    "External board benchmark ({protocol}): {} board(s)",
                    reports.len()
                );
                for r in reports {
                    println!(
                        "{}: {} vias, {:.1}mm existing copper",
                        r["board"].as_str().unwrap(),
                        r["copper"]["via_count"],
                        r["copper"]["wirelength_mm"].as_f64().unwrap()
                    )
                }
                println!(
                    "No autorouter timing claimed; these are reproducible existing-board metrics."
                )
            }
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct CalibrateArgs {
    #[arg(long)]
    show: bool,
    #[arg(long = "show-gpu")]
    show_gpu: bool,
    #[arg(long)]
    gpu: bool,
    #[arg(long = "all")]
    all_backends: bool,
    #[arg(long)]
    benchmark: bool,
    #[arg(long)]
    quick: bool,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long,default_value="text",value_parser=["text","json"])]
    format: String,
    #[arg(long)]
    json: bool,
}
#[derive(Serialize, Deserialize)]
struct Calibration {
    cpu_cores: usize,
    available_memory_gb: f64,
    monte_carlo_trials: usize,
    parallel_workers: usize,
    grid_memory_limit_mb: usize,
    negotiated_iterations: usize,
    partition_rows: usize,
    partition_cols: usize,
    calibrated: bool,
    benchmarks: Vec<CalibrationRun>,
    gpu_available: bool,
}
#[derive(Serialize, Deserialize)]
struct CalibrationRun {
    worker_count: usize,
    trial_count: usize,
    duration_ms: f64,
    throughput: f64,
}
fn memory_gb() -> f64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemAvailable:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<f64>().ok())
        })
        .map(|kb| kb / 1024.0 / 1024.0)
        .unwrap_or(0.0)
}
fn calibration(quick: bool) -> Calibration {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    let candidates = if quick {
        vec![(cores / 2).max(1), cores]
    } else {
        vec![1, 2.min(cores), 4.min(cores), (cores / 2).max(1), cores]
    };
    let mut candidates: Vec<_> = candidates.into_iter().collect();
    candidates.sort_unstable();
    candidates.dedup();
    let trials = if quick { 20 } else { 60 };
    let mut runs = Vec::new();
    for workers in candidates {
        let start = Instant::now();
        std::thread::scope(|scope| {
            for worker in 0..workers {
                scope.spawn(move || {
                    let mut sum = 0.0;
                    for i in 0..trials * 2000 {
                        sum += ((i + worker) as f64).sqrt().sin().abs()
                    }
                    black_box(sum);
                });
            }
        });
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        runs.push(CalibrationRun {
            worker_count: workers,
            trial_count: trials,
            duration_ms: ms,
            throughput: (trials * workers) as f64 / (ms / 1000.0).max(f64::EPSILON),
        });
    }
    let best = runs
        .iter()
        .max_by(|a, b| a.throughput.total_cmp(&b.throughput))
        .map_or(1, |r| r.worker_count);
    let mem = memory_gb();
    let p = (best as f64).sqrt().floor().max(2.0) as usize;
    Calibration {
        cpu_cores: cores,
        available_memory_gb: mem,
        monte_carlo_trials: (best * 2).max(4),
        parallel_workers: best,
        grid_memory_limit_mb: (mem * 150.0).min(2000.0) as usize,
        negotiated_iterations: 15 + (cores / 2).min(10),
        partition_rows: p,
        partition_cols: p,
        calibrated: true,
        benchmarks: runs,
        gpu_available: false,
    }
}
pub fn calibrate(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args = parse_args::<CalibrateArgs>("calibrate", args);
    let json = args.json || args.format == "json";
    if args.show || args.show_gpu {
        let Some(path) = args.output.as_ref() else {
            let v = serde_json::json!({"calibrated":false,"cpu_cores":std::thread::available_parallelism().map_or(1,usize::from),"available_memory_gb":memory_gb(),"gpu_available":false,"message":"pass --output to inspect a saved calibration"});
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?)
            } else {
                println!("No calibration path supplied. CPU cores: {}, memory: {:.1} GB; GPU calibration is unavailable in the all-Rust build.",v["cpu_cores"],v["available_memory_gb"].as_f64().unwrap())
            }
            return Ok(0);
        };
        let config: Calibration = serde_json::from_slice(&std::fs::read(path)?)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&config)?)
        } else {
            println!("Performance Configuration\n=========================\nCPU cores: {}\nAvailable memory: {:.1} GB\nParallel workers: {}\nMonte Carlo trials: {}\nGPU available: {}",config.cpu_cores,config.available_memory_gb,config.parallel_workers,config.monte_carlo_trials,config.gpu_available)
        }
        return Ok(0);
    }
    if args.gpu || args.all_backends {
        eprintln!("GPU calibration is unavailable in this all-Rust build; CPU calibration will continue and record gpu_available=false.")
    }
    let output = args
        .output
        .context("calibrate writes configuration; pass --output (or use --show)")?;
    let config = calibration(args.quick);
    std::fs::write(&output, serde_json::to_string_pretty(&config)? + "\n")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&config)?)
    } else {
        println!(
            "Calibration complete: {} workers, {} trials; saved to {}",
            config.parallel_workers,
            config.monte_carlo_trials,
            output.display()
        );
        if args.benchmark {
            for r in &config.benchmarks {
                println!(
                    "  {} workers: {:.1} ops/s ({:.2}ms)",
                    r.worker_count, r.throughput, r.duration_ms
                )
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calibration_has_valid_shape() {
        let c = calibration(true);
        assert!(c.cpu_cores >= 1 && c.parallel_workers >= 1 && c.monte_carlo_trials >= 4);
    }
    #[test]
    fn built_in_cases_unique() {
        let mut n = CASES.iter().map(|x| x.0).collect::<Vec<_>>();
        n.sort();
        n.dedup();
        assert_eq!(n.len(), CASES.len());
    }
}
