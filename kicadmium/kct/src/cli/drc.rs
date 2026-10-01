//! `kct drc`: run KiCad DRC or parse/explain DRC reports (port of
//! `kicad_tools.cli.drc_cmd`, plus `cli.drc_summary` as [`summary_main`]).
//!
//! Accepts the full `drc_cmd` flag set (a superset of the outer `kct drc`
//! parser, which only forwarded a subset).
//!
//! Exit codes: 0 no errors, 1 errors or failure, 2 warnings with `--strict`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;

use super::runner::{find_kicad_cli, run_drc};
use super::{parse_args, Globals};
use crate::drc::checker::{check_manufacturer_rules, get_design_rules, ManufacturerCheck};
use crate::drc::suggestions::generate_fix_suggestions;
use crate::drc::violation::{DRCViolation, Suggestion, ViolationCategory, ViolationType};
use crate::drc::waivers::{
    apply_waivers_to_report, discover_waivers_sidecar, load_waivers, py_parent, WaiverApplication,
    Waivers, WAIVERS_FILENAME,
};
use crate::drc::DRCReport;
use crate::jobj;
use crate::manufacturers::{self, DesignRules};
use crate::pyjson::{dumps_indent, py_float_repr, Json};
use crate::validate::filters::{load_filters_from_toml, FilterEngine, FilterLoadError};

/// Default manufacturer for compatibility hints.
pub const DEFAULT_HINT_MANUFACTURER: &str = "jlcpcb";

/// Every manufacturer name accepted by `get_profile` (ids and aliases).
fn all_manufacturer_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = manufacturers::profiles().iter().map(|p| p.id).collect();
    names.extend([
        "flash",
        "jlc",
        "lcsc",
        "seeed_fusion",
        "seeed-fusion",
        "seeedfusion",
        "seeedstudio",
        "osh",
        "osh_park",
        "jlcpcb_tier1",
        "jlcpcb-capabilityplus",
        "jlcpcb_capabilityplus",
        "jlcpcb-capability-plus",
    ]);
    names.sort_unstable();
    names
}

fn category_values() -> Vec<&'static str> {
    ViolationCategory::ALL.iter().map(|c| c.value()).collect()
}

#[derive(Parser, Debug)]
#[command(about = "Run DRC on PCBs or parse DRC reports")]
struct Args {
    /// PCB (.kicad_pcb) to check or DRC report (.json/.rpt) to parse
    input: Option<PathBuf>,
    /// Output format
    #[arg(long, default_value = "table", value_parser = ["table", "json", "summary"])]
    format: String,
    /// Show only errors, not warnings
    #[arg(long)]
    errors_only: bool,
    /// Exit with error code on warnings
    #[arg(long)]
    strict: bool,
    /// Filter by violation type (partial match)
    #[arg(short = 't', long = "type")]
    filter_type: Option<String>,
    /// Filter by net name
    #[arg(long)]
    net: Option<String>,
    /// Filter by root-cause category
    #[arg(short = 'c', long, value_parser = clap::builder::PossibleValuesParser::new(category_values()))]
    category: Option<String>,
    /// Target manufacturer for rules check
    #[arg(short = 'm', long, value_parser = clap::builder::PossibleValuesParser::new(all_manufacturer_names()))]
    mfr: Option<String>,
    /// Number of copper layers
    #[arg(short = 'l', long, default_value_t = 2)]
    layers: i64,
    /// Print manufacturer design rules and exit
    #[arg(long)]
    rules: bool,
    /// Compare design rules across manufacturers
    #[arg(long)]
    compare: bool,
    /// Show detailed violation information
    #[arg(short = 'v', long)]
    verbose: bool,
    /// Show fix suggestions for each violation
    #[arg(short = 's', long)]
    suggest: bool,
    /// TOML file with [[drc.filters]] rules to suppress or reclassify violations
    #[arg(long)]
    filter_config: Option<PathBuf>,
    /// Path to a .kct_waivers.json sidecar (schema version 2); auto-discovered when omitted
    #[arg(long)]
    waivers: Option<PathBuf>,
    /// Keep the DRC report file after running
    #[arg(long)]
    keep_report: bool,
    /// Save DRC report to this path
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
}

fn suffix(p: &Path) -> String {
    p.extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default()
}

fn file_name(s: &str) -> String {
    Path::new(s)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `Path(p).resolve()` (non-strict).
fn py_resolve(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args: Args = parse_args("drc", args);

    if args.compare {
        print_comparison(args.layers)?;
        return Ok(0);
    }
    if args.rules {
        print_manufacturer_rules(args.mfr.as_deref().unwrap_or("jlcpcb"), args.layers)?;
        return Ok(0);
    }
    let Some(input) = args.input.clone() else {
        use clap::CommandFactory;
        let mut cmd = Args::command().name("kct drc");
        let _ = cmd.print_help();
        eprintln!("\nError: input file required");
        eprintln!("\nTo see design rules, run with --rules --mfr <manufacturer>");
        eprintln!("To compare manufacturers, run with --compare");
        return Ok(1);
    };

    let waivers = match load_waivers_for(args.waivers.as_deref(), &input) {
        Ok(w) => w,
        Err(code) => return Ok(code),
    };

    let ext = suffix(&input);
    let mut report = if ext == ".kicad_pcb" {
        match run_drc_on_pcb(&input, args.output.as_deref(), args.keep_report) {
            Some(r) => r,
            None => return Ok(1),
        }
    } else if ext == ".json" || ext == ".rpt" {
        if !input.exists() {
            eprintln!("Error: File not found: {}", input.display());
            return Ok(1);
        }
        match DRCReport::load(&input) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error loading report: {e}");
                return Ok(1);
            }
        }
    } else {
        eprintln!("Error: Unsupported file type: {ext}");
        eprintln!("Expected .kicad_pcb (PCB) or .json/.rpt (report)");
        return Ok(1);
    };

    let mut filter_ignored = 0;
    if let Some(cfg) = &args.filter_config {
        match load_filters_from_toml(cfg) {
            Ok((drc_filters, _)) => {
                if !drc_filters.is_empty() {
                    let result = FilterEngine::new(drc_filters).apply(&report.violations);
                    filter_ignored = result.ignored_count();
                    report = DRCReport {
                        source_file: report.source_file,
                        created_at: report.created_at,
                        pcb_name: report.pcb_name,
                        violations: result.kept,
                        footprint_errors: report.footprint_errors,
                        ..Default::default()
                    };
                }
            }
            Err(FilterLoadError::NotFound(m)) => {
                eprintln!("Error: {m}");
                return Ok(1);
            }
            Err(e) => {
                eprintln!("Error loading filter config: {e}");
                return Ok(1);
            }
        }
    }

    if let Some(mfr) = &args.mfr {
        if waivers.as_ref().is_some_and(|w| !w.is_empty()) {
            eprintln!(
                "[INFO] --mfr manufacturer compatibility check does not apply waivers; run \
                 without --mfr for the waived DRC gate."
            );
        }
        return output_manufacturer_check(&report, mfr, args.layers, args.verbose);
    }

    let waiver_result = match &waivers {
        Some(w) if !w.is_empty() => Some(apply_waivers_to_report(&mut report, w)),
        _ => None,
    };

    let mut violations: Vec<DRCViolation> = report.violations.clone();
    if args.errors_only {
        violations.retain(DRCViolation::is_error);
    }
    if let Some(t) = &args.filter_type {
        let f = t.to_lowercase();
        violations.retain(|v| {
            v.type_str.to_lowercase().contains(&f) || v.message.to_lowercase().contains(&f)
        });
    }
    if let Some(net) = &args.net {
        violations.retain(|v| v.nets.contains(net));
    }
    if let Some(cat) = args
        .category
        .as_deref()
        .and_then(ViolationCategory::from_value)
    {
        violations.retain(|v| v.category() == cat);
    }
    if args.suggest {
        for v in violations.iter_mut() {
            if let Some(s) = generate_fix_suggestions(v) {
                v.suggestions = vec![Suggestion::Fix(s)];
            }
        }
    }

    match args.format.as_str() {
        "json" => output_json(
            &violations,
            &report,
            args.suggest,
            filter_ignored,
            waiver_result.as_ref(),
        ),
        "summary" => output_summary(&violations, &report, filter_ignored, waiver_result.as_ref()),
        _ => output_table(
            &violations,
            &report,
            args.verbose,
            args.suggest,
            args.layers,
            filter_ignored,
            waiver_result.as_ref(),
        ),
    }

    let errors = violations.iter().filter(|v| v.is_error()).count();
    let waived = violations.iter().filter(|v| v.is_waived()).count();
    let warnings = violations.len() - errors - waived;
    Ok(if errors > 0 {
        1
    } else if warnings > 0 && args.strict {
        2
    } else {
        0
    })
}

/// Resolve the waivers sidecar; `Err(code)` means exit with `code`.
fn load_waivers_for(explicit: Option<&Path>, input: &Path) -> Result<Option<Waivers>, i32> {
    let path = match explicit {
        Some(p) => Some(py_resolve(p)),
        None => {
            let mut found = discover_waivers_sidecar(input);
            let ext = suffix(input);
            if found.is_none() && (ext == ".json" || ext == ".rpt") {
                let candidate = py_parent(&py_parent(input)).join(WAIVERS_FILENAME);
                if candidate.is_file() {
                    found = Some(candidate);
                }
            }
            found
        }
    };
    let Some(path) = path else {
        return Ok(None);
    };
    if !path.exists() {
        eprintln!("Error: waivers file not found: {}", path.display());
        return Err(1);
    }
    match load_waivers(&path) {
        Ok(w) => {
            if explicit.is_none() {
                eprintln!("[INFO] auto-loaded waivers sidecar: {}", path.display());
            }
            Ok(Some(w))
        }
        Err(e) if explicit.is_some() => {
            eprintln!("Error: {e}");
            Err(1)
        }
        Err(e) => {
            eprintln!(
                "WARNING: ignoring malformed waivers sidecar {}: {e}",
                path.display()
            );
            Ok(None)
        }
    }
}

fn print_unused_waivers(w: Option<&WaiverApplication>) {
    let Some(w) = w.filter(|w| !w.unused.is_empty()) else {
        return;
    };
    println!("\n{}", "-".repeat(60));
    println!("UNUSED WAIVERS (advisory, non-blocking):");
    for m in w.unused_messages() {
        println!("\n  [i] {m}");
    }
}

/// Run kicad-cli DRC on a board and parse the JSON report.
pub fn run_drc_on_pcb(pcb: &Path, output: Option<&Path>, keep_report: bool) -> Option<DRCReport> {
    if !pcb.exists() {
        eprintln!("Error: PCB not found: {}", pcb.display());
        return None;
    }
    if find_kicad_cli().is_none() {
        eprintln!("Error: kicad-cli not found");
        eprintln!("Install KiCad 8 from: https://www.kicad.org/download/");
        eprintln!("\nmacOS: brew install --cask kicad");
        return None;
    }
    println!(
        "Running DRC on: {}",
        pcb.file_name().unwrap_or_default().to_string_lossy()
    );
    let result = run_drc(pcb, output, "json", true, None);
    if !result.success {
        eprintln!("Error running DRC: {}", result.stderr);
        return None;
    }
    let out = result.output_path?;
    let report = match DRCReport::load(&out) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error parsing DRC report: {e}");
            return None;
        }
    };
    if !keep_report && output.is_none() {
        let _ = std::fs::remove_file(&out);
    }
    Some(report)
}

#[derive(Default, Clone, Copy)]
struct Counts {
    errors: usize,
    warnings: usize,
    waived: usize,
}

impl Counts {
    fn add(&mut self, v: &DRCViolation) {
        if v.is_waived() {
            self.waived += 1;
        } else if v.is_error() {
            self.errors += 1;
        } else {
            self.warnings += 1;
        }
    }
    fn total(&self) -> usize {
        self.errors + self.warnings + self.waived
    }
    fn format(&self) -> String {
        let mut parts = Vec::new();
        let plural = |n: usize| if n != 1 { "s" } else { "" };
        if self.errors > 0 {
            parts.push(format!("{} error{}", self.errors, plural(self.errors)));
        }
        if self.warnings > 0 {
            parts.push(format!(
                "{} warning{}",
                self.warnings,
                plural(self.warnings)
            ));
        }
        if self.waived > 0 {
            parts.push(format!("{} waived", self.waived));
        }
        parts.join(", ")
    }
}

fn group_counts<'a>(
    violations: &'a [DRCViolation],
    key: impl Fn(&'a DRCViolation) -> String,
) -> Vec<(String, Counts)> {
    let mut out: Vec<(String, Counts)> = Vec::new();
    for v in violations {
        let k = key(v);
        match out.iter_mut().find(|(n, _)| *n == k) {
            Some((_, c)) => c.add(v),
            None => {
                let mut c = Counts::default();
                c.add(v);
                out.push((k, c));
            }
        }
    }
    out
}

fn output_table(
    violations: &[DRCViolation],
    report: &DRCReport,
    verbose: bool,
    show_suggestions: bool,
    layers: i64,
    filtered: usize,
    waivers: Option<&WaiverApplication>,
) {
    let errors_n = violations.iter().filter(|v| v.is_error()).count();
    let waived_n = violations.iter().filter(|v| v.is_waived()).count();
    let warnings_n = violations.len() - errors_n - waived_n;
    let eq = "=".repeat(60);
    let dash = "-".repeat(60);
    println!("\n{eq}");
    println!("DRC VALIDATION SUMMARY");
    println!("{eq}");
    if !report.source_file.is_empty() {
        println!("File: {}", file_name(&report.source_file));
    }
    if !report.pcb_name.is_empty() {
        println!("PCB: {}", report.pcb_name);
    }
    println!("\nResults:");
    println!("  Errors:     {errors_n}");
    println!("  Warnings:   {warnings_n}");
    if waived_n > 0 {
        println!("  Waived:     {waived_n}");
    }
    if filtered > 0 {
        println!("  Filtered:   {filtered} violations filtered");
    }
    if violations.is_empty() {
        println!("\n{eq}");
        println!("DRC PASSED - No violations found");
        print_unused_waivers(waivers);
        return;
    }

    let mut by_type = group_counts(violations, |v| v.type_str.clone());
    by_type.sort_by_key(|(_, c)| std::cmp::Reverse(c.total()));
    println!("\n{dash}");
    println!("BY TYPE:");
    for (t, c) in &by_type {
        println!("  {t}: {}", c.format());
    }
    let mut by_cat = group_counts(violations, |v| v.category().value().to_string());
    by_cat.sort_by_key(|(_, c)| std::cmp::Reverse(c.total()));
    println!("\n{dash}");
    println!("BY CATEGORY:");
    for (t, c) in &by_cat {
        println!("  {t}: {}", c.format());
    }

    let errors: Vec<&DRCViolation> = violations.iter().filter(|v| v.is_error()).collect();
    let warnings: Vec<&DRCViolation> = violations
        .iter()
        .filter(|v| !v.is_error() && !v.is_waived())
        .collect();
    let waived: Vec<&DRCViolation> = violations.iter().filter(|v| v.is_waived()).collect();

    if !errors.is_empty() {
        println!("\n{dash}");
        println!("ERRORS (must fix):");
        for v in &errors {
            print_single(v, verbose, show_suggestions, "  ");
        }
    }
    if !warnings.is_empty() {
        println!("\n{dash}");
        println!("WARNINGS (review recommended):");
        let shown = if verbose {
            &warnings[..]
        } else {
            &warnings[..warnings.len().min(10)]
        };
        for v in shown {
            print_single(v, verbose, show_suggestions, "  ");
        }
        if warnings.len() > 10 && !verbose {
            println!(
                "\n  ... and {} more warnings (use --verbose)",
                warnings.len() - 10
            );
        }
    }
    if !waived.is_empty() {
        println!("\n{dash}");
        println!("WAIVED (documented exceptions, non-blocking):");
        for v in &waived {
            print_single(v, verbose, show_suggestions, "  ");
            println!(
                "      Waiver reason: {}",
                v.waiver_reason.as_deref().unwrap_or("None")
            );
            if let Some(issue) = v.waiver_issue.as_deref().filter(|i| !i.is_empty()) {
                println!("      Waiver issue: {issue}");
            }
        }
    }
    print_unused_waivers(waivers);

    println!("\n{eq}");
    if !errors.is_empty() {
        println!("DRC FAILED - Fix errors before manufacturing");
        if let Some(hint) = manufacturer_compatibility_hint(violations, layers) {
            println!("{hint}");
        }
    } else if !warnings.is_empty() {
        println!("DRC WARNING - Review warnings");
    } else {
        println!("DRC PASSED - No unwaived violations");
    }
}

fn print_single(v: &DRCViolation, verbose: bool, show_suggestions: bool, indent: &str) {
    let symbol = if v.is_waived() {
        "~"
    } else if v.is_error() {
        "X"
    } else {
        "!"
    };
    println!("\n{indent}[{symbol}] {}", v.type_str);
    println!("{indent}    {}", v.message);
    if let (Some(r), Some(a)) = (v.required_value_mm, v.actual_value_mm) {
        println!(
            "{indent}    Required: {r:.3}mm, Actual: {a:.3}mm (need +{:.3}mm)",
            r - a
        );
    }
    if verbose {
        for loc in &v.locations {
            let pos = if loc.x_mm != 0.0 || loc.y_mm != 0.0 {
                format!("({:.2}, {:.2})", loc.x_mm, loc.y_mm)
            } else {
                String::new()
            };
            let layer = if loc.layer.is_empty() {
                String::new()
            } else {
                format!("[{}]", loc.layer)
            };
            println!("{indent}    -> {pos} {layer}");
        }
        if !v.nets.is_empty() {
            println!("{indent}    Nets: {}", v.nets.join(", "));
        }
        if !v.suggestions.is_empty() {
            println!("{indent}    Suggestions:");
            for s in v.suggestions.iter().take(3) {
                println!("{indent}      - {s}");
            }
        }
    }
    if show_suggestions {
        if let Some(first) = v.suggestions.first() {
            println!("{indent}    FIX: {}", first.text());
            if let (true, Suggestion::Fix(fix)) = (verbose, first) {
                for alt in fix.alternatives.iter().take(2) {
                    println!("{indent}         or: {}", alt.description);
                }
            }
        }
    }
}

fn output_json(
    violations: &[DRCViolation],
    report: &DRCReport,
    show_suggestions: bool,
    filtered: usize,
    waivers: Option<&WaiverApplication>,
) {
    let data_violations: Vec<Json> = violations
        .iter()
        .map(|v| {
            let mut d = v.to_json();
            if !show_suggestions {
                d.remove("suggestions");
            }
            d
        })
        .collect();
    let waived = violations.iter().filter(|v| v.is_waived()).count();
    let mut summary = jobj! {
        "errors" => violations.iter().filter(|v| v.is_error()).count(),
        "warnings" => violations.iter().filter(|v| !v.is_error() && !v.is_waived()).count(),
    };
    if waived > 0 {
        summary.set("waived", waived);
    }
    if filtered > 0 {
        summary.set("filtered", filtered);
    }
    let mut data = jobj! {
        "source" => report.source_file.as_str(),
        "pcb_name" => report.pcb_name.as_str(),
        "summary" => summary,
        "violations" => Json::Arr(data_violations),
    };
    if let Some(w) = waivers.filter(|w| !w.unused.is_empty()) {
        data.set("unused_waivers", w.to_json_list());
    }
    println!("{}", dumps_indent(&data, 2));
}

fn output_summary(
    violations: &[DRCViolation],
    report: &DRCReport,
    filtered: usize,
    waivers: Option<&WaiverApplication>,
) {
    if violations.is_empty() {
        if filtered > 0 {
            println!("No DRC violations found ({filtered} violations filtered).");
        } else {
            println!("No DRC violations found.");
        }
        print_unused_waivers(waivers);
        return;
    }
    println!("DRC Summary: {}", report.source_file);
    println!("{}", "=".repeat(50));
    let mut by_type = group_counts(violations, |v| v.type_str.clone());
    by_type.sort_by(|a, b| a.0.cmp(&b.0));
    let any_waived = by_type.iter().any(|(_, c)| c.waived > 0);
    let wh = if any_waived {
        format!(" {:<8}", "Waived")
    } else {
        String::new()
    };
    println!("{:<25} {:<8} {:<8}{wh}", "Type", "Errors", "Warnings");
    println!("{}", "-".repeat(50));
    let (mut te, mut tw, mut tv) = (0, 0, 0);
    for (name, c) in &by_type {
        let wc = if any_waived {
            format!(" {:<8}", c.waived)
        } else {
            String::new()
        };
        println!("{name:<25} {:<8} {:<8}{wc}", c.errors, c.warnings);
        te += c.errors;
        tw += c.warnings;
        tv += c.waived;
    }
    println!("{}", "-".repeat(50));
    let wt = if any_waived {
        format!(" {tv:<8}")
    } else {
        String::new()
    };
    println!("{:<25} {te:<8} {tw:<8}{wt}", "TOTAL");
    print_unused_waivers(waivers);
}

fn output_manufacturer_check(
    report: &DRCReport,
    mfr: &str,
    layers: i64,
    _verbose: bool,
) -> Result<i32> {
    let profile = manufacturers::profile(mfr)?;
    let eq = "=".repeat(60);
    let dash = "-".repeat(60);
    println!("\n{eq}");
    println!("MANUFACTURER COMPATIBILITY: {}", profile.name);
    println!("{eq}");
    println!("Layer count: {layers}");
    println!("Website: {}", profile.website);

    let checks = check_manufacturer_rules(report, mfr, layers, 1.0);
    let (compatible, incompatible): (Vec<&ManufacturerCheck>, Vec<&ManufacturerCheck>) =
        checks.iter().partition(|c| c.is_compatible());
    if !incompatible.is_empty() {
        println!("\n{dash}");
        println!("INCOMPATIBLE VIOLATIONS ({}):", incompatible.len());
        for c in &incompatible {
            println!("\n  [X] {}", c.violation.type_str);
            println!("      {}", c.violation.message);
            if let (Some(a), Some(l)) = (c.actual_value, c.manufacturer_limit) {
                println!("      Actual: {a:.3}mm, Limit: {l:.3}mm");
            }
        }
    } else {
        println!("\n{dash}");
        println!(
            "All {} checked violations are compatible!",
            compatible.len()
        );
    }
    println!("\n{eq}");
    println!(
        "Compatible: {}, Incompatible: {}",
        compatible.len(),
        incompatible.len()
    );
    if profile.supports_assembly {
        println!("Assembly: Supported");
        if let Some(lib) = profile.parts_library {
            println!("Parts Library: {lib}");
        }
    } else {
        println!("Assembly: Not available");
    }
    Ok(if incompatible.is_empty() { 0 } else { 1 })
}

fn catalog_url(parts_library: &str) -> Option<&'static str> {
    match parts_library {
        "LCSC" => Some("https://jlcpcb.com/parts"),
        "OPL" => Some("https://www.seeedstudio.com/opl.html"),
        _ => None,
    }
}

fn print_manufacturer_rules(mfr: &str, layers: i64) -> Result<()> {
    let profile = manufacturers::profile(mfr)?;
    let r = get_design_rules(profile.id, layers, 1.0)?;
    let eq = "=".repeat(60);
    let f = py_float_repr;
    println!("\n{eq}");
    println!(
        "{} {layers}-LAYER PCB CAPABILITIES",
        profile.name.to_uppercase()
    );
    println!("{eq}");
    println!("\nMinimum Values:");
    println!(
        "  Trace width:      {:.4} mm ({:.1} mil)",
        r.min_trace_width_mm,
        r.min_trace_width_mil()
    );
    println!(
        "  Trace spacing:    {:.4} mm ({:.1} mil)",
        r.min_clearance_mm,
        r.min_clearance_mil()
    );
    println!("  Via drill:        {} mm", f(r.min_via_drill_mm));
    println!("  Via diameter:     {} mm", f(r.min_via_diameter_mm));
    println!("  Annular ring:     {} mm", f(r.min_annular_ring_mm));
    println!("  Copper-to-edge:   {} mm", f(r.min_copper_to_edge_mm));
    println!("\nSilkscreen:");
    println!("  Min line width:   {} mm", f(r.min_silkscreen_width_mm));
    println!("  Min text height:  {} mm", f(r.min_silkscreen_height_mm));
    println!("\nBoard Specifications:");
    println!("  Thickness:        {} mm", f(r.board_thickness_mm));
    println!("  Outer copper:     {} oz", f(r.outer_copper_oz));
    if r.inner_copper_oz > 0.0 {
        println!("  Inner copper:     {} oz", f(r.inner_copper_oz));
    }
    println!("\nWebsite: {}", profile.website);
    if profile.supports_assembly {
        println!("\nAssembly: Supported");
        if let Some(lib) = profile.parts_library {
            println!("Parts Library: {lib}");
            if let Some(url) = catalog_url(lib) {
                println!("Catalog: {url}");
            }
        }
    } else {
        println!("\nAssembly: Not available (PCB only)");
    }
    println!("\n{eq}");
    Ok(())
}

/// Hint shown when failing DRC errors would pass the default fab's rules.
pub fn manufacturer_compatibility_hint(violations: &[DRCViolation], layers: i64) -> Option<String> {
    let errors: Vec<DRCViolation> = violations
        .iter()
        .filter(|v| v.is_error())
        .cloned()
        .collect();
    if errors.is_empty() {
        return None;
    }
    let temp = DRCReport::new("", "", errors);
    let checks = check_manufacturer_rules(&temp, DEFAULT_HINT_MANUFACTURER, layers, 1.0);
    if checks.is_empty() {
        return None;
    }
    let incompatible = checks.iter().filter(|c| !c.is_compatible()).count();
    let compatible = checks.len() - incompatible;
    let name = manufacturers::profile(DEFAULT_HINT_MANUFACTURER).ok()?.name;
    let m = DEFAULT_HINT_MANUFACTURER;
    if incompatible == 0 && compatible > 0 {
        return Some(format!(
            "\nNote: These {compatible} violation(s) may pass {name} manufacturing rules.\n      \
             Your board's internal rules are stricter than the manufacturer's minimums.\n      \
             Run with --mfr {m} to check compatibility:\n      kicad-drc <file> --mfr {m} -l {layers}"
        ));
    }
    if compatible > 0 && incompatible < checks.len() {
        return Some(format!(
            "\nNote: {compatible} of {} violations may pass {name} rules.\n      \
             Run with --mfr {m} to see manufacturer compatibility.",
            checks.len()
        ));
    }
    None
}

fn print_comparison(layers: i64) -> Result<()> {
    let eq = "=".repeat(70);
    let rules: Vec<(&str, DesignRules, bool)> = manufacturers::profiles()
        .iter()
        .map(|p| {
            Ok((
                p.id,
                get_design_rules(p.id, layers, 1.0)?,
                p.supports_assembly,
            ))
        })
        .collect::<Result<_>>()?;
    println!("\n{eq}");
    println!("MANUFACTURER COMPARISON - {layers}-LAYER PCB");
    println!("{eq}");
    let mut header = format!("{:<25}", "Constraint");
    for (id, _, _) in &rules {
        header += &format!("{:>12}", id.to_uppercase());
    }
    println!("{header}");
    println!("{}", "-".repeat(70));
    let row = |label: &str, prec: usize, get: &dyn Fn(&DesignRules) -> f64| {
        let mut s = format!("{label:<25}");
        for (_, r, _) in &rules {
            s += &format!("{:>12.prec$}", get(r));
        }
        println!("{s}");
    };
    row("Trace width (mil)", 1, &|r| r.min_trace_width_mil());
    row("Clearance (mil)", 1, &|r| r.min_clearance_mil());
    row("Via drill (mm)", 2, &|r| r.min_via_drill_mm);
    row("Via diameter (mm)", 2, &|r| r.min_via_diameter_mm);
    row("Copper-to-edge (mm)", 2, &|r| r.min_copper_to_edge_mm);
    println!("{}", "-".repeat(70));
    let mut s = format!("{:<25}", "Assembly");
    for (_, _, asm) in &rules {
        s += &format!("{:>12}", if *asm { "Yes" } else { "No" });
    }
    println!("{s}");
    println!("\n{eq}");
    Ok(())
}

// ---------------------------------------------------------------------------
// drc_summary (kicad-drc-summary)
// ---------------------------------------------------------------------------

/// Severity classification for structured DRC summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueSeverity {
    Blocking,
    Warning,
    FabAcceptable,
    Cosmetic,
}

/// Default severity bucket for a violation type (`SEVERITY_RULES`).
pub fn get_severity(v: &DRCViolation) -> IssueSeverity {
    use ViolationType::*;
    match v.vtype {
        SHORTING_ITEMS
        | CLEARANCE
        | COPPER_EDGE_CLEARANCE
        | DRILL_HOLE_TOO_SMALL
        | NPTH_HOLE_TOO_SMALL
        | TRACK_WIDTH
        | VIA_HOLE_LARGER_THAN_PAD => IssueSeverity::Blocking,
        SILK_OVER_COPPER | SILK_OVERLAP | SOLDER_MASK_BRIDGE => IssueSeverity::Cosmetic,
        _ => IssueSeverity::Warning,
    }
}

/// A violation compared against manufacturer limits.
#[derive(Debug, Clone)]
pub struct ManufacturerComparison {
    pub violation: DRCViolation,
    pub is_false_positive: bool,
    pub manufacturer_limit: Option<f64>,
    pub actual_value: Option<f64>,
    pub message: String,
}

/// Compare one violation against fab limits; `None` when not comparable.
pub fn compare_with_manufacturer(
    v: &DRCViolation,
    rules: &DesignRules,
    manufacturer_id: &str,
) -> Option<ManufacturerComparison> {
    use ViolationType::*;
    let actual = v.actual_value_mm?;
    let name = manufacturer_id.to_uppercase();
    let simple = |limit: f64| {
        let fp = actual >= limit;
        ManufacturerComparison {
            violation: v.clone(),
            is_false_positive: fp,
            manufacturer_limit: Some(limit),
            actual_value: Some(actual),
            message: if fp {
                format!("{name} accepts {limit:.3}mm")
            } else {
                String::new()
            },
        }
    };
    match v.vtype {
        CLEARANCE => Some(simple(rules.min_clearance_mm)),
        TRACK_WIDTH => Some(simple(rules.min_trace_width_mm)),
        VIA_ANNULAR_WIDTH => Some(simple(rules.min_annular_ring_mm)),
        COPPER_EDGE_CLEARANCE => Some(simple(rules.min_copper_to_edge_mm)),
        CLEARANCE_PAD_PAD => {
            if v.is_same_component_pad_clearance() {
                Some(ManufacturerComparison {
                    violation: v.clone(),
                    is_false_positive: true,
                    manufacturer_limit: Some(rules.min_clearance_mm),
                    actual_value: Some(actual),
                    message: format!(
                        "Same-component pad-pad clearance inherent to IC footprint ({name} accepts for IC packages)"
                    ),
                })
            } else {
                Some(simple(rules.min_clearance_mm))
            }
        }
        SOLDER_MASK_BRIDGE => {
            let limit = rules.min_solder_mask_dam_mm;
            if v.is_fine_pitch_inherent(limit) {
                Some(ManufacturerComparison {
                    violation: v.clone(),
                    is_false_positive: true,
                    manufacturer_limit: Some(limit),
                    actual_value: Some(actual),
                    message: format!(
                        "Fine-pitch IC bridge inherent to footprint ({name} accepts for fine-pitch ICs)"
                    ),
                })
            } else {
                Some(simple(limit))
            }
        }
        _ => None,
    }
}

type Counter = Vec<(String, usize)>;

fn bump(c: &mut Counter, key: &str) {
    match c.iter_mut().find(|(k, _)| k == key) {
        Some((_, n)) => *n += 1,
        None => c.push((key.to_string(), 1)),
    }
}

/// `Counter.most_common()`: count descending, insertion order on ties.
fn most_common(c: &Counter) -> Counter {
    let mut out = c.clone();
    out.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    out
}

fn counter_json(c: &Counter) -> Json {
    Json::Obj(c.iter().map(|(k, n)| (k.clone(), Json::from(*n))).collect())
}

/// Structured DRC summary with severity categorization.
#[derive(Debug, Clone, Default)]
pub struct DRCSummary {
    pub pcb_name: String,
    pub source_file: String,
    pub total_violations: usize,
    pub blocking: Vec<DRCViolation>,
    pub warnings: Vec<DRCViolation>,
    pub fab_acceptable: Vec<DRCViolation>,
    pub cosmetic: Vec<DRCViolation>,
    pub blocking_by_type: Counter,
    pub warning_by_type: Counter,
    pub fab_acceptable_by_type: Counter,
    pub cosmetic_by_type: Counter,
    pub manufacturer: Option<String>,
    pub false_positives: Vec<ManufacturerComparison>,
    pub true_violations: Vec<ManufacturerComparison>,
    pub unconnected_by_net: Counter,
}

impl DRCSummary {
    pub fn has_blocking(&self) -> bool {
        !self.blocking.is_empty()
    }

    pub fn verdict(&self) -> String {
        let mfr = self.manufacturer.as_deref().map(str::to_uppercase);
        if self.has_blocking() {
            match mfr {
                Some(m) => format!(
                    "BLOCKING - {} violation(s) fail {m} rules",
                    self.blocking.len()
                ),
                None => "BLOCKING - Fix issues before manufacturing".into(),
            }
        } else if let (true, Some(m)) = (!self.fab_acceptable.is_empty(), mfr) {
            format!(
                "FAB COMPATIBLE - All {} violation(s) acceptable for {m}",
                self.fab_acceptable.len()
            )
        } else if !self.warnings.is_empty() {
            "WARNINGS - Review before fab".into()
        } else if !self.cosmetic.is_empty() {
            "COSMETIC ONLY - Safe to manufacture".into()
        } else {
            "PASSED - No issues found".into()
        }
    }

    fn add(&mut self, v: &DRCViolation, sev: IssueSeverity) {
        let (list, counter) = match sev {
            IssueSeverity::Blocking => (&mut self.blocking, &mut self.blocking_by_type),
            IssueSeverity::FabAcceptable => {
                (&mut self.fab_acceptable, &mut self.fab_acceptable_by_type)
            }
            IssueSeverity::Warning => (&mut self.warnings, &mut self.warning_by_type),
            IssueSeverity::Cosmetic => (&mut self.cosmetic, &mut self.cosmetic_by_type),
        };
        list.push(v.clone());
        bump(counter, &v.type_str);
    }

    pub fn to_json(&self) -> Json {
        let mut d = jobj! {
            "pcb_name" => self.pcb_name.as_str(),
            "source_file" => self.source_file.as_str(),
            "total_violations" => self.total_violations,
            "verdict" => self.verdict(),
            "counts" => jobj!{
                "blocking" => self.blocking.len(),
                "warning" => self.warnings.len(),
                "fab_acceptable" => self.fab_acceptable.len(),
                "cosmetic" => self.cosmetic.len(),
            },
            "blocking" => jobj!{
                "count" => self.blocking.len(),
                "by_type" => counter_json(&self.blocking_by_type),
                "violations" => Json::Arr(self.blocking.iter().map(DRCViolation::to_json).collect()),
            },
            "warnings" => jobj!{"count" => self.warnings.len(), "by_type" => counter_json(&self.warning_by_type)},
            "fab_acceptable" => jobj!{"count" => self.fab_acceptable.len(), "by_type" => counter_json(&self.fab_acceptable_by_type)},
            "cosmetic" => jobj!{"count" => self.cosmetic.len(), "by_type" => counter_json(&self.cosmetic_by_type)},
        };
        if !self.unconnected_by_net.is_empty() {
            d.set("unconnected_by_net", counter_json(&self.unconnected_by_net));
        }
        if let Some(m) = &self.manufacturer {
            let details: Vec<Json> = self
                .false_positives
                .iter()
                .chain(&self.true_violations)
                .map(|c| {
                    jobj! {
                        "type" => c.violation.type_str.as_str(),
                        "is_false_positive" => c.is_false_positive,
                        "message" => c.message.as_str(),
                        "manufacturer_limit" => c.manufacturer_limit,
                        "actual_value" => c.actual_value,
                    }
                })
                .collect();
            d.set(
                "manufacturer",
                jobj! {
                    "id" => m.as_str(),
                    "false_positives" => self.false_positives.len(),
                    "true_violations" => self.true_violations.len(),
                    "details" => Json::Arr(details),
                },
            );
        }
        d
    }
}

/// Build a structured summary; with a manufacturer, violations within fab
/// limits are reclassified as fab-acceptable.
pub fn create_summary(
    report: &DRCReport,
    manufacturer_id: Option<&str>,
    layers: i64,
) -> Result<DRCSummary> {
    let mut s = DRCSummary {
        pcb_name: report.pcb_name.clone(),
        source_file: report.source_file.clone(),
        total_violations: report.violation_count(),
        manufacturer: manufacturer_id.map(str::to_string),
        ..Default::default()
    };
    let rules = match manufacturer_id {
        Some(m) => Some(get_design_rules(
            manufacturers::profile(m)?.id,
            layers,
            1.0,
        )?),
        None => None,
    };
    for v in &report.violations {
        let sev = get_severity(v);
        if v.vtype == ViolationType::UNCONNECTED_ITEMS {
            for net in &v.nets {
                bump(&mut s.unconnected_by_net, net);
            }
        }
        let cmp = rules
            .as_ref()
            .zip(manufacturer_id)
            .and_then(|(r, m)| compare_with_manufacturer(v, r, m));
        match cmp {
            Some(c) if c.is_false_positive => {
                s.false_positives.push(c);
                s.add(v, IssueSeverity::FabAcceptable);
            }
            Some(c) => {
                s.true_violations.push(c);
                s.add(v, sev);
            }
            None => s.add(v, sev),
        }
    }
    Ok(s)
}

fn fab_annotation(type_str: &str, fps: &[ManufacturerComparison], mfr: &str) -> String {
    fps.iter()
        .find(|fp| fp.violation.type_str == type_str && fp.actual_value.is_some())
        .map(|fp| {
            format!(
                " ({:.3}mm >= {} min {:.3}mm)",
                fp.actual_value.unwrap_or_default(),
                mfr.to_uppercase(),
                fp.manufacturer_limit.unwrap_or_default()
            )
        })
        .unwrap_or_default()
}

/// Render a [`DRCSummary`] as the upstream table.
pub fn summary_table(s: &DRCSummary, blocking_only: bool) -> String {
    use std::fmt::Write as _;
    let mut o = String::new();
    let title = if s.source_file.is_empty() {
        &s.pcb_name
    } else {
        &s.source_file
    };
    let _ = writeln!(o, "\nDRC Summary: {title}");
    let _ = writeln!(o, "{}", "=".repeat(60));
    let _ = writeln!(o, "\nBLOCKING ({} violations):", s.blocking.len());
    if s.blocking.is_empty() {
        let _ = writeln!(o, "  (none)");
    } else {
        for (t, n) in most_common(&s.blocking_by_type) {
            let _ = writeln!(o, "  X {n} {t}");
        }
    }
    if blocking_only {
        let _ = writeln!(o, "\n{}", "=".repeat(60));
        let _ = writeln!(o, "VERDICT: {}", s.verdict());
        return o;
    }
    if let (Some(m), false) = (&s.manufacturer, s.fab_acceptable.is_empty()) {
        let _ = writeln!(
            o,
            "\nFAB-ACCEPTABLE ({} violations for {}):",
            s.fab_acceptable.len(),
            m.to_uppercase()
        );
        for (t, n) in most_common(&s.fab_acceptable_by_type) {
            let _ = writeln!(
                o,
                "  ~ {n} {t}{}",
                fab_annotation(&t, &s.false_positives, m)
            );
        }
    }
    if !s.warnings.is_empty() {
        let _ = writeln!(o, "\nWARNINGS ({} violations):", s.warnings.len());
        for (t, n) in most_common(&s.warning_by_type) {
            let _ = writeln!(o, "  ! {n} {t}");
        }
        if !s.unconnected_by_net.is_empty() {
            let top: Vec<String> = most_common(&s.unconnected_by_net)
                .into_iter()
                .take(5)
                .map(|(net, n)| format!("{net}: {n}"))
                .collect();
            let _ = writeln!(o, "      {}", top.join(", "));
        }
    }
    if !s.cosmetic.is_empty() {
        let _ = writeln!(o, "\nCOSMETIC ({} violations):", s.cosmetic.len());
        for (t, n) in most_common(&s.cosmetic_by_type) {
            let _ = writeln!(o, "  o {n} {t}");
        }
    }
    if let Some(m) = &s.manufacturer {
        let non_blocking = s.fab_acceptable.len() + s.warnings.len() + s.cosmetic.len();
        if non_blocking > 0 && !s.blocking.is_empty() {
            let _ = writeln!(
                o,
                "\n  Only {} of {} violations are blocking for {}",
                s.blocking.len(),
                s.total_violations,
                m.to_uppercase()
            );
        }
    }
    let _ = writeln!(o, "\n{}", "=".repeat(60));
    let _ = writeln!(o, "VERDICT: {}", s.verdict());
    o
}

#[derive(Parser, Debug)]
#[command(about = "Structured DRC summary with severity levels")]
struct SummaryArgs {
    /// PCB (.kicad_pcb) to check or DRC report (.json/.rpt) to parse
    input: PathBuf,
    /// Compare against manufacturer rules
    #[arg(short = 'f', long = "fab", value_parser = clap::builder::PossibleValuesParser::new(manufacturers::profiles().iter().map(|p| p.id).collect::<Vec<_>>()))]
    manufacturer: Option<String>,
    #[arg(short = 'l', long, default_value_t = 2)]
    layers: i64,
    #[arg(long, default_value = "table", value_parser = ["table", "json"])]
    format: String,
    /// Only show blocking issues
    #[arg(long)]
    blocking_only: bool,
    /// Exit with code 2 on warnings (when no blocking issues)
    #[arg(long)]
    strict: bool,
    /// Keep the DRC report file after running (for PCB input)
    #[arg(long)]
    keep_report: bool,
}

/// `kicad-drc-summary` entry point (not a registered `kct` command upstream;
/// exposed for callers such as `audit`).
pub fn summary_main(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let args: SummaryArgs = parse_args("drc-summary", args);
    let ext = suffix(&args.input);
    let report = if ext == ".kicad_pcb" {
        match run_drc_on_pcb(&args.input, None, args.keep_report) {
            Some(r) => r,
            None => return Ok(1),
        }
    } else if ext == ".json" || ext == ".rpt" {
        if !args.input.exists() {
            eprintln!("Error: File not found: {}", args.input.display());
            return Ok(1);
        }
        match DRCReport::load(&args.input) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error loading report: {e}");
                return Ok(1);
            }
        }
    } else {
        eprintln!("Error: Unsupported file type: {ext}");
        eprintln!("Expected .kicad_pcb (PCB) or .json/.rpt (report)");
        return Ok(1);
    };
    let s = create_summary(&report, args.manufacturer.as_deref(), args.layers)?;
    if args.format == "json" {
        println!("{}", dumps_indent(&s.to_json(), 2));
    } else {
        print!("{}", summary_table(&s, args.blocking_only));
    }
    Ok(if s.has_blocking() {
        1
    } else if !s.warnings.is_empty() && args.strict {
        2
    } else {
        0
    })
}
