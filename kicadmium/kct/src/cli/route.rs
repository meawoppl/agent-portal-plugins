//! `kct route` / `kct route-auto` (port of `kicad_tools.cli.route_cmd` and
//! `cli/commands/routing.py`), driving the native Rust autorouter.
//!
//! Default flow mirrors upstream's layer-escalation mode: auto-select the
//! grid, auto-pour plane nets, route with negotiated congestion on the
//! board's layer count, escalate 2 -> 4 -> 6 layers while completion is
//! below `--min-completion`, save, refill zones with kicad-cli when
//! available, write the `<name>.route.json` artifact receipt and report.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

use super::route_args::{parse_or_exit, Namespace, ROUTE_AUTO_OPTS, ROUTE_OPTS};
use super::Globals;
use crate::router::core::{Autorouter, RouterConfig, RoutingStats};
use crate::router::io::{load_pcb_for_routing, merge_routes_into_pcb, pads_by_net, BoardData};
use crate::schema::pcb::{is_power_net, Pcb};

/// Exit code when routing completes below the requested completion rate.
pub const EXIT_PARTIAL: i32 = 1;

/// Resolved routing parameters shared by `route` and `route-auto`.
#[derive(Debug, Clone)]
pub struct RouteParams {
    pub pcb: PathBuf,
    pub output: PathBuf,
    pub trace_width: f64,
    pub clearance: f64,
    pub via_drill: f64,
    pub via_diameter: f64,
    pub grid: Option<f64>,
    pub max_cells: usize,
    pub iterations: usize,
    pub per_net_timeout: f64,
    pub timeout: Option<f64>,
    pub edge_clearance: f64,
    pub manufacturer: String,
    pub skip_nets: Vec<String>,
    pub only_nets: Option<Vec<String>>,
    pub layers: Option<usize>,
    pub auto_layers: bool,
    pub max_layers: usize,
    pub min_completion: f64,
    pub auto_pour: bool,
    pub preserve_existing: bool,
    pub strategy: String,
    pub dry_run: bool,
    pub skip_drc: bool,
    pub verbose: bool,
    pub quiet: bool,
    pub json: bool,
}

/// Board-declared Default net class (from the sibling `.kicad_pro`).
fn project_default_class(pcb: &Path) -> Option<(f64, f64, f64, f64)> {
    let pro = pcb.with_extension("kicad_pro");
    let text = std::fs::read_to_string(pro).ok()?;
    let v: Json = serde_json::from_str(&text).ok()?;
    let classes = v.get("net_settings")?.get("classes")?.as_array()?;
    let d = classes
        .iter()
        .find(|c| c.get("name").and_then(Json::as_str) == Some("Default"))?;
    Some((
        d.get("clearance").and_then(Json::as_f64).unwrap_or(0.0),
        d.get("track_width").and_then(Json::as_f64).unwrap_or(0.0),
        d.get("via_diameter").and_then(Json::as_f64).unwrap_or(0.0),
        d.get("via_drill").and_then(Json::as_f64).unwrap_or(0.0),
    ))
}

fn split_names(s: Option<&str>) -> Vec<String> {
    s.map(|s| {
        s.split(',')
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

fn resolve_params(ns: &Namespace, g: &Globals) -> Result<RouteParams, (i32, String)> {
    let pcb = PathBuf::from(&ns.positionals[0]);
    if !pcb.exists() {
        return Err((1, format!("Error: File not found: {}", pcb.display())));
    }
    if pcb.extension().and_then(|e| e.to_str()) != Some("kicad_pcb") {
        return Err((1, format!("Error: Expected .kicad_pcb file, got: {}", pcb.display())));
    }
    if ns.get("nets").is_some() && ns.get("skip_nets").is_some() {
        return Err((
            2,
            "Error: --nets and --skip-nets are mutually exclusive (--nets routes ONLY the listed nets; --skip-nets routes all BUT the listed nets).".into(),
        ));
    }
    let min_completion = ns.f64("min_completion").unwrap_or(0.95);
    if !(0.0..=1.0).contains(&min_completion) {
        return Err((1, "Error: --min-completion must be between 0 and 1".into()));
    }
    let output = match ns.get("output") {
        Some(o) => PathBuf::from(o),
        None => {
            let stem = pcb.file_stem().and_then(|s| s.to_str()).unwrap_or("board");
            pcb.with_file_name(format!("{stem}_routed.kicad_pcb"))
        }
    };
    let grid = match ns.get("grid") {
        None | Some("auto") | Some("AUTO") | Some("Auto") => None,
        Some(s) => match s.parse::<f64>() {
            Ok(v) if v > 0.0 => Some(v),
            _ => return Err((1, format!("Error: Invalid --grid value: {s}"))),
        },
    };
    let layers_arg = ns.str_or("layers", "auto");
    let layers = match layers_arg.as_str() {
        "auto" => None,
        "2" => Some(2),
        "6" => Some(6),
        _ => Some(4),
    };
    let explicit_auto_layers = ns.tri("auto_layers");
    if explicit_auto_layers == Some(true) && layers.is_some() && ns.was_explicit("--auto-layers") {
        return Err((
            1,
            "Error: --auto-layers and --layers are mutually exclusive.".into(),
        ));
    }
    let auto_layers = explicit_auto_layers.unwrap_or(layers.is_none());
    let mut clearance = ns.f64("clearance").unwrap_or(0.15);
    let declared = project_default_class(&pcb);
    if !ns.was_explicit("--clearance") {
        if let Some((c, ..)) = declared {
            if c > 0.0 {
                clearance = c;
            }
        }
    }
    let trace_width = ns.f64("trace_width").unwrap_or(0.2);
    let quiet = ns.flag("quiet") || g.quiet;
    Ok(RouteParams {
        pcb,
        output,
        trace_width,
        clearance,
        via_drill: ns.f64("via_drill").unwrap_or(0.3),
        via_diameter: ns.f64("via_diameter").unwrap_or(0.6),
        grid,
        max_cells: ns.usize("max_cells").unwrap_or(500_000),
        iterations: ns.usize("iterations").unwrap_or(15),
        per_net_timeout: ns.f64("per_net_timeout").unwrap_or(30.0),
        timeout: ns.f64("timeout"),
        edge_clearance: ns.f64("edge_clearance").unwrap_or(0.3),
        manufacturer: ns.str_or("manufacturer", "jlcpcb"),
        skip_nets: split_names(ns.get("skip_nets")),
        only_nets: ns.get("nets").map(|s| split_names(Some(s))),
        layers,
        auto_layers,
        max_layers: ns.usize("max_layers").unwrap_or(6),
        min_completion,
        auto_pour: ns.tri("auto_pour").unwrap_or(true),
        preserve_existing: ns.flag("preserve_existing") || ns.flag("complete"),
        strategy: ns.str_or("strategy", "negotiated"),
        dry_run: ns.flag("dry_run"),
        skip_drc: ns.flag("skip_drc"),
        verbose: ns.flag("verbose") || g.verbose,
        quiet,
        json: ns.get("format") == Some("json"),
    })
}

/// Upstream `auto_select_grid_resolution`: the finest candidate grid whose
/// per-layer cell count fits `max_cells`, never coarser than the clearance.
pub fn auto_grid(width: f64, height: f64, clearance: f64, max_cells: usize, fine_pitch: bool) -> f64 {
    let mut candidates = vec![0.1, 0.127, 0.15, 0.2, 0.25];
    if fine_pitch {
        candidates.insert(0, 0.05);
    }
    for &g in &candidates {
        let cells = ((width / g).ceil() + 1.0) * ((height / g).ceil() + 1.0);
        if cells <= max_cells as f64 && g <= clearance.max(0.1) * 1.5 {
            return g;
        }
    }
    *candidates.last().unwrap()
}

fn min_pad_pitch(board: &BoardData) -> f64 {
    let mut by_ref: HashMap<&str, Vec<(f64, f64)>> = HashMap::new();
    for p in &board.pads {
        by_ref
            .entry(p.pad.r#ref.as_str())
            .or_default()
            .push((p.shape.cx, p.shape.cy));
    }
    let mut best = f64::MAX;
    for pts in by_ref.values() {
        if pts.len() > 64 {
            continue;
        }
        for i in 0..pts.len() {
            for j in i + 1..pts.len() {
                let d = (pts[i].0 - pts[j].0).hypot(pts[i].1 - pts[j].1);
                if d > 1e-6 {
                    best = best.min(d);
                }
            }
        }
    }
    best
}

/// Plane (pour) nets: power nets with >= 2 pads (upstream
/// `classify_pour_candidates` heuristic). GND-like nets go to the bottom
/// layer, the largest other power net to the top layer.
fn pour_assignments(board: &BoardData, layers: usize) -> Vec<(String, String)> {
    let pads = pads_by_net(board);
    let mut gnd: Vec<(usize, String)> = Vec::new();
    let mut pwr: Vec<(usize, String)> = Vec::new();
    for (net, idx) in &pads {
        let Some(name) = board.nets.get(net) else { continue };
        if idx.len() < 2 || !is_power_net(name, None) {
            continue;
        }
        let up = name.to_ascii_uppercase();
        if up.contains("GND") || up.starts_with("VSS") {
            gnd.push((idx.len(), name.clone()));
        } else {
            pwr.push((idx.len(), name.clone()));
        }
    }
    gnd.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    pwr.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out = Vec::new();
    if let Some((_, n)) = gnd.first() {
        out.push((n.clone(), if layers >= 4 { "In1.Cu".into() } else { "B.Cu".into() }));
    }
    if let Some((_, n)) = pwr.first() {
        out.push((n.clone(), if layers >= 4 { "In2.Cu".into() } else { "F.Cu".into() }));
    }
    out
}

fn zone_sexp(net: &str, layer: &str, bounds: (f64, f64, f64, f64), inset: f64, uuid: &str) -> String {
    let (x0, y0, x1, y1) = (bounds.0 + inset, bounds.1 + inset, bounds.2 - inset, bounds.3 - inset);
    format!(
        "(zone (net \"{net}\") (layer \"{layer}\") (uuid \"{uuid}\") (hatch edge 0.5) (priority 1) \
         (connect_pads (clearance 0.3)) (min_thickness 0.25) \
         (fill yes (thermal_gap 0.3) (thermal_bridge_width 0.4) (island_removal_mode 0)) \
         (polygon (pts (xy {} {}) (xy {} {}) (xy {} {}) (xy {} {}))))",
        fmt(x0),
        fmt(y0),
        fmt(x0),
        fmt(y1),
        fmt(x1),
        fmt(y1),
        fmt(x1),
        fmt(y0)
    )
}

fn fmt(v: f64) -> String {
    crate::router::primitives::fmt_num(v)
}

/// Whether every pad of `net` has copper on `layer` (so a pour there
/// connects the net without traces).
fn pour_connects_all(board: &BoardData, net: &str, layer: &str) -> bool {
    board
        .pads
        .iter()
        .filter(|p| p.pad.net_name == net && !p.copper.is_empty())
        .all(|p| p.copper.iter().any(|l| l == layer))
}

/// One routing attempt at a fixed layer count.
pub struct Attempt {
    pub layers: usize,
    pub router: Autorouter,
    pub stats: RoutingStats,
    pub completion: f64,
}

fn make_config(p: &RouteParams, layers: usize, grid: f64, skip: &HashSet<String>, late: &HashSet<String>) -> RouterConfig {
    RouterConfig {
        trace_width: p.trace_width,
        clearance: p.clearance,
        via_drill: p.via_drill,
        via_diameter: p.via_diameter,
        grid_resolution: grid,
        layers,
        edge_clearance: p.edge_clearance,
        max_iterations: p.iterations,
        per_net_timeout: p.per_net_timeout,
        total_timeout: p.timeout,
        skip_nets: skip.clone(),
        late_nets: late.clone(),
        verbose: p.verbose,
        quiet: p.quiet,
        ..Default::default()
    }
}

fn run_attempt(
    p: &RouteParams,
    pcb: &Pcb,
    layers: usize,
    grid: f64,
    skip: &HashSet<String>,
    late: &HashSet<String>,
) -> Result<Attempt> {
    let board = load_pcb_for_routing(pcb, layers)?;
    let mut board = board;
    if !p.preserve_existing {
        // Fresh route: existing copper of routed nets is replaced.
        board.fixed.retain(|f| {
            let name = board_net_name(pcb, f.net());
            skip.contains(&name)
        });
    }
    let cfg = make_config(p, layers, grid, skip, late);
    let mut router = Autorouter::new(board, cfg);
    if !p.quiet {
        println!(
            "  Edge clearance: {}mm, {} cells blocked",
            crate::utils::pyrepr::py_float_repr(p.edge_clearance),
            router.edge_blocked_cells
        );
        let b = router.board.bounds;
        println!("  Board size: {:.1}mm x {:.1}mm", b.2 - b.0, b.3 - b.1);
    }
    router.compute_order();
    if !p.quiet {
        println!("  Nets to route: {}", router.order.len());
        println!("\n  Routing ({})...", p.strategy);
    }
    let iterations = if p.strategy == "basic" { 0 } else { p.iterations };
    router.route_all_negotiated(iterations);
    let stats = router.get_statistics();
    let completion = if stats.nets_total == 0 {
        1.0
    } else {
        stats.nets_routed as f64 / stats.nets_total as f64
    };
    Ok(Attempt {
        layers,
        router,
        stats,
        completion,
    })
}

fn board_net_name(pcb: &Pcb, net: i64) -> String {
    pcb.get_net(net).map(|n| n.name.clone()).unwrap_or_default()
}

fn sha256_file(path: &Path) -> Option<(String, u64)> {
    let bytes = std::fs::read(path).ok()?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Some((format!("{:x}", h.finalize()), bytes.len() as u64))
}

/// Write the `<stem>.route.json` artifact receipt (upstream
/// `cli/route_receipt.py`, schema `kicad-tools.route-artifacts.v1`).
pub fn write_route_receipt(output: &Path, exit_code: i32) -> Result<PathBuf> {
    let stem = output.file_stem().and_then(|s| s.to_str()).unwrap_or("board");
    let dir = output.parent().unwrap_or(Path::new("."));
    let entry = |p: PathBuf| -> Json {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        match sha256_file(&p) {
            Some((sha, size)) => json!({"path": name, "state": "present", "sha256": sha, "size": size}),
            None => json!({"path": name, "state": "absent"}),
        }
    };
    let receipt = json!({
        "schema": "kicad-tools.route-artifacts.v1",
        "route_exit_code": exit_code,
        "scope": "Final artifact bytes only; not DRC or factory DFM qualification",
        "files": {
            "board": entry(output.to_path_buf()),
            "project": entry(dir.join(format!("{stem}.kicad_pro"))),
            "rules": entry(dir.join(format!("{stem}.kicad_dru"))),
        }
    });
    let path = dir.join(format!("{stem}.route.json"));
    let mut text = serde_json::to_string_pretty(&receipt)?;
    text.push('\n');
    crate::fsutil::atomic_write(&path, text.as_bytes())?;
    Ok(path)
}

/// Copy the input project file next to the output so KiCad tools see the
/// same design rules (upstream DRC-constraint sidecars).
fn write_project_sidecar(input: &Path, output: &Path) -> Option<PathBuf> {
    let src = input.with_extension("kicad_pro");
    let dst = output.with_extension("kicad_pro");
    if src == dst || !src.exists() {
        return None;
    }
    std::fs::copy(&src, &dst).ok()?;
    let dru = input.with_extension("kicad_dru");
    if dru.exists() {
        let _ = std::fs::copy(&dru, output.with_extension("kicad_dru"));
    }
    Some(dst)
}

/// Refill zones with `kicad-cli pcb drc --refill-zones --save-board`;
/// returns the DRC report JSON when available.
fn fill_zones_and_drc(output: &Path, quiet: bool) -> Option<Json> {
    let cli = crate::cli::runner::find_kicad_cli()?;
    let text = std::fs::read_to_string(output).ok()?;
    let has_zones = text.contains("(zone");
    let report = std::env::temp_dir().join(format!("kct-route-drc-{}.json", std::process::id()));
    let mut cmd = std::process::Command::new(&cli);
    cmd.args(["pcb", "drc", "--format", "json", "--severity-all", "-o"]).arg(&report);
    if has_zones {
        cmd.args(["--refill-zones", "--save-board"]);
        if !quiet {
            println!("\n--- Filling Copper Zones ---");
            println!(
                "  Filling zones in {}...",
                output.file_name().and_then(|s| s.to_str()).unwrap_or("")
            );
        }
    }
    cmd.arg(output);
    let out = cmd.output().ok()?;
    if has_zones && !quiet {
        if out.status.success() || report.exists() {
            println!("  Zone fill: complete");
        } else {
            println!("  Zone fill: failed");
        }
    }
    let rep = std::fs::read_to_string(&report).ok();
    let _ = std::fs::remove_file(&report);
    rep.and_then(|r| serde_json::from_str(&r).ok())
}

/// Copper-relevant DRC violation types (routing defects).
const ROUTING_DRC_TYPES: &[&str] = &[
    "clearance",
    "shorting_items",
    "tracks_crossing",
    "hole_clearance",
    "hole_to_hole",
    "copper_edge_clearance",
    "track_dangling",
    "via_dangling",
    "annular_width",
    "track_width",
];

fn summarize_drc(report: &Json) -> (usize, usize, Vec<String>, usize) {
    let mut errors = 0;
    let mut warnings = 0;
    let mut lines = Vec::new();
    let mut routing = 0;
    if let Some(v) = report.get("violations").and_then(Json::as_array) {
        for x in v {
            let sev = x.get("severity").and_then(Json::as_str).unwrap_or("");
            let ty = x.get("type").and_then(Json::as_str).unwrap_or("");
            let desc = x.get("description").and_then(Json::as_str).unwrap_or("");
            if sev == "error" {
                errors += 1;
            } else {
                warnings += 1;
            }
            if ROUTING_DRC_TYPES.contains(&ty) {
                routing += 1;
            }
            if lines.len() < 10 {
                lines.push(format!("{ty}: {desc}"));
            }
        }
    }
    let _ = (errors, warnings);
    (errors, warnings, lines, routing)
}

fn print_results(stats: &RoutingStats) {
    println!("\n--- Results ---");
    println!("  Routes created:  {}", stats.routes);
    println!("  Segments:        {}", stats.segments);
    println!("  Vias:            {}", stats.vias);
    println!("  Total length:    {:.2}mm", stats.total_length_mm);
    println!("  Nets routed:     {}/{}", stats.nets_routed, stats.nets_total);
    println!(
        "  Partial routes:  {}/{} -- have segments, not all pads connected",
        stats.nets_partial, stats.nets_total
    );
    println!(
        "  Unrouted:        {}/{} -- no segments at all",
        stats.nets_unrouted, stats.nets_total
    );
}

/// `kct route` entry point.
pub fn run(args: Vec<OsString>, g: &Globals) -> Result<i32> {
    let ns = match parse_or_exit("route", ROUTE_OPTS, &["pcb"], &args) {
        Ok(ns) => ns,
        Err(code) => return Ok(code),
    };
    let p = match resolve_params(&ns, g) {
        Ok(p) => p,
        Err((code, msg)) => {
            eprintln!("{msg}");
            return Ok(code);
        }
    };
    route_main(&p)
}

/// Shared route driver (also used by `route-auto` / benchmarks).
pub fn route_main(p: &RouteParams) -> Result<i32> {
    let started = Instant::now();
    let mut pcb = Pcb::load(&p.pcb)?;
    let detected_layers = pcb.copper_layers().len().max(2);
    let quiet = p.quiet;
    // Validate --nets names.
    let all_names: Vec<String> = pcb.nets().iter().map(|n| n.name.clone()).filter(|n| !n.is_empty()).collect();
    let mut skip: HashSet<String> = p.skip_nets.iter().cloned().collect();
    if let Some(only) = &p.only_nets {
        for n in only {
            if !all_names.contains(n) {
                eprintln!("Error: --nets: net '{n}' not found on the board");
                return Ok(1);
            }
        }
        for n in &all_names {
            if !only.contains(n) {
                skip.insert(n.clone());
            }
        }
    }
    let start_layers = p.layers.unwrap_or(detected_layers).min(p.max_layers.max(2));
    // Auto-pour plane nets.
    let mut late: HashSet<String> = HashSet::new();
    let board0 = load_pcb_for_routing(&pcb, start_layers)?;
    let mut pour_skipped: Vec<String> = Vec::new();
    let mut zone_text: Vec<String> = Vec::new();
    if p.auto_pour && p.only_nets.is_none() {
        let assigns = pour_assignments(&board0, start_layers);
        let mut created = Vec::new();
        for (net, layer) in &assigns {
            if board0.zone_nets.contains(net) || skip.contains(net) {
                continue;
            }
            let uuid = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, format!("{net}:{layer}").as_bytes())
                .to_string();
            zone_text.push(zone_sexp(net, layer, board0.bounds, p.edge_clearance, &uuid));
            created.push(net.clone());
            if pour_connects_all(&board0, net, layer) {
                pour_skipped.push(net.clone());
            } else {
                late.insert(net.clone());
            }
        }
        if !created.is_empty() && !quiet {
            println!(
                "Auto-pour: created {} zone(s) for {}",
                created.len(),
                created.join(", ")
            );
        }
        for (net, _) in &assigns {
            if board0.zone_nets.contains(net) && pour_connects_all(&board0, net, &assigns.iter().find(|a| &a.0 == net).unwrap().1) {
                pour_skipped.push(net.clone());
            }
        }
        if !pour_skipped.is_empty() && !quiet {
            println!("Auto-skip: {} (pour nets — use zone fill)", pour_skipped.join(", "));
        }
        for n in &pour_skipped {
            skip.insert(n.clone());
        }
    }
    let (bw, bh) = (board0.bounds.2 - board0.bounds.0, board0.bounds.3 - board0.bounds.1);
    let fine = min_pad_pitch(&board0) < 0.65;
    let grid = p
        .grid
        .unwrap_or_else(|| auto_grid(bw, bh, p.clearance, p.max_cells, fine));
    if !quiet {
        println!("{}", "=".repeat(60));
        println!(
            "KiCad PCB Autorouter{}",
            if p.auto_layers { " - Layer Escalation Mode" } else { "" }
        );
        println!("{}", "=".repeat(60));
        println!("Input:          {}", p.pcb.display());
        println!("Output:         {}", p.output.display());
        println!("Strategy:       {}", p.strategy);
        println!("Grid:           {grid}mm");
        println!("Trace width:    {}mm", p.trace_width);
        println!("Clearance:      {}mm", p.clearance);
        if p.auto_layers {
            println!("Max layers:     {}", p.max_layers);
            println!("Min completion: {:.0}%", p.min_completion * 100.0);
        }
        if !skip.is_empty() && p.only_nets.is_none() {
            let mut s: Vec<&String> = skip.iter().collect();
            s.sort();
            println!("Skip:           {}", s.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
        }
    }
    if p.dry_run {
        if !quiet {
            println!("\nDry run: {} nets would be routed on {start_layers} layers (grid {grid}mm)", pads_by_net(&board0).iter().filter(|(n, v)| v.len() >= 2 && !skip.contains(board0.nets.get(n).map(String::as_str).unwrap_or(""))).count());
        }
        return Ok(0);
    }
    // Layer escalation.
    let mut ladder: Vec<usize> = vec![start_layers];
    if p.auto_layers {
        for l in [2usize, 4, 6] {
            if l > start_layers && l <= p.max_layers {
                ladder.push(l);
            }
        }
    }
    let mut best: Option<Attempt> = None;
    for (k, &layers) in ladder.iter().enumerate() {
        if !quiet {
            println!("\n{}", "=".repeat(60));
            println!("Attempt {}: {layers} layers", k + 1);
            println!("{}", "=".repeat(60));
        }
        let attempt = run_attempt(p, &pcb, layers, grid, &skip, &late)?;
        if !quiet {
            println!(
                "\n  Routed: {}/{} nets ({:.0}%)",
                attempt.stats.nets_routed,
                attempt.stats.nets_total,
                attempt.completion * 100.0
            );
            println!(
                "  Status: {}",
                if attempt.completion >= p.min_completion { "SUCCESS" } else { "INSUFFICIENT" }
            );
        }
        let better = best
            .as_ref()
            .is_none_or(|b| attempt.completion > b.completion + 1e-9);
        let done = attempt.completion >= p.min_completion;
        if better {
            best = Some(attempt);
        }
        if done {
            break;
        }
        if p.timeout.is_some_and(|t| started.elapsed().as_secs_f64() > t) {
            break;
        }
    }
    let best = best.expect("at least one attempt");
    if !quiet && p.auto_layers {
        println!("\n{}", "=".repeat(60));
        println!("LAYER ESCALATION SUMMARY");
        println!("{}", "=".repeat(60));
        if best.completion >= p.min_completion {
            println!(
                "Result: Design routed successfully on {} layers ({:.0}% completion)",
                best.layers,
                best.completion * 100.0
            );
        } else {
            println!(
                "Result: Best result {:.0}% completion on {} layers (below {:.0}% target)",
                best.completion * 100.0,
                best.layers,
                p.min_completion * 100.0
            );
        }
    }
    if !quiet {
        print_results(&best.stats);
        let failed: Vec<String> = best
            .router
            .results
            .values()
            .filter(|r| !r.is_complete())
            .map(|r| format!("{} ({}/{} pads)", r.name, r.connected_pads, r.pad_count))
            .collect();
        if !failed.is_empty() {
            println!("  Failed nets:     {}", failed.join(", "));
        }
    }
    // Save.
    let routes = best.router.routes();
    let mut strip: Vec<String> = Vec::new();
    if !p.preserve_existing {
        for net in &best.router.order {
            if let Some(n) = best.router.board.nets.get(net) {
                strip.push(n.clone());
            }
        }
    }
    if !zone_text.is_empty() {
        for z in &zone_text {
            let node = crate::sexp::parse(z)?;
            pcb.sexp_mut().push(node);
        }
    }
    let staged = std::env::temp_dir().join(format!("kct-route-{}.kicad_pcb", std::process::id()));
    pcb.save(Some(&staged))?;
    merge_routes_into_pcb(&staged, &p.output, &routes, &strip, best.layers)?;
    let _ = std::fs::remove_file(&staged);
    if !quiet {
        println!("\n--- Saving routed PCB ---");
        println!("  Saved to: {}", p.output.display());
        println!("  Layer count: {}", best.layers);
    }
    if let Some(pro) = write_project_sidecar(&p.pcb, &p.output) {
        if !quiet {
            println!("  Project sidecar: {}", pro.display());
        }
    }
    // Zone fill + DRC.
    let mut drc_routing_errors = 0;
    let drc = if p.skip_drc && zone_text.is_empty() {
        None
    } else {
        fill_zones_and_drc(&p.output, quiet)
    };
    if let (Some(report), false) = (&drc, p.skip_drc) {
        let (errors, warnings, lines, routing) = summarize_drc(report);
        drc_routing_errors = routing;
        if !quiet {
            println!("\n--- DRC Validation ---");
            if errors == 0 && warnings == 0 {
                println!("  DRC: clean");
            } else {
                if errors > 0 {
                    println!("  Errors: {errors}");
                }
                if warnings > 0 {
                    println!("  Warnings: {warnings}");
                }
                for l in &lines {
                    println!("    - {l}");
                }
            }
            println!("\n  Run 'kct check {} --mfr {}' for full details", p.output.display(), p.manufacturer);
        }
    }
    let exit = if best.completion >= p.min_completion { 0 } else { EXIT_PARTIAL };
    let receipt = write_route_receipt(&p.output, exit)?;
    if p.json {
        let s = &best.stats;
        let doc = json!({
            "input": p.pcb.display().to_string(),
            "output": p.output.display().to_string(),
            "receipt": receipt.display().to_string(),
            "layers": best.layers,
            "grid": grid,
            "strategy": p.strategy,
            "success": exit == 0,
            "completion": best.completion,
            "routes": s.routes,
            "segments": s.segments,
            "vias": s.vias,
            "total_length_mm": (s.total_length_mm * 1000.0).round() / 1000.0,
            "nets_routed": s.nets_routed,
            "nets_partial": s.nets_partial,
            "nets_unrouted": s.nets_unrouted,
            "nets_total": s.nets_total,
            "drc_routing_violations": drc_routing_errors,
            "elapsed_s": (started.elapsed().as_secs_f64() * 100.0).round() / 100.0,
            "unrouted_nets": best.router.results.values().filter(|r| !r.is_complete()).map(|r| r.name.clone()).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&doc)?);
    }
    if !quiet {
        println!("\n{}", "=".repeat(60));
        if exit == 0 {
            println!("SUCCESS: Design requires minimum {} layers", best.layers);
        } else {
            println!(
                "PARTIAL: {:.0}% completion on {} layers (min {:.0}%)",
                best.completion * 100.0,
                best.layers,
                p.min_completion * 100.0
            );
        }
    }
    Ok(exit)
}

/// `kct route-auto` entry point: route the listed net(s) (or all nets)
/// with the native router, keeping all other copper fixed.
pub fn run_auto(args: Vec<OsString>, g: &Globals) -> Result<i32> {
    let ns = match parse_or_exit("route-auto", ROUTE_AUTO_OPTS, &["pcb"], &args) {
        Ok(ns) => ns,
        Err(code) => return Ok(code),
    };
    let pcb_path = PathBuf::from(&ns.positionals[0]);
    let as_json = ns.get("format") == Some("json");
    if !pcb_path.exists() {
        eprintln!("Error: File not found: {}", pcb_path.display());
        return Ok(1);
    }
    let mut targets: Vec<String> = Vec::new();
    if let Some(n) = ns.get("net") {
        targets.push(n.to_string());
    }
    targets.extend(split_names(ns.get("nets")));
    let pcb = Pcb::load(&pcb_path)?;
    let names: Vec<String> = pcb.nets().iter().map(|n| n.name.clone()).filter(|n| !n.is_empty()).collect();
    if targets.is_empty() {
        targets = names.clone();
    }
    for t in &targets {
        if !names.contains(t) {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&json!({"error": format!("Net '{t}' not found"), "nets": []}))?);
            } else {
                eprintln!("Error: Net '{t}' not found in {}", pcb_path.display());
            }
            return Ok(1);
        }
    }
    // Never rewrite the input implicitly: require -o/--output or --in-place.
    let output = match (ns.get("output"), ns.flag("in_place")) {
        (Some(o), _) => PathBuf::from(o),
        (None, true) => pcb_path.clone(),
        (None, false) if ns.flag("dry_run") => pcb_path.clone(),
        (None, false) => {
            let msg = "Error: route-auto needs -o/--output PATH (or --in-place to overwrite the input board)";
            if as_json {
                println!("{}", serde_json::to_string_pretty(&json!({"error": msg, "nets": []}))?);
            } else {
                eprintln!("{msg}");
            }
            return Ok(2);
        }
    };
    let declared = project_default_class(&pcb_path);
    let via_d = ns.f64("via_diameter").or(declared.map(|d| d.2).filter(|v| *v > 0.0)).unwrap_or(0.6);
    let via_dr = ns.f64("via_drill").or(declared.map(|d| d.3).filter(|v| *v > 0.0)).unwrap_or(0.3);
    let strategy = ns.str_or("strategy", "auto");
    let params = RouteParams {
        pcb: pcb_path.clone(),
        output: output.clone(),
        trace_width: 0.2,
        clearance: declared.map(|d| d.0).filter(|c| *c > 0.0).unwrap_or(0.15),
        via_drill: via_dr,
        via_diameter: via_d,
        grid: None,
        max_cells: 500_000,
        iterations: 15,
        per_net_timeout: 30.0,
        timeout: None,
        edge_clearance: 0.3,
        manufacturer: "jlcpcb".into(),
        skip_nets: Vec::new(),
        only_nets: Some(targets.clone()),
        layers: None,
        auto_layers: false,
        max_layers: 6,
        min_completion: if ns.flag("allow_partial") { 0.0 } else { 1.0 },
        auto_pour: false,
        preserve_existing: true,
        strategy: strategy.clone(),
        dry_run: ns.flag("dry_run"),
        skip_drc: true,
        verbose: ns.flag("verbose") || g.verbose,
        quiet: true,
        json: false,
    };
    if !as_json {
        println!("Routing {} net(s) in {} (strategy: {strategy})", targets.len(), pcb_path.display());
        println!("  Via geometry: {via_d}mm diameter / {via_dr}mm drill");
    }
    if params.dry_run {
        if as_json {
            println!("{}", serde_json::to_string_pretty(&json!({"board": pcb_path.display().to_string(), "output": output.display().to_string(), "dry_run": true, "nets": targets.iter().map(|t| json!({"net": t, "strategy": strategy})).collect::<Vec<_>>()}))?);
        } else {
            println!("Dry run: no changes written");
        }
        return Ok(0);
    }
    // Route.
    let detected = pcb.copper_layers().len().max(2);
    let board = load_pcb_for_routing(&pcb, detected)?;
    let (bw, bh) = (board.bounds.2 - board.bounds.0, board.bounds.3 - board.bounds.1);
    let grid = auto_grid(bw, bh, params.clearance, params.max_cells, min_pad_pitch(&board) < 0.65);
    let skip: HashSet<String> = names.iter().filter(|n| !targets.contains(n)).cloned().collect();
    // Existing copper of the target nets is kept as same-net fixed copper.
    let attempt = run_attempt(&params, &pcb, detected, grid, &skip, &HashSet::new())?;
    let routes = attempt.router.routes();
    merge_routes_into_pcb(&pcb_path, &output, &routes, &[], detected)?;
    let mut entries = Vec::new();
    let mut failed = 0;
    for t in &targets {
        let res = attempt.router.results.values().find(|r| &r.name == t);
        let (ok, segs, vias, len) = match res {
            Some(r) => (r.is_complete(), r.route.segments.len(), r.route.vias.len(), r.route.total_length()),
            None => (true, 0, 0, 0.0),
        };
        if !ok {
            failed += 1;
        }
        if !as_json {
            println!(
                "  {t}: {} ({segs} segments, {vias} vias, {len:.2}mm)",
                if ok { "routed" } else { "FAILED" }
            );
        }
        entries.push(json!({
            "net": t,
            "strategy": if strategy == "auto" { "global" } else { strategy.as_str() },
            "success": ok,
            "metrics": {"segments": segs, "vias": vias, "length_mm": (len * 1000.0).round() / 1000.0},
            "warnings": [],
        }));
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&json!({
            "board": pcb_path.display().to_string(),
            "output": output.display().to_string(),
            "nets": entries,
        }))?);
    } else {
        println!("Saved: {}", output.display());
    }
    Ok(if failed > 0 && !ns.flag("allow_partial") { 1 } else { 0 })
}
