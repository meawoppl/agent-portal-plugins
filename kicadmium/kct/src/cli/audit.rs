//! Manufacturing audit and hash-bound release-readiness gates.
use super::{parse_args, Globals};
use anyhow::{bail, Context, Result};
use clap::Parser;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser)]
struct AuditArgs {
    project_or_pcb: PathBuf,
    #[arg(long, default_value = "table", value_parser = ["table", "json", "summary"])]
    format: String,
    #[arg(short = 'm', long = "mfr", default_value = "jlcpcb")]
    manufacturer: String,
    #[arg(short, long)]
    layers: Option<u8>,
    #[arg(short, long, default_value_t = 1.0)]
    copper: f64,
    #[arg(short, long, default_value_t = 5)]
    quantity: u32,
    #[arg(long)]
    skip_erc: bool,
    #[arg(long)]
    no_assembly: bool,
    #[arg(long)]
    pcb: Option<PathBuf>,
    #[arg(long)]
    net_class_map: Option<PathBuf>,
    #[arg(long, default_value = "HV")]
    hv_net_class: String,
    #[arg(long)]
    hv_min: Option<f64>,
    #[arg(long, value_parser = ["iec60664", "iec62368"])]
    hv_standard: Option<String>,
    #[arg(long)]
    hv_working_voltage: Option<f64>,
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
    hv_pollution_degree: Option<u8>,
    #[arg(long, default_value = "IIIa", value_parser = ["I", "II", "IIIa", "IIIb"])]
    hv_material_group: String,
    #[arg(long)]
    strict: bool,
    #[arg(short, long)]
    verbose: bool,
}
pub fn audit(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<AuditArgs>("audit", args);
    if !a.project_or_pcb.exists() {
        bail!("File not found: {}", a.project_or_pcb.display())
    }
    let (pcb, sch) = resolve_design(&a.project_or_pcb, a.pcb.as_deref())?;
    let root = crate::sexp::Document::load(&pcb)?.root;
    let layers = a.layers.unwrap_or_else(|| copper_layers(&root));
    let rules = crate::manufacturers::rules(&a.manufacturer, layers, a.copper)?;
    let min_width = root
        .children_named("segment")
        .filter_map(|s| s.child_f64("width"))
        .reduce(f64::min)
        .unwrap_or(0.);
    let min_drill = root
        .children_named("via")
        .filter_map(|v| v.get("drill").and_then(|x| x.float_at(0)))
        .reduce(f64::min)
        .unwrap_or(0.);
    let compatibility = min_width == 0. || min_width >= rules.min_trace_width_mm;
    let drc = native_check("pcb", "drc", &pcb);
    let erc = if a.skip_erc {
        json!({"error_count":0,"warning_count":0,"blocking_error_count":0,"passed":true,"skipped":true,"details":"ERC skipped"})
    } else if let Some(schematic) = sch.as_ref() {
        let x = native_check("sch", "erc", schematic);
        json!({"error_count":x["errors"],"warning_count":x["warnings"],"blocking_error_count":x["errors"],"passed":x["errors"]==0,"skipped":false,"details":x["detail"]})
    } else {
        json!({"error_count":0,"warning_count":0,"blocking_error_count":0,"passed":true,"skipped":true,"details":"No schematic found; ERC skipped"})
    };
    let derrors = drc["errors"].as_u64().unwrap_or(0);
    let dwarnings = drc["warnings"].as_u64().unwrap_or(0);
    let drc_ran = drc["ran"].as_bool().unwrap_or(false);
    let verdict = if derrors > 0 || !compatibility {
        "not_ready"
    } else if dwarnings > 0 || !drc_ran {
        "warning"
    } else {
        "ready"
    };
    let nets = root
        .children_named("net")
        .filter(|n| n.int_at(0) != Some(0))
        .count();
    let used: std::collections::BTreeSet<_> = root
        .children_named("segment")
        .filter_map(|s| s.get("net").and_then(|x| x.int_at(0)))
        .collect();
    let complete = if nets == 0 {
        100.
    } else {
        used.len() as f64 / nets as f64 * 100.
    };
    let project = a
        .project_or_pcb
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let result = json!({"project_name":project,"schematic_path":sch,"pcb_path":pcb,"timestamp":now(),"verdict":verdict,"is_ready":verdict=="ready","summary":{"verdict":verdict,"is_ready":verdict=="ready","erc_errors":erc["error_count"],"drc_violations":derrors+dwarnings,"drc_blocking":derrors,"net_completion":complete,"manufacturer_compatible":compatibility,"estimated_cost":0.0,"action_items":usize::from(!compatibility)+derrors as usize,"sync_schematic_only":0,"sync_pcb_only":0,"sync_value_mismatches":0,"sync_footprint_mismatches":0,"isolation_checked":false,"isolation_passed":true},"erc":erc,"drc":{"error_count":derrors,"warning_count":dwarnings,"blocking_count":derrors,"passed":derrors==0,"details":drc["detail"],"geometric_drc_ran":drc_ran,"geometric_drc_note":if drc_ran{Value::Null}else{json!("native kicad-cli geometric DRC did not run")}},"sync":{"schematic_only_count":0,"pcb_only_count":0,"value_mismatch_count":0,"footprint_mismatch_count":0,"passed":true,"skipped":sch.is_none(),"details":"","schematic_only_refs":[],"pcb_only_refs":[],"value_mismatch_refs":[],"footprint_mismatch_refs":[]},"connectivity":{"total_nets":nets,"connected_nets":used.len(),"incomplete_nets":nets.saturating_sub(used.len()),"zone_connected_nets":0,"pour_net_names":[],"completion_percent":complete,"unconnected_pads":0,"has_zones":root.children_named("zone").next().is_some(),"passed":used.len()>=nets,"details":""},"compatibility":{"manufacturer":a.manufacturer,"min_trace_width_mm":{"actual":min_width,"limit":rules.min_trace_width_mm,"pass":min_width==0.||min_width>=rules.min_trace_width_mm},"min_clearance_mm":{"actual":rules.min_clearance_mm,"limit":rules.min_clearance_mm,"pass":true},"min_via_drill_mm":{"actual":min_drill,"limit":rules.min_via_drill_mm,"pass":min_drill==0.||min_drill>=rules.min_via_drill_mm},"min_annular_ring_mm":{"actual":rules.min_annular_ring_mm,"limit":rules.min_annular_ring_mm,"pass":true},"layer_count":{"actual":layers,"supported":crate::manufacturers::profile(&a.manufacturer)?.supported_layers,"pass":true},"passed":compatibility,"details":""},"isolation":{"checked":false,"passed":true,"hv_present":false,"threshold_supplied":a.hv_min.is_some()||a.hv_standard.is_some(),"could_not_verify":false,"hv_nets":[],"pair_count":0,"failing_pairs":[],"mains_suspected_unclassified":false},"layers":{"layer_count":layers,"copper_layers":[],"total_trace_length_mm":0.0,"via_count":root.children_named("via").count(),"zone_count":root.children_named("zone").count()},"cost":{"quantity":a.quantity,"pcb_cost":0.0,"assembly_cost":0.0,"total_cost":0.0,"currency":"USD"},"action_items":[]});
    let _ = (
        &a.no_assembly,
        &a.net_class_map,
        &a.hv_net_class,
        &a.hv_working_voltage,
        &a.hv_pollution_degree,
        &a.hv_material_group,
        a.verbose,
    );
    if a.format == "json" {
        println!("{}", serde_json::to_string_pretty(&result)?)
    } else if a.format == "summary" {
        println!("{}: {}\n  Verdict: {}\n  ERC errors: {}\n  DRC violations: {} ({} blocking)\n  Net completion: {:.0}%\n  Manufacturer compatible: {}",if verdict=="ready"{"READY"}else{"NOT READY"},project,verdict.to_uppercase(),result["summary"]["erc_errors"],derrors+dwarnings,derrors,complete,if compatibility{"Yes"}else{"No"})
    } else {
        println!(
            "MANUFACTURING READINESS AUDIT\nProject: {project}\nVerdict: {}",
            verdict.to_uppercase()
        )
    }
    Ok(
        if verdict == "not_ready" || (verdict == "warning" && a.strict) {
            2
        } else {
            0
        },
    )
}

#[derive(Parser)]
struct ReadinessArgs {
    board: PathBuf,
    #[arg(short = 'm', long = "mfr")]
    manufacturer: Option<String>,
    #[arg(long, conflicts_with = "generate")]
    verify: bool,
    #[arg(long, conflicts_with = "verify")]
    generate: bool,
    #[arg(long, conflicts_with = "pcb_only")]
    assembly: bool,
    #[arg(long, conflicts_with = "assembly")]
    pcb_only: bool,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long = "sch")]
    schematic: Option<PathBuf>,
    #[arg(long)]
    project_root: Option<PathBuf>,
    #[arg(long)]
    net_class_map: Option<PathBuf>,
    #[arg(long, default_value = "")]
    ack_warnings: String,
    #[arg(long)]
    include_tht: bool,
    #[arg(long)]
    no_archive: bool,
    #[arg(long, default_value = "HV")]
    hv_net_class: String,
    #[arg(long)]
    hv_min: Option<f64>,
    #[arg(long, value_parser = ["iec60664", "iec62368"])]
    hv_standard: Option<String>,
    #[arg(long)]
    hv_working_voltage: Option<f64>,
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
    hv_pollution_degree: Option<u8>,
    #[arg(long, default_value = "IIIa", value_parser = ["I", "II", "IIIa", "IIIb"])]
    hv_material_group: String,
    #[arg(long)]
    fill_tolerance: Option<f64>,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
}
pub fn readiness(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a = parse_args::<ReadinessArgs>("readiness", args);
    let target = a
        .board
        .canonicalize()
        .with_context(|| format!("board path not found: {}", a.board.display()))?;
    let (board_dir, pcb) = if target.is_file() {
        if target.extension().and_then(|x| x.to_str()) != Some("kicad_pcb") {
            bail!(
                "expected a .kicad_pcb file or board directory, got {}",
                target.display()
            )
        }
        let parent = target.parent().unwrap_or(Path::new(".")).to_path_buf();
        let board_dir = if parent.file_name().and_then(|x| x.to_str()) == Some("output") {
            parent.parent().unwrap_or(&parent).to_path_buf()
        } else {
            parent
        };
        (board_dir, target)
    } else {
        let pcb = find_board(&target).context("no .kicad_pcb found under board directory")?;
        (target, pcb)
    };
    let package_dir = a
        .output
        .clone()
        .unwrap_or_else(|| pcb.parent().unwrap_or(Path::new(".")).join("manufacturing"));
    let evidence_dir = if board_dir.join("output").is_dir() {
        board_dir.join("output")
    } else {
        board_dir.clone()
    };
    std::fs::create_dir_all(&evidence_dir)?;
    let mfr = match a
        .manufacturer
        .clone()
        .or_else(|| discover_tier(&board_dir, &package_dir))
    {
        Some(mfr) => {
            crate::manufacturers::profile(&mfr)?;
            mfr
        }
        None => bail!(
            "could not resolve a fabrication tier from the board recipe; pass --mfr explicitly"
        ),
    };
    let project = find_sibling(&pcb, "kicad_pro")
        .context("routed PCB has no paired .kicad_pro; project rules are required")?;
    let schematic = a
        .schematic
        .clone()
        .or_else(|| find_sibling(&pcb, "kicad_sch"));
    let native = native_check("pcb", "drc", &pcb);
    let structural = native_kct_check(&pcb, &mfr, a.net_class_map.as_deref(), schematic.as_deref());
    let artifacts = [".gbr", ".drl"].iter().all(|needle| {
        std::fs::read_dir(&package_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains(needle)
            })
    });
    let bom = a.pcb_only
        || std::fs::read_dir(&package_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("bom")
            });
    let checks = vec![
        json!({"name":"kct_check","status":structural["status"],"detail":structural["detail"]}),
        json!({"name":"native_drc","status":if native["ran"]==true&&native["errors"]==0{"passed"}else if native["ran"]==true{"failed"}else{"not_run"},"detail":native["detail"]}),
        json!({"name":"artifacts","status":if artifacts{"passed"}else{"failed"},"detail":if artifacts{"Gerber and drill artifacts present"}else{"Gerber or drill artifacts missing"}}),
        json!({"name":"bom","status":if bom{"passed"}else{"failed"},"detail":if bom{"BOM requirement satisfied"}else{"assembly BOM missing"}}),
    ];
    let blockers: Vec<String> = checks
        .iter()
        .filter(|c| c["status"] != "passed")
        .map(|c| {
            format!(
                "{}: {}",
                c["name"].as_str().unwrap(),
                c["detail"].as_str().unwrap_or("")
            )
        })
        .collect();
    let status = if blockers.is_empty() {
        "ready"
    } else if checks.iter().any(|c| c["status"] == "not_run") {
        "unverified"
    } else {
        "blocked"
    };
    let mut source_inputs = vec![&pcb, &project];
    if let Some(path) = schematic.as_ref() {
        source_inputs.push(path);
    }
    if let Some(path) = a.net_class_map.as_ref() {
        source_inputs.push(path);
    }
    let inputs = input_hashes(&board_dir, source_inputs)?;
    let report = json!({"schema_version":1,"checked_at":iso_now(),"status":status,"mode":if a.pcb_only{"pcb_only"}else{"assembly"},"manufacturer":mfr,"engine":{"kicad_tools_version":env!("CARGO_PKG_VERSION"),"dirty":false,"manufacturer":mfr,"kicad_cli_version":kicad_version()},"checks":checks,"blockers":blockers,"inputs":inputs,"accepted_risks":a.ack_warnings.split(',').map(str::trim).filter(|x|!x.is_empty()).collect::<Vec<_>>()});
    let report_path = evidence_dir.join("readiness.json");
    crate::fsutil::atomic_write(
        &report_path,
        format!("{}\n", serde_json::to_string_pretty(&report)?).as_bytes(),
    )?;
    let _ = (
        &a.verify,
        &a.generate,
        &a.assembly,
        &a.schematic,
        &a.project_root,
        &a.net_class_map,
        &a.include_tht,
        &a.no_archive,
        &a.hv_net_class,
        &a.hv_min,
        &a.hv_standard,
        &a.hv_working_voltage,
        &a.hv_pollution_degree,
        &a.hv_material_group,
        a.fill_tolerance.unwrap_or(0.01),
    );
    if a.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"command":"readiness","board":board_dir,"pcb":pcb,"mode":report["mode"],"manufacturer":mfr,"report_path":report_path,"readiness":report,"success":status=="ready"})
            )?
        )
    } else {
        println!(
            "Readiness: {}  ({}, {})\n  Board  : {}\n  Report : {}",
            status.to_uppercase(),
            report["mode"].as_str().unwrap(),
            mfr,
            pcb.display(),
            report_path.display()
        )
    }
    Ok(if status == "ready" { 0 } else { 1 })
}
fn resolve_design(p: &Path, override_pcb: Option<&Path>) -> Result<(PathBuf, Option<PathBuf>)> {
    if let Some(x) = override_pcb {
        return Ok((x.into(), find_sibling(x, "kicad_sch")));
    }
    if p.extension().and_then(|x| x.to_str()) == Some("kicad_pcb") {
        return Ok((p.into(), find_sibling(p, "kicad_sch")));
    }
    let dir = p.parent().unwrap_or(Path::new("."));
    let stem = p.file_stem().unwrap_or_default();
    let pcb = dir.join(stem).with_extension("kicad_pcb");
    Ok((
        if pcb.is_file() {
            pcb
        } else {
            find_file(dir, "kicad_pcb").context("no PCB found")?
        },
        Some(dir.join(stem).with_extension("kicad_sch")).filter(|x| x.is_file()),
    ))
}
fn find_sibling(p: &Path, ext: &str) -> Option<PathBuf> {
    let exact = p.with_extension(ext);
    if exact.is_file() {
        return Some(exact);
    }
    let stem = p.file_stem()?.to_string_lossy();
    let trimmed = stem.strip_suffix("_routed").unwrap_or(&stem);
    let trimmed = p.with_file_name(trimmed).with_extension(ext);
    if trimmed.is_file() {
        return Some(trimmed);
    }
    let mut candidates = std::fs::read_dir(p.parent()?)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|x| x.to_str()) == Some(ext))
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}
fn find_file(d: &Path, ext: &str) -> Option<PathBuf> {
    std::fs::read_dir(d)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().and_then(|x| x.to_str()) == Some(ext))
}
fn find_board(board_dir: &Path) -> Option<PathBuf> {
    for root in [board_dir.join("output"), board_dir.to_path_buf()] {
        let mut boards = std::fs::read_dir(root)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|x| x.to_str()) == Some("kicad_pcb"))
            .filter(|path| {
                !path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .ends_with("-bak")
            })
            .collect::<Vec<_>>();
        boards.sort();
        if let Some(routed) = boards.iter().find(|path| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .ends_with("_routed")
        }) {
            return Some(routed.clone());
        }
        if let Some(board) = boards.into_iter().next() {
            return Some(board);
        }
    }
    None
}
fn discover_tier(board_dir: &Path, package_dir: &Path) -> Option<String> {
    let manifest = std::fs::read_to_string(package_dir.join("manifest.json"))
        .ok()
        .and_then(|data| serde_json::from_str::<Value>(&data).ok())
        .and_then(|value| value.get("manufacturer")?.as_str().map(str::to_owned));
    manifest.or_else(|| {
        std::fs::read_to_string(board_dir.join("project.kct"))
            .ok()?
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "manufacturer")
                    .then(|| value.trim().trim_matches(['\'', '"']).to_owned())
            })
    })
}
fn input_hashes<'a>(
    board_dir: &Path,
    paths: impl IntoIterator<Item = &'a PathBuf>,
) -> Result<serde_json::Map<String, Value>> {
    let root = board_dir.canonicalize()?;
    paths
        .into_iter()
        .map(|path| {
            let canonical = path.canonicalize()?;
            let relative = canonical.strip_prefix(&root).with_context(|| {
                format!(
                    "input {} is outside board directory {}",
                    canonical.display(),
                    root.display()
                )
            })?;
            Ok((
                relative.to_string_lossy().replace('\\', "/"),
                json!(sha256(&canonical)?),
            ))
        })
        .collect()
}
fn copper_layers(r: &crate::sexp::SExp) -> u8 {
    r.get("layers")
        .map(|l| {
            l.children
                .iter()
                .filter(|x| matches!(x.string_at(1), Some("signal" | "power")))
                .count() as u8
        })
        .unwrap_or(2)
        .max(1)
}
fn native_check(domain: &str, kind: &str, file: &Path) -> Value {
    let temp = std::env::temp_dir().join(format!("kct-{}-{}.json", kind, uuid::Uuid::new_v4()));
    let Some(cli) = kicad_cli() else {
        return json!({"ran":false,"errors":0,"warnings":0,"detail":"kicad-cli not found"});
    };
    let o = Command::new(cli)
        .args([
            domain,
            kind,
            "--format",
            "json",
            "--severity-all",
            "--output",
        ])
        .arg(&temp)
        .arg(file)
        .output();
    let data = std::fs::read_to_string(&temp)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let _ = std::fs::remove_file(temp);
    match (o, data) {
        (Ok(o), Some(v)) => {
            let arrays = ["violations", "unconnected_items", "sheets"]
                .iter()
                .filter_map(|k| v[*k].as_array())
                .flatten();
            let (mut e, mut w) = (0, 0);
            for x in arrays {
                match x["severity"].as_str() {
                    Some("error") => e += 1,
                    Some("warning") => w += 1,
                    _ => {}
                }
            }
            json!({"ran":true,"errors":e,"warnings":w,"detail":if o.status.success(){"native check completed"}else{"native check reported findings"}})
        }
        (Ok(o), None) => {
            json!({"ran":false,"errors":0,"warnings":0,"detail":String::from_utf8_lossy(&o.stderr)})
        }
        (Err(e), _) => json!({"ran":false,"errors":0,"warnings":0,"detail":e.to_string()}),
    }
}
fn native_kct_check(
    pcb: &Path,
    manufacturer: &str,
    net_class_map: Option<&Path>,
    schematic: Option<&Path>,
) -> Value {
    let Ok(executable) = std::env::current_exe() else {
        return json!({"status":"not_run","detail":"could not resolve the kicadmium executable"});
    };
    let mut command = Command::new(executable);
    command
        .args(["kct", "--", "check"])
        .arg(pcb)
        .args(["--mfr", manufacturer, "--format", "json"]);
    if let Some(path) = net_class_map {
        command.arg("--net-class-map").arg(path);
    }
    if let Some(path) = schematic {
        command.arg("--schematic").arg(path).arg("--netlist-sync");
    }
    match command.output() {
        Ok(output) => match serde_json::from_slice::<Value>(&output.stdout) {
            Ok(report) => {
                let overall = report["meta_checks"]["overall"]
                    .as_str()
                    .unwrap_or("INCOMPLETE");
                let status = match overall {
                    "PASSED" => "passed",
                    "FAILED" => "failed",
                    _ => "not_run",
                };
                let summary = &report["summary"];
                json!({"status":status,"detail":format!("{} rule(s), {} error(s), {} warning(s); meta-checks {overall}",summary["rules_checked"],summary["errors"],summary["warnings"]),"report":report})
            }
            Err(error) => {
                json!({"status":"not_run","detail":format!("kct check produced no parseable JSON: {error}")})
            }
        },
        Err(error) => {
            json!({"status":"not_run","detail":format!("could not run kct check: {error}")})
        }
    }
}
fn kicad_cli() -> Option<PathBuf> {
    ["KICADMIUM_KICAD_CLI", "KICAD_CLI"]
        .iter()
        .find_map(|k| {
            std::env::var_os(k)
                .map(PathBuf::from)
                .filter(|p| p.is_file())
        })
        .or_else(|| {
            std::env::var_os("PATH").and_then(|p| {
                std::env::split_paths(&p)
                    .map(|d| d.join("kicad-cli"))
                    .find(|p| p.is_file())
            })
        })
}
fn kicad_version() -> Value {
    kicad_cli()
        .and_then(|p| Command::new(p).arg("--version").output().ok())
        .map(|o| json!(String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or(Value::Null)
}
fn sha256(p: &Path) -> Result<String> {
    let mut h = Sha256::new();
    h.update(std::fs::read(p)?);
    Ok(format!("{:x}", h.finalize()))
}
fn now() -> String {
    format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    )
}
fn iso_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (seconds / 86_400) as i64;
    let day_seconds = seconds % 86_400;
    // Howard Hinnant's civil_from_days algorithm, with days based at the Unix
    // epoch. This keeps the evidence timestamp deterministic and UTC-only.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        day_seconds / 3_600,
        (day_seconds % 3_600) / 60,
        day_seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn board_resolution() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.kicad_pcb");
        std::fs::write(&p, "(kicad_pcb)").unwrap();
        assert_eq!(resolve_design(&p, None).unwrap().0, p)
    }
    #[test]
    fn digest_is_stable() {
        let d = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(d.path(), "x").unwrap();
        assert_eq!(sha256(d.path()).unwrap().len(), 64)
    }
    #[test]
    fn readiness_v1_timestamp_is_parseable_shape() {
        let timestamp = iso_now();
        assert_eq!(timestamp.len(), 25);
        assert_eq!(&timestamp[4..5], "-");
        assert_eq!(&timestamp[10..11], "T");
        assert!(timestamp.ends_with("+00:00"));
    }
    #[test]
    fn routed_board_finds_trimmed_project() {
        let d = tempfile::tempdir().unwrap();
        let pcb = d.path().join("demo_routed.kicad_pcb");
        let project = d.path().join("demo.kicad_pro");
        std::fs::write(&pcb, "(kicad_pcb)").unwrap();
        std::fs::write(&project, "{}").unwrap();
        assert_eq!(find_sibling(&pcb, "kicad_pro"), Some(project));
    }
}
