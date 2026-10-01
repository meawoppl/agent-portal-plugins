//! `kct check`: pure-Rust DRC for KiCad PCBs (port of
//! `kicad_tools.cli.check_cmd` plus the unified-parser shim in
//! `cli/commands/validation.py::run_check_command`).
//!
//! Exit codes: 0 all meta sub-checks PASSED (or `--drc-only`: no errors);
//! 1 command failure; 2 any sub-check FAILED, INCOMPLETE without
//! `--allow-incomplete`, or warnings with `--strict`.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;

use super::Globals;
use crate::analysis::routing_quality::{
    self, compute_routing_quality, evaluate_routing_quality_thresholds, routing_quality_gate_dict,
    RoutingQualityMetrics, FRAGMENT_LENGTH_MM, STAIRCASE_STEP_MM,
};
use crate::jobj;
use crate::manufacturers;
use crate::pyjson::{dumps_indent, py_repr_str, Json};
use crate::router::current_paths::{self, CurrentPathSpec};
use crate::router::rules::NetClassMap;
use crate::schema::pcb::Pcb;
use crate::utils::pyfmt::g;
use crate::validate::checker::{
    category_for_rule, DRCChecker, DRCCheckerOptions, CATEGORY_ADVISORY,
};
use crate::validate::mask_copper::MaskCopperRequest;
use crate::validate::rules::{courtyard_waivers, waivers};
use crate::validate::violations::{DRCResults, DRCViolation};

/// Default `--sch-field-threshold` (upstream `DEFAULT_SCH_FIELD_THRESHOLD_MM`).
pub const DEFAULT_SCH_FIELD_THRESHOLD_MM: f64 = 15.0;

pub const CHECK_CATEGORIES: &[&str] = &[
    "physical_copper_gap",
    "ampacity",
    "path_ampacity",
    "clearance",
    "connectivity",
    "connector_access",
    "segment_zone",
    "via_zone",
    "copper_sliver",
    "courtyard_overlap",
    "dangling_copper",
    "diffpair_clearance_intra",
    "diffpair_length_skew",
    "diffpair_routing_continuity",
    "dimensions",
    "doc_drift",
    "edge",
    "impedance",
    "isolated_copper",
    "match_group_length_skew",
    "netlist",
    "pad_grid",
    "pin1_marker",
    "placement",
    "sch_fields",
    "silkscreen",
    "single_pad_net",
    "solder_mask",
    "mask_to_copper",
    "via_in_pad",
    "via_under_body",
    "width_consistency",
    "zero_length_segment",
    "zones",
];

const MEASUREMENT_RULE_IDS: &[&str] = &[
    "match_group_length_skew",
    "diffpair_length_skew",
    "diffpair_routing_continuity",
];

const NET_RELATIONSHIP_RULE_IDS: &[&str] = &[
    "hole_to_hole_clearance",
    "clearance_segment_via",
    "clearance_segment_segment",
    "clearance_via_via",
    "clearance_pad_via",
];

const ZONE_FILL_DEPENDENT_RULE_IDS: &[&str] = &[
    "clearance_segment_zone",
    "clearance_via_zone",
    "clearance_pad_zone",
];

fn mfr_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = manufacturers::profiles().iter().map(|p| p.id).collect();
    ids.sort();
    ids
}

#[derive(Parser, Debug)]
#[command(about = "Native Rust DRC for PCBs (no kicad-cli required)")]
struct Args {
    /// Path to .kicad_pcb file or directory containing one
    pcb: String,
    #[arg(long = "mask-copper-config")]
    mask_copper_config: Option<PathBuf>,
    #[arg(long = "physical-copper-gap", value_name = "MM")]
    physical_copper_gap: Option<f64>,
    #[arg(long, default_value = "table", value_parser = ["table", "json", "summary"])]
    format: String,
    #[arg(long = "errors-only")]
    errors_only: bool,
    #[arg(long)]
    strict: bool,
    #[arg(long = "strict-connectivity")]
    strict_connectivity: bool,
    #[arg(long = "legacy-connectivity")]
    legacy_connectivity: bool,
    #[arg(long = "refill-zones")]
    refill_zones: bool,
    #[arg(long, short = 'm', value_parser = clap::builder::PossibleValuesParser::new(mfr_ids()))]
    mfr: Option<String>,
    #[arg(long, short = 'l')]
    layers: Option<i64>,
    #[arg(long, short = 'c', value_name = "OZ")]
    copper: Option<String>,
    #[arg(long = "emit-dru")]
    emit_dru: bool,
    #[arg(long = "emit-drc-constraints")]
    emit_drc_constraints: bool,
    #[arg(long = "only")]
    only_checks: Option<String>,
    #[arg(long = "skip")]
    skip_checks: Option<String>,
    #[arg(long, short = 'o')]
    output: Option<String>,
    #[arg(long, short = 'v')]
    verbose: bool,
    #[arg(long = "suppress-library")]
    suppress_library: bool,
    #[arg(long = "drc-only")]
    drc_only: bool,
    #[arg(long = "allow-incomplete")]
    allow_incomplete: bool,
    #[arg(long = "netlist-sync")]
    netlist_sync: bool,
    #[arg(long)]
    schematic: Option<String>,
    #[arg(long = "net-class-map")]
    net_class_map: Option<String>,
    #[arg(long = "no-net-class-map")]
    no_net_class_map: bool,
    #[arg(long = "current-paths")]
    current_paths: Option<String>,
    #[arg(long = "no-current-paths")]
    no_current_paths: bool,
    #[arg(long = "courtyard-waivers")]
    courtyard_waivers: Option<String>,
    #[arg(long)]
    waivers: Option<String>,
    #[arg(long = "pad-grid-strict", conflicts_with = "pad_grid_tolerance")]
    pad_grid_strict: bool,
    #[arg(long = "pad-grid-tolerance", value_name = "MM")]
    pad_grid_tolerance: Option<f64>,
    #[arg(long = "sch-field-threshold", value_name = "MM")]
    sch_field_threshold: Option<f64>,
    #[arg(long = "max-fragment-fraction", value_name = "FRACTION")]
    max_fragment_fraction: Option<f64>,
    #[arg(long = "max-staircase-fraction", value_name = "FRACTION")]
    max_staircase_fraction: Option<f64>,
}

// ------------------------------------------------------------------ helpers

fn is_floating_net(net: &str) -> bool {
    net.is_empty() || net == "net:0"
}

/// `_net_relationship`: same-net / different-net for a 2-net finding.
fn net_relationship(nets: &[String]) -> Option<&'static str> {
    if nets.len() != 2 {
        return None;
    }
    if is_floating_net(&nets[0]) || is_floating_net(&nets[1]) {
        return Some("different-net");
    }
    Some(if nets[0] == nets[1] {
        "same-net"
    } else {
        "different-net"
    })
}

/// `SubCheckResult`.
#[derive(Debug, Clone)]
pub struct SubCheckResult {
    pub status: &'static str,
    pub detail: String,
    pub data: Option<Json>,
}

impl SubCheckResult {
    fn new(status: &'static str, detail: impl Into<String>) -> Self {
        SubCheckResult {
            status,
            detail: detail.into(),
            data: None,
        }
    }

    pub fn to_dict(&self) -> Json {
        let mut out = jobj! {"status" => self.status, "detail" => self.detail.as_str()};
        if let Some(Json::Obj(items)) = &self.data {
            for (k, v) in items {
                out.set(k, v.clone());
            }
        }
        out
    }
}

/// `MetaCheckResult`.
#[derive(Debug, Clone)]
pub struct MetaCheckResult {
    pub drc: SubCheckResult,
    pub erc: SubCheckResult,
    pub lvs: SubCheckResult,
    pub manifest: SubCheckResult,
    pub overall: &'static str,
    pub schematic_missing: bool,
}

impl MetaCheckResult {
    fn compute_overall(&mut self) {
        let subs = [&self.drc, &self.erc, &self.lvs, &self.manifest];
        self.overall = if subs.iter().any(|s| s.status == "FAILED") {
            "FAILED"
        } else if subs.iter().any(|s| s.status == "NOT RUN") {
            "INCOMPLETE"
        } else {
            "PASSED"
        };
    }

    pub fn to_dict(&self) -> Json {
        jobj! {
            "drc" => self.drc.to_dict(),
            "erc" => self.erc.to_dict(),
            "lvs" => self.lvs.to_dict(),
            "manifest" => self.manifest.to_dict(),
            "overall" => self.overall,
            "schematic_missing" => self.schematic_missing,
        }
    }
}

/// Python `str(Path(...).resolve())`.
fn resolve(p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    let abs = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    std::fs::canonicalize(&abs).unwrap_or_else(|_| normalize_lexically(&abs))
}

fn normalize_lexically(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `_find_pcb_file`: recursive glob, skipping `_routed` / `-bak` boards.
fn find_pcb_file(dir: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().and_then(|e| e.to_str()) == Some("kicad_pcb") {
                out.push(p);
            }
        }
    }
    let mut all = Vec::new();
    walk(dir, &mut all);
    all.into_iter().find(|f| {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
        !name.ends_with("_routed.kicad_pcb") && !name.ends_with("-bak.kicad_pcb")
    })
}

fn parent(p: &Path) -> PathBuf {
    p.parent().map(Path::to_path_buf).unwrap_or_default()
}

fn discover_fab_profile_sidecar(pcb_path: &Path) -> Option<PathBuf> {
    let dir = parent(pcb_path);
    [
        dir.join("fab_profile.json"),
        dir.join("output").join("fab_profile.json"),
        parent(&dir).join("output").join("fab_profile.json"),
    ]
    .into_iter()
    .find(|c| c.is_file())
}

/// `_resolve_effective_check_mfr` -> `(mfr, messages, source)`.
pub fn resolve_effective_check_mfr(
    cli_mfr: Option<&str>,
    pcb_path: &Path,
    default: &str,
) -> (String, Vec<String>, &'static str) {
    let mut messages = Vec::new();
    if let Some(m) = cli_mfr {
        return (m.to_string(), messages, "cli");
    }
    let valid = mfr_ids();
    if let Some(sidecar) = discover_fab_profile_sidecar(pcb_path) {
        match std::fs::read_to_string(&sidecar)
            .map_err(|e| e.to_string())
            .and_then(|t| crate::pyjson::loads(&t).map_err(|e| e.to_string()))
        {
            Err(e) => messages.push(format!(
                "WARNING: ignoring malformed fab-profile sidecar {}: {e}",
                sidecar.display()
            )),
            Ok(data) => {
                let mfr = data.get("mfr").filter(|v| v.truthy());
                match mfr {
                    None => messages.push(format!(
                        "WARNING: ignoring fab-profile sidecar {}: no 'mfr' field",
                        sidecar.display()
                    )),
                    Some(v) => {
                        let s = v.as_str().unwrap_or("");
                        if !valid.contains(&s) {
                            messages.push(format!(
                                "WARNING: ignoring fab-profile sidecar {}: unknown profile {}",
                                sidecar.display(),
                                v.py_repr()
                            ));
                        } else {
                            messages.push(format!(
                                "[INFO] auto-loaded fab profile: {s} (from {})",
                                sidecar.display()
                            ));
                            return (s.to_string(), messages, "sidecar");
                        }
                    }
                }
            }
        }
    }
    if let Some(target) = crate::sync::discover::resolve_target_fab_for_pcb(pcb_path) {
        if !valid.contains(&target.as_str()) {
            messages.push(format!(
                "WARNING: ignoring project.kct target_fab {}: unknown profile",
                py_repr_str(&target)
            ));
        } else {
            messages.push(format!(
                "[INFO] auto-loaded fab profile: {target} (from project.kct target_fab)"
            ));
            return (target, messages, "project_kct");
        }
    }
    (default.to_string(), messages, "default")
}

fn profile_supports_via_in_pad(mfr: &str) -> bool {
    manufacturers::rules(mfr, 4, 1.0)
        .map(|r| r.via_in_pad_supported)
        .unwrap_or(false)
}

fn maybe_emit_via_in_pad_tier_advisory(mfr: &str, violations: &[DRCViolation], source: &str) {
    let Ok(rules) = manufacturers::rules(mfr, 4, 1.0) else {
        return;
    };
    if rules.via_in_pad_supported {
        return;
    }
    let vip = violations
        .iter()
        .filter(|v| v.rule_id == "via_in_pad")
        .count();
    if vip == 0 {
        return;
    }
    let Some(permitting) = mfr_ids()
        .into_iter()
        .find(|id| profile_supports_via_in_pad(id))
    else {
        return;
    };
    let m = py_repr_str(mfr);
    let tail = match source {
        "project_kct" => format!("Board declares {m} (project.kct target_fab)."),
        "sidecar" => format!("Board declares {m} (fab_profile.json sidecar)."),
        "cli" => format!("Profile {m} was passed explicitly via --mfr."),
        _ => "Defaulting to the base tier.".to_string(),
    };
    eprintln!(
        "WARNING: {vip} via_in_pad finding(s) at profile {m}. Via-in-pad is a tier-gated \
         capability: it is LEGAL at {permitting} (via_in_pad_supported). If this board targets \
         a higher fab tier, re-run with --mfr {permitting} (or pass the intended --mfr). {tail}"
    );
}

fn maybe_emit_via_in_pad_process_advisory(violations: &[DRCViolation]) {
    let missing = violations
        .iter()
        .filter(|v| v.rule_id == "via_in_pad_process_missing")
        .count();
    let ineligible = violations
        .iter()
        .filter(|v| v.rule_id == "via_in_pad_process_ineligible")
        .count();
    if missing > 0 {
        eprintln!(
            "WARNING: {missing} via_in_pad_process_missing finding(s). The active manufacturer \
             tier supports via-in-pad in its general catalog, but no eligible fabrication \
             process is declared for this board's layer/copper configuration \
             (DesignRules.via_in_pad_process_id is unset or unrecognized). See \
             kicad_tools.manufacturers.fabrication_process for the known process catalog, or \
             move the via off the pad."
        );
    }
    if ineligible > 0 {
        eprintln!(
            "WARNING: {ineligible} via_in_pad_process_ineligible finding(s). A fabrication \
             process IS declared, but this via's drill/annular-ring geometry, the board's layer \
             count, or its distance to another component's drilled hole does not meet that \
             process's published requirements. See the violation message for the specific \
             reason(s), or move the via off the pad."
        );
    }
}

fn warn_stale_zone_fills(violations: &[DRCViolation], pcb_path: &Path) {
    let present: BTreeSet<&str> = violations
        .iter()
        .map(|v| v.rule_id.as_str())
        .filter(|r| ZONE_FILL_DEPENDENT_RULE_IDS.contains(r))
        .collect();
    if present.is_empty() {
        return;
    }
    eprintln!(
        "WARNING: zone-clearance findings present ({}) — kct check measures these against the \
         committed zone fills in the .kicad_pcb, which may be STALE if the board was routed or \
         refilled after the fills were last saved. Cross-gate / fix with:\n    kicad-cli pcb drc \
         --refill-zones --save-board {}\nthen re-run kct check, or pass --refill-zones to do \
         this automatically (issue #4096).",
        present.into_iter().collect::<Vec<_>>().join(", "),
        pcb_path.display()
    );
}

/// `run_refill_zones` (simplified: no net-table restoration).
fn refill_zones_in_place(pcb_path: &Path) {
    let result = (|| -> std::result::Result<(), String> {
        let cli = super::runner::find_kicad_cli().ok_or_else(|| {
            "kicad-cli not found. Install KiCad 8 from https://www.kicad.org/download/".to_string()
        })?;
        let report = std::env::temp_dir().join(format!("refill_drc_{}.json", std::process::id()));
        let supports = std::process::Command::new(&cli)
            .args(["pcb", "drc", "--help"])
            .output()
            .map(|o| {
                let t = String::from_utf8_lossy(&o.stdout).into_owned()
                    + &String::from_utf8_lossy(&o.stderr);
                t.contains("--refill-zones") && t.contains("--save-board")
            })
            .unwrap_or(false);
        let mut cmd = std::process::Command::new(&cli);
        cmd.args(["pcb", "drc", "--output"])
            .arg(&report)
            .args(["--format", "json"]);
        if supports {
            cmd.args(["--refill-zones", "--save-board"]);
        }
        cmd.arg(pcb_path);
        let out = cmd
            .output()
            .map_err(|e| format!("kicad-cli not found: {e}"))?;
        let ok = std::fs::metadata(&report).is_ok_and(|m| m.len() > 0);
        let _ = std::fs::remove_file(&report);
        if ok {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            Err(if stderr.is_empty() {
                "Zone refill via DRC failed — no report produced".into()
            } else {
                stderr
            })
        }
    })();
    match result {
        Ok(()) => eprintln!(
            "[INFO] refilled zones in place via kicad-cli: {}",
            pcb_path.display()
        ),
        Err(e) => {
            let e = e.trim().to_string();
            eprintln!(
                "WARNING: --refill-zones requested but the refill did not run ({}); continuing \
                 against the stored (possibly stale) zone fills.  Install KiCad 8+ so kicad-cli \
                 is on PATH to enable the pre-check refill (issue #4096).",
                if e.is_empty() { "unknown error" } else { &e }
            )
        }
    }
}

// ------------------------------------------------------------ meta checks

fn erc_subcheck(sch_path: Option<&Path>, strict: bool) -> SubCheckResult {
    let Some(sch) = sch_path else {
        return SubCheckResult::new("NOT RUN", "no schematic discovered next to PCB");
    };
    if super::runner::find_kicad_cli().is_none() {
        return SubCheckResult::new(
            "NOT RUN",
            "kicad-cli not found in PATH; install KiCad 8+ to enable ERC",
        );
    }
    let res = super::runner::run_erc(sch, None, "json", false, None);
    let Some(out) = res.output_path.filter(|_| res.success) else {
        let last = res
            .stderr
            .trim()
            .lines()
            .last()
            .map(str::to_string)
            .filter(|_| !res.stderr.is_empty())
            .unwrap_or_else(|| "unknown error".into());
        return SubCheckResult::new("FAILED", format!("kicad-cli ERC failed: {last}"));
    };
    let report = match crate::erc::report::ERCReport::load(&out) {
        Ok(r) => r,
        Err(e) => return SubCheckResult::new("FAILED", format!("failed to parse ERC report: {e}")),
    };
    let (e, w) = (report.error_count(), report.warning_count());
    let detail = format!("{e} error(s), {w} warning(s)");
    if e > 0 {
        return SubCheckResult::new("FAILED", detail);
    }
    if strict && w > 0 {
        return SubCheckResult::new("FAILED", detail + " (strict)");
    }
    SubCheckResult::new("PASSED", detail)
}

fn lvs_subcheck(sch_path: Option<&Path>, pcb_path: &Path) -> SubCheckResult {
    let Some(sch) = sch_path else {
        return SubCheckResult::new("NOT RUN", "no schematic discovered; cannot compare");
    };
    crate::lvs::lvs_subcheck(sch, pcb_path)
        .map(|(status, detail, data)| SubCheckResult {
            status,
            detail,
            data,
        })
        .unwrap_or_else(|e| SubCheckResult::new("FAILED", e))
}

fn manifest_subcheck(pcb_path: &Path) -> SubCheckResult {
    let candidates = [
        parent(pcb_path).join("manufacturing").join("manifest.json"),
        parent(&parent(pcb_path))
            .join("manufacturing")
            .join("manifest.json"),
    ];
    let Some(manifest_path) = candidates.into_iter().find(|c| c.exists()) else {
        return SubCheckResult::new("NOT RUN", "no manufacturing bundle; run `kct export` first");
    };
    match crate::validate::manifest::verify_bundle(&manifest_path, pcb_path) {
        Ok(()) => SubCheckResult::new(
            "PASSED",
            "bundle SHA-256 hashes verified; archived PCB matches current content",
        ),
        Err(e) => SubCheckResult::new("FAILED", format!("STALE/unverified: {e}")),
    }
}

fn resolve_check_schematic(schematic: Option<&str>, pcb_path: &Path) -> Option<PathBuf> {
    match schematic {
        Some(s) => {
            let c = resolve(s);
            c.exists().then_some(c)
        }
        None => crate::sync::discover::resolve_schematic_for_pcb(pcb_path),
    }
}

fn run_meta_checks(
    pcb_path: &Path,
    drc: SubCheckResult,
    schematic: Option<&str>,
    strict: bool,
) -> MetaCheckResult {
    let sch = resolve_check_schematic(schematic, pcb_path);
    let erc = erc_subcheck(sch.as_deref(), strict);
    let lvs = lvs_subcheck(sch.as_deref(), pcb_path);
    let manifest = manifest_subcheck(pcb_path);
    let schematic_missing = sch.is_none();
    if schematic_missing {
        eprintln!(
            "WARNING: no schematic discovered next to {}; ERC and the LVS manufacturing hard \
             gate were SKIPPED (not run) -- copper was NOT compared to any schematic. Pass \
             --schematic <path.kicad_sch> to run them.",
            pcb_path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        );
    }
    let mut r = MetaCheckResult {
        drc,
        erc,
        lvs,
        manifest,
        overall: "PASSED",
        schematic_missing,
    };
    r.compute_overall();
    r
}

fn format_meta_status_line(name: &str, sub: &SubCheckResult) -> String {
    let mut status = sub.status.to_string();
    let mut detail = sub.detail.as_str();
    if name == "Manifest" && sub.status == "FAILED" && detail.starts_with("STALE:") {
        status = "STALE".into();
        detail = &detail["STALE: ".len().min(detail.len())..];
    }
    format!("{:10} {status:8} ({detail})", format!("{name}:"))
}

fn print_lvs_verbose_expansion(sub: &SubCheckResult) {
    let Some(data) = &sub.data else { return };
    let arr = |k: &str| data.get(k).and_then(Json::as_array).unwrap_or(&[]).to_vec();
    let copper = arr("copper_mismatches");
    let label = arr("mismatches");
    let s = |m: &Json, k: &str| match m.get(k) {
        Some(Json::Str(s)) => s.clone(),
        Some(v) => v.py_repr(),
        None => String::new(),
    };
    if !copper.is_empty() {
        println!("  copper: {} mismatch(es)", copper.len());
        for m in &copper {
            println!(
                "    {} {}({})/{}({})",
                s(m, "kind"),
                s(m, "pad_a"),
                s(m, "net_a"),
                s(m, "pad_b"),
                s(m, "net_b")
            );
        }
    }
    if !label.is_empty() {
        println!("  label: {} mismatch(es)", label.len());
        for m in &label {
            println!(
                "    {}.{} sch={} pcb={}",
                s(m, "ref"),
                s(m, "pad"),
                m.get("schematic_net")
                    .map(Json::py_repr)
                    .unwrap_or_default(),
                m.get("pcb_net").map(Json::py_repr).unwrap_or_default()
            );
        }
    }
}

fn print_meta_check_stanza(r: &MetaCheckResult, verbose: bool) {
    println!();
    println!("{}", format_meta_status_line("DRC", &r.drc));
    println!("{}", format_meta_status_line("ERC", &r.erc));
    println!("{}", format_meta_status_line("LVS", &r.lvs));
    if verbose {
        print_lvs_verbose_expansion(&r.lvs);
    }
    println!("{}", format_meta_status_line("Manifest", &r.manifest));
    println!("{:10} {}", "Overall:", r.overall);
}

fn print_routing_quality_stanza(m: &RoutingQualityMetrics) {
    let base = m.total_segments - m.zero_length_count;
    let pct = |c: i64| {
        if base > 0 {
            format!("{:.1}%", 100.0 * c as f64 / base as f64)
        } else {
            "0.0%".into()
        }
    };
    println!();
    println!("Routing quality (advisory):");
    println!(
        "  Segments:     {} across {} net(s) ({:.1}/net)",
        m.total_segments, m.nets_with_copper, m.segments_per_net
    );
    println!("  Median length: {:.3} mm", m.median_length_mm);
    println!(
        "  Fragments (<{FRAGMENT_LENGTH_MM} mm): {} ({})",
        m.fragment_count,
        pct(m.fragment_count)
    );
    println!(
        "  Direction:    orthogonal {} ({}) / 45-deg {} ({}) / off-axis {} ({})",
        m.orthogonal_count,
        pct(m.orthogonal_count),
        m.diagonal_45_count,
        pct(m.diagonal_45_count),
        m.off_axis_count,
        pct(m.off_axis_count)
    );
    println!(
        "  Staircase steps (<{STAIRCASE_STEP_MM} mm legs): {} ({})",
        m.staircase_step_count,
        pct(m.staircase_step_count)
    );
    println!("  Zero-length:  {}", m.zero_length_count);
}

fn warn_unevaluated_ampacity(
    declared: &[(String, f64)],
    resolved: &[(String, String)],
    pcb: &Pcb,
    only_set: Option<&BTreeSet<String>>,
    skip_set: &BTreeSet<String>,
) {
    let active = only_set.is_none_or(|o| o.contains("ampacity")) && !skip_set.contains("ampacity");
    let seg_nets: BTreeSet<&str> = pcb.segments().iter().map(|s| s.net_name.as_str()).collect();
    let mut keys: Vec<&str> = declared.iter().map(|(k, _)| k.as_str()).collect();
    keys.sort();
    keys.dedup();
    let mut out: Vec<String> = Vec::new();
    for key in keys {
        // `{user_key: board_net}` inversion keeps the LAST board net per key.
        let board = resolved
            .iter()
            .rev()
            .find(|(_, u)| u == key)
            .map(|(b, _)| b);
        match board {
            None => out.push(key.to_string()),
            Some(b) if !active || !seg_nets.contains(b.as_str()) => out.push(b.clone()),
            _ => {}
        }
    }
    let mut seen = BTreeSet::new();
    out.retain(|x| seen.insert(x.clone()));
    if out.is_empty() {
        return;
    }
    eprintln!(
        "WARNING: ampacity rule declared but not evaluated for net(s): {}. A declared \
         target_ampacity that matched zero routed segments is reported as 0 ampacity errors but \
         was never actually checked (post-resolution key miss, unrouted net, or the ampacity \
         category was excluded via --skip/--only).",
        out.join(", ")
    );
}

/// `_parse_copper_weight_arg`.
pub fn parse_copper_weight_arg(
    raw: &str,
) -> std::result::Result<(Option<f64>, Option<f64>), String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("--copper value is empty".into());
    }
    let pyfloat = |s: &str| -> Option<f64> {
        let t = s.trim();
        match t.to_ascii_lowercase().as_str() {
            "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
            "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
            "nan" | "+nan" | "-nan" => Some(f64::NAN),
            _ => t.replace('_', "").parse().ok(),
        }
    };
    if !text.contains('=') {
        let Some(oz) = pyfloat(text) else {
            return Err(format!(
                "invalid --copper value {} (expected a number like '2' or a keyed form \
                 'outer=2,inner=0.5')",
                py_repr_str(raw)
            ));
        };
        if oz <= 0.0 {
            return Err(format!(
                "--copper weight must be positive: {}",
                crate::pyjson::py_float_repr(oz)
            ));
        }
        return Ok((Some(oz), Some(oz)));
    }
    let (mut outer, mut inner) = (None, None);
    let mut seen = BTreeSet::new();
    for token in text.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let Some((key, value)) = token.split_once('=') else {
            return Err(format!(
                "invalid --copper token {} (expected 'key=value')",
                py_repr_str(token)
            ));
        };
        let key = key.trim().to_lowercase();
        let value = value.trim();
        if key != "outer" && key != "inner" {
            return Err(format!(
                "unknown --copper key {} (expected 'outer' or 'inner')",
                py_repr_str(&key)
            ));
        }
        if !seen.insert(key.clone()) {
            return Err(format!("duplicate --copper key {}", py_repr_str(&key)));
        }
        let Some(oz) = pyfloat(value) else {
            return Err(format!(
                "invalid --copper {key} value {} (expected a number)",
                py_repr_str(value)
            ));
        };
        if oz <= 0.0 {
            return Err(format!(
                "--copper {key} weight must be positive: {}",
                crate::pyjson::py_float_repr(oz)
            ));
        }
        if key == "outer" {
            outer = Some(oz);
        } else {
            inner = Some(oz);
        }
    }
    if outer.is_none() && inner.is_none() {
        return Err(format!(
            "--copper keyed form set no values: {}",
            py_repr_str(raw)
        ));
    }
    Ok((outer, inner))
}

// --------------------------------------------------------------- dispatch

/// Options for [`run_selected_checks`] beyond the checker itself.
pub struct SelectedChecksOptions<'a> {
    pub pad_grid_threshold: Option<f64>,
    pub pad_grid_auto_derive: bool,
    pub sch_path: Option<&'a Path>,
    pub sch_field_threshold: f64,
    pub pcb_path: Option<&'a Path>,
}

/// Category -> checker method (upstream `check_methods` dict order).
const CHECK_METHODS: &[(&str, &str)] = &[
    ("ampacity", "check_ampacity"),
    ("path_ampacity", "check_path_ampacity"),
    ("clearance", "check_clearances"),
    ("physical_copper_gap", "check_physical_copper_gap"),
    ("connectivity", "check_connectivity"),
    ("connector_access", "check_connector_access"),
    ("segment_zone", "check_segment_zone_clearances"),
    ("via_zone", "check_via_zone_clearances"),
    ("copper_sliver", "check_copper_slivers"),
    ("courtyard_overlap", "check_courtyard_overlap"),
    ("dangling_copper", "check_dangling_copper"),
    ("diffpair_clearance_intra", "check_diffpair_clearance_intra"),
    ("diffpair_length_skew", "check_diffpair_length_skew"),
    (
        "diffpair_routing_continuity",
        "check_diffpair_routing_continuity",
    ),
    ("dimensions", "check_dimensions"),
    ("doc_drift", "<doc_drift>"),
    ("edge", "check_edge_clearances"),
    ("impedance", "check_impedance"),
    ("isolated_copper", "check_isolated_copper"),
    ("match_group_length_skew", "check_match_group_length_skew"),
    ("netlist", "check_netlist"),
    ("pad_grid", "<pad_grid>"),
    ("pin1_marker", "check_pin1_markers"),
    ("placement", "check_footprint_placement"),
    ("sch_fields", "<sch_fields>"),
    ("silkscreen", "check_silkscreen"),
    ("single_pad_net", "check_single_pad_nets"),
    ("solder_mask", "check_solder_mask_pads"),
    ("mask_to_copper", "check_mask_to_copper"),
    ("via_in_pad", "check_via_in_pad"),
    ("via_under_body", "check_via_under_body"),
    ("width_consistency", "check_width_consistency"),
    ("zero_length_segment", "check_zero_length_segments"),
    ("zones", "check_zones"),
];

/// `run_selected_checks`.
pub fn run_selected_checks(
    checker: &DRCChecker,
    only_set: Option<&BTreeSet<String>>,
    skip_set: &BTreeSet<String>,
    opts: &SelectedChecksOptions,
) -> DRCResults {
    let mut results = DRCResults::new();
    for (category, method) in CHECK_METHODS {
        if *category == "mask_to_copper"
            && only_set.is_none()
            && checker.mask_copper_request.is_none()
        {
            continue;
        }
        if *category == "width_consistency" && only_set.is_none() {
            continue;
        }
        if only_set.is_some_and(|o| !o.contains(*category)) {
            continue;
        }
        if skip_set.contains(*category) {
            continue;
        }
        let r = match *method {
            "<pad_grid>" => checker.check_pad_grid_alignment(
                0.1,
                opts.pad_grid_threshold,
                opts.pad_grid_auto_derive,
                !checker.verbose,
            ),
            "<sch_fields>" => match opts.sch_path {
                None => DRCResults::new(),
                Some(p) => crate::validate::rules::schematic_fields::check_schematic_fields(
                    p,
                    opts.sch_field_threshold,
                ),
            },
            "<doc_drift>" => match opts.pcb_path {
                None => DRCResults::new(),
                Some(p) => crate::validate::doc_drift::check_doc_drift(p),
            },
            m => checker.run_named(m),
        };
        results.merge(r);
    }
    results
}

// ----------------------------------------------------------------- output

fn print_measurement_summary(violations: &[DRCViolation]) {
    let mut rows: Vec<&DRCViolation> = violations
        .iter()
        .filter(|v| MEASUREMENT_RULE_IDS.contains(&v.rule_id.as_str()) && v.actual_value.is_some())
        .collect();
    if rows.is_empty() {
        return;
    }
    let subject = |v: &DRCViolation| -> String {
        if let Some(i) = v.items.first() {
            return i.clone();
        }
        if !v.nets.is_empty() {
            return v
                .nets
                .iter()
                .filter(|n| !n.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("/");
        }
        v.rule_id.clone()
    };
    rows.sort_by(|a, b| (a.rule_id.as_str(), subject(a)).cmp(&(b.rule_id.as_str(), subject(b))));
    let table: Vec<(String, &str, String, String, &str)> = rows
        .iter()
        .map(|v| {
            (
                subject(v),
                if v.rule_id == "diffpair_routing_continuity" {
                    "continuity"
                } else {
                    "skew"
                },
                v.actual_value.map_or("-".into(), |x| format!("{x:.3}")),
                v.required_value.map_or("-".into(), |x| format!("{x:.3}")),
                if v.is_error() { "FAIL" } else { "pass" },
            )
        })
        .collect();
    let sw = table
        .iter()
        .map(|r| r.0.chars().count())
        .max()
        .unwrap_or(0)
        .max("Group/Pair".len());
    let mw = table
        .iter()
        .map(|r| r.1.len())
        .max()
        .unwrap_or(0)
        .max("Metric".len());
    println!("\n{}", "-".repeat(60));
    println!("MEASUREMENT SUMMARY (length-match / continuity):");
    println!(
        "  {:<sw$}  {:<mw$}  {:>10}  {:>10}  Status",
        "Group/Pair", "Metric", "Measured", "Tolerance"
    );
    for (s, m, me, t, st) in table {
        println!("  {s:<sw$}  {m:<mw$}  {me:>10}  {t:>10}  {st}");
    }
}

#[derive(Default)]
struct CategoryTally {
    errors: usize,
    warnings: usize,
    infos: usize,
    waived: usize,
    rules: BTreeSet<String>,
}

impl CategoryTally {
    fn add(&mut self, v: &DRCViolation) {
        self.rules.insert(v.rule_id.clone());
        if v.is_waived() {
            self.waived += 1;
        } else if v.is_error() {
            self.errors += 1;
        } else if v.is_info() {
            self.infos += 1;
        } else {
            self.warnings += 1;
        }
    }
    fn total(&self) -> usize {
        self.errors + self.warnings + self.infos
    }
    fn detail(&self) -> String {
        let mut parts = vec![
            format!("{} errors", self.errors),
            format!("{} warnings", self.warnings),
            format!("{} infos", self.infos),
        ];
        if self.waived > 0 {
            parts.push(format!("{} waived", self.waived));
        }
        parts.join(", ")
    }
    fn rule_list(&self) -> String {
        if self.rules.is_empty() {
            "none".into()
        } else {
            self.rules.iter().cloned().collect::<Vec<_>>().join(", ")
        }
    }
}

fn print_category_summary(violations: &[DRCViolation]) {
    let mut mfg = CategoryTally::default();
    let mut adv = CategoryTally::default();
    for v in violations {
        if category_for_rule(&v.rule_id) == CATEGORY_ADVISORY {
            adv.add(v);
        } else {
            mfg.add(v);
        }
    }
    println!("\n{}", "-".repeat(60));
    println!("CATEGORY SUMMARY (fabrication-blocking vs advisory -- Issue #3803):");
    println!(
        "  Manufacturing DRC: {} blocking  (copper/clearance/hole/edge/mask/drill)",
        mfg.errors
    );
    println!("      {}  [{}]", mfg.detail(), mfg.rule_list());
    if adv.total() > 0 {
        println!(
            "  Advisory/quality:  {} advisory  (connectivity, diff-pair, copper_sliver, ampacity, silk)",
            adv.total()
        );
        println!("      {}  [{}]", adv.detail(), adv.rule_list());
    }
}

fn print_violation(v: &DRCViolation, verbose: bool, indent: &str) {
    let symbol = if v.is_error() {
        "X"
    } else if v.is_info() {
        "i"
    } else {
        "!"
    };
    let rel = if NET_RELATIONSHIP_RULE_IDS.contains(&v.rule_id.as_str()) {
        net_relationship(&v.nets)
    } else {
        None
    };
    let header = match rel {
        None => v.rule_id.clone(),
        Some(r) => format!("{} ({r})", v.rule_id),
    };
    println!("\n{indent}[{symbol}] {header}");
    println!("{indent}    {}", v.message);
    let labels = || -> Vec<String> {
        v.nets
            .iter()
            .map(|n| {
                if n.is_empty() {
                    "<no net>".into()
                } else {
                    n.clone()
                }
            })
            .collect()
    };
    let mut net_shown = false;
    if rel.is_some() && !v.nets.is_empty() {
        println!("{indent}    Nets: {}", labels().join(" / "));
        net_shown = true;
    }
    if verbose {
        if let Some((x, y)) = v.location {
            println!("{indent}    -> ({x:.2}, {y:.2}) mm");
        }
        if let Some(l) = v.layer.as_deref().filter(|l| !l.is_empty()) {
            println!("{indent}    Layer: {l}");
        }
        if let (Some(a), Some(r)) = (v.actual_value, v.required_value) {
            println!("{indent}    Actual: {a:.3}mm, Required: {r:.3}mm");
        }
        if !v.items.is_empty() {
            println!("{indent}    Items: {}", v.items.join(", "));
        }
        if !v.nets.is_empty() && !net_shown {
            println!("{indent}    Nets: {}", labels().join(", "));
        }
    }
}

fn mask_all_passed(results: &DRCResults) -> bool {
    results.mask_copper_assessments.iter().all(|a| a.passed())
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn output_table(
    violations: &[DRCViolation],
    results: &DRCResults,
    pcb_path: &Path,
    mfr: &str,
    layers: i64,
    verbose: bool,
) {
    let count = |f: fn(&DRCViolation) -> bool| violations.iter().filter(|v| f(v)).count();
    let (e, w, i, wv) = (
        count(DRCViolation::is_error),
        count(DRCViolation::is_warning),
        count(DRCViolation::is_info),
        count(DRCViolation::is_waived),
    );
    let bar = "=".repeat(60);
    let dash = "-".repeat(60);
    println!("\n{bar}");
    println!("NATIVE RUST DRC CHECK");
    println!("{bar}");
    println!("File: {}", file_name(pcb_path));
    println!("Manufacturer: {}", mfr.to_uppercase());
    println!("Layers: {layers}");
    println!("Rules checked: {}", results.rules_checked);
    println!("\nResults:");
    println!("  Errors:     {e}");
    println!("  Warnings:   {w}");
    if i > 0 {
        println!("  Infos:      {i}");
    }
    if wv > 0 {
        println!("  Waived:     {wv}");
    }
    if results.suppressed_count > 0 {
        println!(
            "  Suppressed: {} (standard library footprints)",
            results.suppressed_count
        );
    }
    let qualified = mask_all_passed(results);
    if violations.is_empty() {
        println!("\n{bar}");
        println!(
            "{}",
            if qualified {
                "DRC PASSED - No violations found"
            } else {
                "DRC NOT QUALIFIED - Requested mask coverage incomplete or failing"
            }
        );
        return;
    }
    print_category_summary(violations);
    print_measurement_summary(violations);

    let by_rule_source: Vec<&DRCViolation> = violations
        .iter()
        .filter(|v| verbose || !(v.is_info() && MEASUREMENT_RULE_IDS.contains(&v.rule_id.as_str())))
        .collect();
    // rule -> [errors, warnings, infos, waived], insertion ordered.
    let mut by_rule: Vec<(String, [usize; 4])> = Vec::new();
    let mut rel_counts: Vec<(String, [usize; 2])> = Vec::new();
    for v in &by_rule_source {
        let idx = match by_rule.iter().position(|(r, _)| *r == v.rule_id) {
            Some(i) => i,
            None => {
                by_rule.push((v.rule_id.clone(), [0; 4]));
                by_rule.len() - 1
            }
        };
        let slot = if v.is_waived() {
            3
        } else if v.is_error() {
            0
        } else if v.is_info() {
            2
        } else {
            1
        };
        by_rule[idx].1[slot] += 1;
        if NET_RELATIONSHIP_RULE_IDS.contains(&v.rule_id.as_str()) {
            if let Some(r) = net_relationship(&v.nets) {
                let j = match rel_counts.iter().position(|(k, _)| *k == v.rule_id) {
                    Some(j) => j,
                    None => {
                        rel_counts.push((v.rule_id.clone(), [0; 2]));
                        rel_counts.len() - 1
                    }
                };
                rel_counts[j].1[if r == "same-net" { 0 } else { 1 }] += 1;
            }
        }
    }
    if !by_rule.is_empty() {
        println!("\n{dash}");
        println!("BY RULE:");
    }
    let mut sorted = by_rule.clone();
    // Python's sort is stable; key = -(total).
    sorted.sort_by_key(|(_, c)| std::cmp::Reverse(c.iter().sum::<usize>()));
    for (rule, c) in &sorted {
        let plural = |n: usize, word: &str| format!("{n} {word}{}", if n != 1 { "s" } else { "" });
        let mut parts = Vec::new();
        if c[0] > 0 {
            parts.push(plural(c[0], "error"));
        }
        if c[1] > 0 {
            parts.push(plural(c[1], "warning"));
        }
        if c[2] > 0 {
            parts.push(plural(c[2], "info"));
        }
        if c[3] > 0 {
            parts.push(format!("{} waived", c[3]));
        }
        let mut line = format!("  {rule}: {}", parts.join(", "));
        if let Some((_, rc)) = rel_counts.iter().find(|(k, _)| k == rule) {
            line += &format!(" ({} different-net, {} same-net)", rc[1], rc[0]);
        }
        println!("{line}");
    }

    let errors: Vec<&DRCViolation> = violations.iter().filter(|v| v.is_error()).collect();
    let warnings: Vec<&DRCViolation> = violations.iter().filter(|v| v.is_warning()).collect();
    let infos: Vec<&DRCViolation> = violations.iter().filter(|v| v.is_info()).collect();
    let waived: Vec<&DRCViolation> = violations.iter().filter(|v| v.is_waived()).collect();
    if !errors.is_empty() {
        println!("\n{dash}");
        println!("ERRORS (must fix):");
        for v in &errors {
            print_violation(v, verbose, "  ");
        }
    }
    if !warnings.is_empty() {
        println!("\n{dash}");
        println!("WARNINGS (review recommended):");
        let shown = if verbose {
            warnings.len()
        } else {
            warnings.len().min(10)
        };
        for v in &warnings[..shown] {
            print_violation(v, verbose, "  ");
        }
        if warnings.len() > 10 && !verbose {
            println!(
                "\n  ... and {} more warnings (use --verbose)",
                warnings.len() - 10
            );
        }
    }
    let info_src: Vec<&&DRCViolation> = infos
        .iter()
        .filter(|v| verbose || !MEASUREMENT_RULE_IDS.contains(&v.rule_id.as_str()))
        .collect();
    if !info_src.is_empty() {
        println!("\n{dash}");
        println!("INFOS (advisory only):");
        let shown = if verbose {
            info_src.len()
        } else {
            info_src.len().min(10)
        };
        for v in &info_src[..shown] {
            print_violation(v, verbose, "  ");
        }
        if info_src.len() > 10 && !verbose {
            println!(
                "\n  ... and {} more infos (use --verbose)",
                info_src.len() - 10
            );
        }
    }
    if !waived.is_empty() {
        println!("\n{dash}");
        println!("WAIVED (documented exceptions, non-blocking):");
        for v in &waived {
            println!("\n  [W] {}", v.rule_id);
            println!("      {}", v.message);
            if let Some(issue) = v.waiver_issue.as_deref().filter(|s| !s.is_empty()) {
                println!("      Waiver issue: {issue}");
            }
            if verbose {
                if !v.items.is_empty() {
                    println!("      Items: {}", v.items.join(", "));
                }
                if let Some(l) = v.layer.as_deref().filter(|l| !l.is_empty()) {
                    println!("      Layer: {l}");
                }
            }
        }
    }
    println!("\n{bar}");
    if !errors.is_empty() {
        println!("DRC FAILED - Fix errors before manufacturing");
    } else if !warnings.is_empty() {
        println!("DRC WARNING - Review warnings");
    } else {
        println!(
            "{}",
            if qualified {
                "DRC PASSED - Advisory infos only"
            } else {
                "DRC NOT QUALIFIED - Requested mask coverage incomplete or failing"
            }
        );
    }
}

/// The JSON envelope shared by `--format json` and `--output`.
#[allow(clippy::too_many_arguments)]
pub fn json_report(
    violations: &[DRCViolation],
    results: &DRCResults,
    pcb_path: &Path,
    mfr: &str,
    layers: i64,
    meta: Option<&MetaCheckResult>,
    routing_quality: Option<&Json>,
    fabrication_process: Option<&Json>,
) -> Json {
    let count = |f: fn(&DRCViolation) -> bool| violations.iter().filter(|v| f(v)).count();
    let errors = count(DRCViolation::is_error);
    let mut summary = jobj! {
        "errors" => errors,
        "warnings" => count(DRCViolation::is_warning),
        "infos" => count(DRCViolation::is_info),
        "waived" => count(DRCViolation::is_waived),
        "rules_checked" => results.rules_checked,
        "rules_checked_by_rule" => results.rules_checked_by_rule_json(),
        "passed" => errors == 0 && mask_all_passed(results),
    };
    if results.suppressed_count > 0 {
        summary.set("suppressed", results.suppressed_count);
    }
    let mut data = jobj! {
        "file" => pcb_path.to_string_lossy().into_owned(),
        "manufacturer" => mfr,
        "layers" => layers,
        "summary" => summary,
        "violations" => Json::Arr(violations.iter().map(DRCViolation::to_dict).collect()),
        "mask_copper_assessments" => Json::Arr(results.mask_copper_assessments.iter().map(|a| a.to_dict()).collect()),
    };
    if let Some(m) = meta {
        data.set("meta_checks", m.to_dict());
    }
    if let Some(rq) = routing_quality {
        data.set("routing_quality", rq.clone());
    }
    if let Some(fp) = fabrication_process {
        data.set("fabrication_process", fp.clone());
    }
    data
}

fn output_summary(violations: &[DRCViolation], results: &DRCResults, pcb_path: &Path) {
    if violations.is_empty() {
        let mut msg = format!(
            "  {} rules checked, no violations found.",
            results.rules_checked
        );
        if results.suppressed_count > 0 {
            msg += &format!(
                "\n  ({} silkscreen warnings suppressed -- standard library footprints)",
                results.suppressed_count
            );
        }
        println!(
            "DRC {}: {}",
            if mask_all_passed(results) {
                "PASSED"
            } else {
                "NOT QUALIFIED"
            },
            file_name(pcb_path)
        );
        println!("{msg}");
        return;
    }
    println!("DRC Summary: {}", file_name(pcb_path));
    println!("{}", "=".repeat(50));
    let mut by_rule: std::collections::BTreeMap<&str, [usize; 4]> = Default::default();
    for v in violations {
        let c = by_rule.entry(v.rule_id.as_str()).or_default();
        if v.is_waived() {
            c[3] += 1;
        } else if v.is_error() {
            c[0] += 1;
        } else if v.is_info() {
            c[2] += 1;
        } else {
            c[1] += 1;
        }
    }
    println!(
        "{:<30} {:<8} {:<10} {:<8} {:<8}",
        "Rule ID", "Errors", "Warnings", "Infos", "Waived"
    );
    println!("{}", "-".repeat(68));
    let mut tot = [0usize; 4];
    for (rule, c) in &by_rule {
        println!(
            "{rule:<30} {:<8} {:<10} {:<8} {:<8}",
            c[0], c[1], c[2], c[3]
        );
        for k in 0..4 {
            tot[k] += c[k];
        }
    }
    println!("{}", "-".repeat(68));
    println!(
        "{:<30} {:<8} {:<10} {:<8} {:<8}",
        "TOTAL", tot[0], tot[1], tot[2], tot[3]
    );
}

// ------------------------------------------------------------------- main

fn parse_categories(spec: &str) -> std::result::Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for cat in spec.split(',') {
        let cat = cat.trim().to_lowercase();
        if !CHECK_CATEGORIES.contains(&cat.as_str()) {
            return Err(cat);
        }
        out.insert(cat);
    }
    Ok(out)
}

/// `kct check` entry point.
pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let mut args: Args = super::parse_args("check", args);
    // Unified-parser shim: `--layers` defaults to 2 there and is only
    // forwarded when it differs, so an explicit `--layers 2` auto-detects.
    if args.layers == Some(2) {
        args.layers = None;
    }
    let sch_field_threshold = args
        .sch_field_threshold
        .unwrap_or(DEFAULT_SCH_FIELD_THRESHOLD_MM);

    for (flag, value) in [
        ("--max-fragment-fraction", args.max_fragment_fraction),
        ("--max-staircase-fraction", args.max_staircase_fraction),
    ] {
        if let Some(v) = value {
            if !(0.0..=1.0).contains(&v) {
                eprintln!(
                    "Error: {flag} must be a fraction between 0.0 and 1.0, got {}",
                    crate::pyjson::py_float_repr(v)
                );
                return Ok(1);
            }
        }
    }
    if args.no_net_class_map {
        if let Some(ncm) = &args.net_class_map {
            eprintln!(
                "Error: --no-net-class-map cannot be combined with --net-class-map {}: one \
                 disables sidecar auto-discovery, the other names a sidecar to load. Pass exactly \
                 one of them.",
                py_repr_str(ncm)
            );
            return Ok(1);
        }
    }
    if args.no_current_paths {
        if let Some(cp) = &args.current_paths {
            eprintln!(
                "Error: --no-current-paths cannot be combined with --current-paths {}: one \
                 disables sidecar auto-discovery, the other names a sidecar to load. Pass exactly \
                 one of them.",
                py_repr_str(cp)
            );
            return Ok(1);
        }
    }
    let bad_category = |cat: String| {
        eprintln!("Error: Unknown check category: {}", py_repr_str(&cat));
        eprintln!("Available: {}", CHECK_CATEGORIES.join(", "));
    };
    let only_set = match args.only_checks.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => match parse_categories(s) {
            Ok(set) => Some(set),
            Err(cat) => {
                bad_category(cat);
                return Ok(1);
            }
        },
    };
    let skip_set = match args.skip_checks.as_deref().filter(|s| !s.is_empty()) {
        None => BTreeSet::new(),
        Some(s) => match parse_categories(s) {
            Ok(set) => set,
            Err(cat) => {
                bad_category(cat);
                return Ok(1);
            }
        },
    };
    if only_set
        .as_ref()
        .is_some_and(|o| o.contains("physical_copper_gap"))
        && args.physical_copper_gap.is_none()
    {
        eprintln!("Error: physical_copper_gap requires --physical-copper-gap MM");
        return Ok(1);
    }

    let input_path = resolve(&args.pcb);
    if !input_path.exists() {
        eprintln!("Error: Path not found: {}", input_path.display());
        return Ok(1);
    }
    let pcb_path = if input_path.is_dir() {
        match find_pcb_file(&input_path) {
            Some(p) => p,
            None => {
                eprintln!(
                    "Error: No .kicad_pcb file found in directory: {}",
                    input_path.display()
                );
                eprintln!(
                    "Hint: Specify a .kicad_pcb file directly, or ensure the directory contains one."
                );
                return Ok(1);
            }
        }
    } else if input_path.extension().and_then(|e| e.to_str()) != Some("kicad_pcb") {
        eprintln!(
            "Error: Expected .kicad_pcb file, got: {}",
            file_name(&input_path)
        );
        eprintln!("Hint: Provide a .kicad_pcb file or a directory containing one.");
        return Ok(1);
    } else {
        input_path
    };

    if args.netlist_sync {
        return crate::sync::drift::run_netlist_sync_gate(
            &pcb_path,
            args.schematic.as_deref(),
            args.strict,
        );
    }
    if args.refill_zones {
        refill_zones_in_place(&pcb_path);
    }
    let pcb = match Pcb::load(&pcb_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error loading PCB: {e}");
            return Ok(1);
        }
    };
    crate::sync::drift::emit_drift_banner(&pcb_path, args.schematic.as_deref());

    let (effective_mfr, notices, mfr_source) =
        resolve_effective_check_mfr(args.mfr.as_deref(), &pcb_path, "jlcpcb");
    for line in notices {
        eprintln!("{line}");
    }
    let layers = match args.layers {
        Some(l) => l,
        None => {
            let detected = pcb.copper_layers().len() as i64;
            if detected > 0 {
                detected
            } else {
                2
            }
        }
    };

    // ---- net-class-map sidecar (Issue #2684 / #3917 / #4601).
    let mut net_class_map: Option<NetClassMap> = None;
    let ncm_explicit = args.net_class_map.is_some();
    let mut ncm_candidates: Vec<PathBuf> = Vec::new();
    let ncm_path = if let Some(p) = &args.net_class_map {
        Some(resolve(p))
    } else if args.no_net_class_map {
        None
    } else {
        ncm_candidates = crate::sidecars::net_class_map_sidecar_candidates(&pcb_path);
        ncm_candidates.iter().find(|c| c.is_file()).cloned()
    };
    if let Some(path) = &ncm_path {
        if !path.exists() {
            eprintln!("Error: net-class-map file not found: {}", path.display());
            return Ok(1);
        }
        match crate::router::rules::net_class_map_from_path(path, Some(&pcb_path)) {
            Ok(m) => {
                net_class_map = Some(m);
                if !ncm_explicit {
                    eprintln!(
                        "[INFO] auto-loaded net-class-map sidecar: {}",
                        path.display()
                    );
                }
            }
            Err(e) => {
                if ncm_explicit {
                    eprintln!("Error: {e}");
                    return Ok(1);
                }
                eprintln!(
                    "WARNING: ignoring malformed net-class-map sidecar {}: {e}",
                    path.display()
                );
            }
        }
    }
    if net_class_map.is_none() {
        let inactive: Vec<&str> = MEASUREMENT_RULE_IDS
            .iter()
            .copied()
            .filter(|r| only_set.as_ref().is_none_or(|o| o.contains(*r)) && !skip_set.contains(*r))
            .collect();
        if !inactive.is_empty() {
            let head = format!(
                "WARNING: the following rules are INACTIVE without --net-class-map and will \
                 silently pass: {}.",
                inactive.join(", ")
            );
            let tail = if args.no_net_class_map {
                "  Sidecar auto-discovery was disabled with --no-net-class-map; drop that flag or \
                 pass a sidecar explicitly with --net-class-map to validate length-match skew."
                    .to_string()
            } else if let Some(p) = &ncm_path {
                format!(
                    "  The sidecar at {} could not be loaded (see the warning above); fix it or \
                     pass a different one with --net-class-map to validate length-match skew.",
                    p.display()
                )
            } else {
                let probed: String = ncm_candidates
                    .iter()
                    .map(|c| format!("\n  {}", c.display()))
                    .collect();
                format!(
                    "  No sidecar was found at any of:{probed}\nPass one explicitly with \
                     --net-class-map, or place it at one of the paths above, to validate \
                     length-match skew."
                )
            };
            eprintln!("{head}{tail}");
        }
    }

    // ---- courtyard waivers (Issue #4137).
    let mut cw = None;
    let cw_explicit = args.courtyard_waivers.is_some();
    let cw_path = match &args.courtyard_waivers {
        Some(p) => Some(resolve(p)),
        None => courtyard_waivers::discover_courtyard_waivers_sidecar(&pcb_path),
    };
    if let Some(p) = &cw_path {
        if !p.exists() {
            eprintln!("Error: courtyard-waivers file not found: {}", p.display());
            return Ok(1);
        }
        match courtyard_waivers::load_courtyard_waivers(p) {
            Ok(w) => {
                cw = Some(w);
                if !cw_explicit {
                    eprintln!(
                        "[INFO] auto-loaded courtyard-waivers sidecar: {}",
                        p.display()
                    );
                }
            }
            Err(e) => {
                if cw_explicit {
                    eprintln!("Error: {e}");
                    return Ok(1);
                }
                eprintln!(
                    "WARNING: ignoring malformed courtyard-waivers sidecar {}: {e}",
                    p.display()
                );
            }
        }
    }

    // ---- general waivers (Issue #4417).
    let mut general_waivers = None;
    let gw_explicit = args.waivers.is_some();
    let gw_path = match &args.waivers {
        Some(p) => Some(resolve(p)),
        None => waivers::discover_waivers_sidecar(&pcb_path),
    };
    if let Some(p) = &gw_path {
        if !p.exists() {
            eprintln!("Error: waivers file not found: {}", p.display());
            return Ok(1);
        }
        match waivers::load_waivers(p) {
            Ok(w) => {
                general_waivers = Some(w);
                if !gw_explicit {
                    eprintln!("[INFO] auto-loaded waivers sidecar: {}", p.display());
                }
            }
            Err(e) => {
                if gw_explicit {
                    eprintln!("Error: {e}");
                    return Ok(1);
                }
                eprintln!(
                    "WARNING: ignoring malformed waivers sidecar {}: {e}",
                    p.display()
                );
            }
        }
    }

    // ---- current-paths sidecar (Issue #4980 / #5124).
    let mut current_path_specs: Vec<CurrentPathSpec> = Vec::new();
    let cp_explicit = args.current_paths.is_some();
    let mut cp_candidates = Vec::new();
    let cp_path = if let Some(p) = &args.current_paths {
        Some(resolve(p))
    } else if args.no_current_paths {
        None
    } else {
        cp_candidates = current_paths::current_paths_sidecar_candidates(&pcb_path);
        cp_candidates.iter().find(|c| c.is_file()).cloned()
    };
    if let Some(p) = &cp_path {
        if !p.exists() {
            eprintln!("Error: current-paths file not found: {}", p.display());
            return Ok(1);
        }
        match current_paths::load_current_path_specs(p) {
            Ok(specs) => {
                current_path_specs = specs;
                if !cp_explicit {
                    eprintln!("[INFO] auto-loaded current-paths sidecar: {}", p.display());
                }
            }
            Err(e) => {
                if cp_explicit {
                    eprintln!("Error: parsing current-paths JSON: {e}");
                    return Ok(1);
                }
                eprintln!(
                    "WARNING: ignoring malformed current-paths sidecar {}: {e}",
                    p.display()
                );
            }
        }
    } else if !args.no_current_paths
        && !cp_candidates.is_empty()
        && only_set
            .as_ref()
            .is_none_or(|o| o.contains("path_ampacity"))
        && !skip_set.contains("path_ampacity")
    {
        let probed: String = cp_candidates
            .iter()
            .map(|c| format!("\n  {}", c.display()))
            .collect();
        eprintln!(
            "WARNING: path_ampacity is INACTIVE without --current-paths and will silently pass. \
             No sidecar was found at any of:{probed}\nPass one explicitly with --current-paths, \
             or place it at one of the paths above, to validate declared branch current paths."
        );
    }

    // ---- resolve net-class-map keys onto board nets (Issue #4321).
    let mut declared_ampacity: Vec<(String, f64)> = Vec::new();
    let mut ampacity_resolution: Option<Vec<(String, String)>> = None;
    if let Some(map) = net_class_map.take() {
        for (k, nc) in &map {
            if let Some(t) = nc.target_ampacity {
                declared_ampacity.push((k.clone(), t));
            }
        }
        let board_names: Vec<String> = pcb
            .nets()
            .iter()
            .filter(|n| !n.name.is_empty())
            .map(|n| n.name.clone())
            .collect();
        let keys: Vec<String> = map.iter().map(|(k, _)| k.clone()).collect();
        let resolved = crate::router::net_names::resolve_net_class_map_keys(&keys, &board_names);
        net_class_map = Some(
            resolved
                .iter()
                .filter_map(|(board, user)| {
                    map.iter()
                        .find(|(k, _)| k == user)
                        .map(|(_, nc)| (board.clone(), nc.clone()))
                })
                .collect(),
        );
        ampacity_resolution = Some(resolved);
    }

    // ---- copper weights (Issue #4326).
    let (mut cli_outer, mut cli_inner) = (None, None);
    let copper_explicit = args.copper.is_some();
    if let Some(raw) = &args.copper {
        match parse_copper_weight_arg(raw) {
            Ok((o, i)) => {
                cli_outer = o;
                cli_inner = i;
            }
            Err(e) => {
                eprintln!("Error: {e}");
                return Ok(1);
            }
        }
    }
    let preset_copper_oz = cli_outer.unwrap_or(1.0);
    let (stackup_outer, stackup_inner) =
        match crate::physics::stackup::Stackup::from_pcb(pcb.sexp()).outer_inner_copper_oz() {
            Some((o, i)) => (o, i),
            None => (None, None),
        };
    let resolved_outer = cli_outer.or(stackup_outer);
    let resolved_inner = cli_inner.or(stackup_inner);
    let disagrees = |c: Option<f64>, s: Option<f64>| matches!((c, s), (Some(c), Some(s)) if (c - s).abs() > 1e-6);
    let mut copper_disagreement = false;
    if copper_explicit {
        if disagrees(cli_outer, stackup_outer) {
            copper_disagreement = true;
            let (c, s) = (g(cli_outer.unwrap()), g(stackup_outer.unwrap()));
            eprintln!(
                "WARNING: stackup declares {s}oz outer but --copper requests {c}oz — ampacity is \
                 evaluating at {c}oz. Pass --copper {s} or fix the stackup."
            );
        }
        if disagrees(cli_inner, stackup_inner) {
            copper_disagreement = true;
            let (c, s) = (g(cli_inner.unwrap()), g(stackup_inner.unwrap()));
            eprintln!(
                "WARNING: stackup declares {s}oz inner but --copper requests {c}oz — ampacity is \
                 evaluating at {c}oz. Pass --copper inner={s} or fix the stackup."
            );
        }
        if copper_disagreement && args.strict {
            eprintln!(
                "ERROR: --strict: stackup-vs---copper copper-weight disagreement is fatal (exit \
                 2). Reconcile --copper with the board's declared stackup."
            );
        }
    }

    let mut mask_copper_request = None;
    if let Some(p) = &args.mask_copper_config {
        match MaskCopperRequest::from_file(p) {
            Ok(r) => mask_copper_request = Some(r),
            Err(e) => {
                eprintln!("Error: invalid mask-to-copper request: {e}");
                return Ok(1);
            }
        }
    }
    if mask_copper_request.is_some()
        && (skip_set.contains("mask_to_copper")
            || only_set
                .as_ref()
                .is_some_and(|o| !o.contains("mask_to_copper")))
    {
        eprintln!("Error: explicit mask-copper config conflicts with --only/--skip selection");
        return Ok(1);
    }

    let checker_result = DRCChecker::new(
        &pcb,
        DRCCheckerOptions {
            manufacturer: effective_mfr.clone(),
            layers,
            copper_oz: preset_copper_oz,
            suppress_library: args.suppress_library,
            net_class_map: net_class_map.clone(),
            warn_on_inactive_skew_rules: false,
            verbose: args.verbose,
            emit_measurements: true,
            courtyard_waivers: cw,
            strict_connectivity: !args.legacy_connectivity,
            copper_oz_outer: resolved_outer,
            copper_oz_inner: resolved_inner,
            current_path_specs,
            mask_copper_request,
            physical_copper_gap_mm: args.physical_copper_gap,
        },
    );
    let mut checker = match checker_result {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return Ok(1);
        }
    };
    let (rules, msg) = manufacturers::fabrication_overrides::resolve_pcb_fabrication_overrides(
        &pcb_path,
        checker.design_rules.clone(),
        &effective_mfr,
    );
    checker.design_rules = rules;
    if let Some(m) = msg {
        let prefix = if m.starts_with("ignoring") {
            "WARNING: "
        } else {
            "[INFO] "
        };
        eprintln!("{prefix}{m}");
    }

    let (pad_grid_threshold, pad_grid_auto_derive) = if let Some(t) = args.pad_grid_tolerance {
        (Some(t), false)
    } else if args.pad_grid_strict {
        (None, false)
    } else {
        (None, true)
    };
    let sch_fields_active = only_set.as_ref().is_none_or(|o| o.contains("sch_fields"))
        && !skip_set.contains("sch_fields")
        && !args.drc_only;
    let sch_path = if sch_fields_active {
        resolve_check_schematic(args.schematic.as_deref(), &pcb_path)
    } else {
        None
    };
    let mut results = run_selected_checks(
        &checker,
        only_set.as_ref(),
        &skip_set,
        &SelectedChecksOptions {
            pad_grid_threshold,
            pad_grid_auto_derive,
            sch_path: sch_path.as_deref(),
            sch_field_threshold,
            pcb_path: Some(&pcb_path),
        },
    );
    if let Some(w) = &general_waivers {
        waivers::apply_waivers(&mut results, w);
    }
    if !declared_ampacity.is_empty() {
        if let Some(res) = &ampacity_resolution {
            warn_unevaluated_ampacity(&declared_ampacity, res, &pcb, only_set.as_ref(), &skip_set);
        }
    }
    maybe_emit_via_in_pad_tier_advisory(&effective_mfr, &results.violations, mfr_source);
    maybe_emit_via_in_pad_process_advisory(&results.violations);

    let drc_only = args.drc_only;
    let rq_gating = args.max_fragment_fraction.is_some() || args.max_staircase_fraction.is_some();
    let routing_quality = if !drc_only || rq_gating {
        Some(compute_routing_quality(&pcb))
    } else {
        None
    };
    let mut routing_quality_dict = routing_quality.as_ref().map(|m| m.to_dict());
    if rq_gating {
        if let (Some(m), Some(Json::Obj(items))) = (&routing_quality, routing_quality_dict.as_mut())
        {
            let breaches = evaluate_routing_quality_thresholds(
                m,
                args.max_fragment_fraction,
                args.max_staircase_fraction,
            );
            for b in &breaches {
                results.add(
                    DRCViolation::new(b.rule_id, "error", b.message.clone())
                        .actual(b.actual)
                        .required(b.limit),
                );
            }
            if let Json::Obj(extra) = routing_quality_gate_dict(
                &breaches,
                args.max_fragment_fraction,
                args.max_staircase_fraction,
            ) {
                items.extend(extra);
            }
        }
    }
    let _ = routing_quality::COORD_EPSILON_MM;

    let mut violations: Vec<DRCViolation> = results.violations.clone();
    if args.errors_only {
        violations.retain(|v| v.is_error());
    }
    if !args.refill_zones {
        warn_stale_zone_fills(&results.violations, &pcb_path);
    }
    let error_count = violations.iter().filter(|v| v.is_error()).count();
    let warning_count = violations.iter().filter(|v| v.is_warning()).count();
    let drc_passed =
        error_count == 0 && mask_all_passed(&results) && !(warning_count > 0 && args.strict);
    let drc_sub = SubCheckResult::new(
        if drc_passed { "PASSED" } else { "FAILED" },
        format!(
            "{} rules checked, {error_count} error(s), {warning_count} warning(s)",
            results.rules_checked
        ),
    );
    let meta = if !drc_only {
        Some(run_meta_checks(
            &pcb_path,
            drc_sub,
            args.schematic.as_deref(),
            args.strict,
        ))
    } else {
        None
    };
    let fabrication_process =
        manufacturers::fabrication_process::describe_selection(&checker.design_rules);
    if !results.mask_copper_assessments.is_empty() && args.format != "json" {
        for a in &results.mask_copper_assessments {
            println!(
                "Mask-to-copper: {}; passed={}",
                a.coverage,
                if a.passed() { "True" } else { "False" }
            );
            for r in &a.reasons {
                println!("  {r}");
            }
        }
    }
    let report = || {
        json_report(
            &violations,
            &results,
            &pcb_path,
            &effective_mfr,
            layers,
            meta.as_ref(),
            routing_quality_dict.as_ref(),
            fabrication_process.as_ref(),
        )
    };
    match args.format.as_str() {
        "json" => println!("{}", dumps_indent(&report(), 2)),
        "summary" => {
            output_summary(&violations, &results, &pcb_path);
            if let Some(m) = &meta {
                print_meta_check_stanza(m, args.verbose);
            }
            if let Some(rq) = &routing_quality {
                print_routing_quality_stanza(rq);
            }
        }
        _ => {
            output_table(
                &violations,
                &results,
                &pcb_path,
                &effective_mfr,
                layers,
                args.verbose,
            );
            if let Some(m) = &meta {
                print_meta_check_stanza(m, args.verbose);
            }
            if let Some(rq) = &routing_quality {
                print_routing_quality_stanza(rq);
            }
        }
    }
    if let Some(out) = &args.output {
        let path = PathBuf::from(out);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, dumps_indent(&report(), 2) + "\n")?;
    }
    if args.emit_dru || args.emit_drc_constraints {
        emit_drc_sidecars(
            &pcb_path,
            &checker.design_rules,
            &effective_mfr,
            layers,
            preset_copper_oz,
            net_class_map.as_ref(),
            args.emit_drc_constraints,
        );
    }

    if results.mask_copper_assessments.iter().any(|a| !a.passed()) {
        return Ok(2);
    }
    if args.strict && copper_disagreement {
        return Ok(2);
    }
    if drc_only {
        return Ok(if error_count > 0 || (warning_count > 0 && args.strict) {
            2
        } else {
            0
        });
    }
    let meta = meta.expect("meta mode");
    if meta.overall == "FAILED" {
        return Ok(2);
    }
    if meta.overall == "INCOMPLETE" && !args.allow_incomplete {
        return Ok(2);
    }
    Ok(0)
}

/// `_emit_drc_sidecars`: write `<board>.kicad_dru` (and `.kicad_pro` with
/// `--emit-drc-constraints`) from the resolved rules.
fn emit_drc_sidecars(
    pcb_path: &Path,
    rules: &manufacturers::DesignRules,
    manufacturer_id: &str,
    layers: i64,
    copper_oz: f64,
    net_class_map: Option<&NetClassMap>,
    emit_both: bool,
) {
    match manufacturers::drc_sidecars::write_drc_sidecars(
        pcb_path,
        rules,
        manufacturer_id,
        layers,
        copper_oz,
        net_class_map,
        emit_both,
    ) {
        Ok(written) => eprintln!(
            "DRC-constraint sidecars updated: {}",
            written
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Err(e) => eprintln!(
            "WARNING: could not emit DRC-constraint sidecar(s) next to {}: {e}. The check \
             verdict is unaffected.",
            pcb_path.display()
        ),
    }
}
