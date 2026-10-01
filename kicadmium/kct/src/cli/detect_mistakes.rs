//! `kct detect-mistakes` (port of `kicad_tools.cli.mistakes_cmd`): detect
//! common PCB design mistakes with educational explanations.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use clap::Parser;

use super::Globals;
use crate::explain::mistakes::{get_default_checks, severity_rank, CheckCoverage, Mistake, MistakeCategory, MistakeDetector};
use crate::pyjson::{dumps_indent, Json};
use crate::schema::pcb::Pcb;

const CATEGORY_VALUES: [&str; 12] = [
    "bypass_capacitor",
    "crystal_oscillator",
    "differential_pair",
    "power_trace",
    "thermal_management",
    "emi_shielding",
    "decoupling",
    "grounding",
    "via_placement",
    "manufacturability",
    "connectivity",
    "bom_health",
];

#[derive(Parser, Debug)]
#[command(about = "Detect common PCB design mistakes with educational explanations")]
struct Args {
    /// Path to .kicad_pcb file to analyze
    pcb_file: Option<String>,
    /// Only check specific category
    #[arg(long, short = 'c', value_parser = CATEGORY_VALUES)]
    category: Option<String>,
    /// Only show issues of this severity or higher
    #[arg(long, short = 's', value_parser = ["error", "warning", "info"])]
    severity: Option<String>,
    /// Output format (default: table)
    #[arg(long, short = 'f', default_value = "table", value_parser = ["table", "json", "tree", "summary"])]
    format: String,
    /// Exit with error code on warnings
    #[arg(long)]
    strict: bool,
    /// List available check categories and exit
    #[arg(long = "list-categories")]
    list_categories: bool,
    /// Show detailed information
    #[arg(long, short = 'v')]
    verbose: bool,
}

fn category_description(c: MistakeCategory) -> &'static str {
    match c {
        MistakeCategory::BypassCap => "Bypass capacitor placement issues",
        MistakeCategory::Crystal => "Crystal oscillator layout problems",
        MistakeCategory::DifferentialPair => "Differential pair routing issues",
        MistakeCategory::PowerTrace => "Power trace width problems",
        MistakeCategory::Thermal => "Thermal management issues",
        MistakeCategory::Emi => "EMI and shielding concerns",
        MistakeCategory::Decoupling => "Decoupling capacitor issues",
        MistakeCategory::Grounding => "Grounding and return path issues",
        MistakeCategory::Via => "Via placement problems",
        MistakeCategory::Manufacturability => "Manufacturing-related issues",
        MistakeCategory::Connectivity => "Pull-up / series-resistor connectivity issues",
        MistakeCategory::BomHealth => "BOM part-number health issues",
    }
}

fn list_categories() -> i32 {
    println!("Available Mistake Categories:");
    println!("{}", "=".repeat(50));
    let checks = get_default_checks();
    for cat in MistakeCategory::ALL {
        let in_cat: Vec<_> = checks.iter().filter(|c| c.category() == cat).collect();
        println!("\n{}", cat.value());
        println!("  {}", category_description(cat));
        println!("  Checks: {}", in_cat.len());
        for c in in_cat {
            println!("    - {}", c.name());
        }
    }
    println!();
    println!("Total categories: {}", MistakeCategory::ALL.len());
    println!("Total checks: {}", checks.len());
    0
}

fn count(m: &[Mistake], sev: &str) -> usize {
    m.iter().filter(|x| x.severity == sev).count()
}

fn print_mistake(m: &Mistake, verbose: bool) {
    let symbol = match m.severity.as_str() {
        "error" => "X",
        "warning" => "!",
        "info" => "i",
        _ => "?",
    };
    println!("\n  [{symbol}] {}", m.title);
    println!("      Components: {}", m.components.join(", "));
    if let Some((x, y)) = m.location {
        println!("      Location: ({x:.2}, {y:.2}) mm");
    }
    if verbose {
        println!("      Problem: {}", m.explanation);
        println!("      Fix: {}", m.fix_suggestion);
        if let Some(u) = m.learn_more_url.as_deref().filter(|u| !u.is_empty()) {
            println!("      Learn more: {u}");
        }
    }
}

fn output_table(mistakes: &[Mistake], verbose: bool) {
    let rule60 = "=".repeat(60);
    let dash60 = "-".repeat(60);
    if mistakes.is_empty() {
        println!("\n{rule60}");
        println!("NO DESIGN MISTAKES DETECTED");
        println!("{rule60}");
        println!("Your PCB passed all checks!");
        return;
    }
    println!("\n{rule60}");
    println!("PCB DESIGN MISTAKE ANALYSIS");
    println!("{rule60}");
    println!("\nSummary:");
    println!("  Errors:   {}", count(mistakes, "error"));
    println!("  Warnings: {}", count(mistakes, "warning"));
    println!("  Info:     {}", count(mistakes, "info"));
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for m in mistakes {
        *by.entry(m.category.value()).or_default() += 1;
    }
    println!("\n{dash60}");
    println!("BY CATEGORY:");
    for (cat, n) in &by {
        println!("  {cat}: {n} issue(s)");
    }
    let errors: Vec<&Mistake> = mistakes.iter().filter(|m| m.severity == "error").collect();
    let warnings: Vec<&Mistake> = mistakes.iter().filter(|m| m.severity == "warning").collect();
    let infos: Vec<&Mistake> = mistakes.iter().filter(|m| m.severity == "info").collect();
    if !errors.is_empty() {
        println!("\n{dash60}");
        println!("ERRORS (must fix):");
        for m in &errors {
            print_mistake(m, verbose);
        }
    }
    if !warnings.is_empty() {
        println!("\n{dash60}");
        println!("WARNINGS (should review):");
        let shown = if verbose { warnings.len() } else { warnings.len().min(10) };
        for m in &warnings[..shown] {
            print_mistake(m, verbose);
        }
        if warnings.len() > 10 && !verbose {
            println!("\n  ... and {} more warnings (use --verbose)", warnings.len() - 10);
        }
    }
    if !infos.is_empty() && verbose {
        println!("\n{dash60}");
        println!("INFO (suggestions):");
        for m in &infos {
            print_mistake(m, verbose);
        }
    }
    println!("\n{rule60}");
    if !errors.is_empty() {
        println!("FIX ERRORS BEFORE MANUFACTURING");
    } else if !warnings.is_empty() {
        println!("REVIEW WARNINGS FOR BEST RESULTS");
    } else {
        println!("DESIGN LOOKS GOOD!");
    }
}

fn output_json(mistakes: &[Mistake], coverage: &[CheckCoverage]) {
    let data = crate::jobj! {
        "summary" => crate::jobj! {
            "errors" => count(mistakes, "error") as i64,
            "warnings" => count(mistakes, "warning") as i64,
            "info" => count(mistakes, "info") as i64,
        },
        "mistakes" => Json::Arr(mistakes.iter().map(Mistake::to_dict).collect()),
        "coverage" => Json::Arr(coverage.iter().map(CheckCoverage::to_dict).collect()),
        "coverage_complete" => coverage.iter().all(|c| c.status == "ran"),
    };
    println!("{}", dumps_indent(&data, 2));
}

fn output_tree(mistakes: &[Mistake]) {
    if mistakes.is_empty() {
        println!("No design mistakes detected.");
        return;
    }
    for m in mistakes {
        println!("{}", m.format_tree());
        println!();
    }
}

fn output_summary(mistakes: &[Mistake]) {
    println!(
        "Errors: {}, Warnings: {}, Info: {}",
        count(mistakes, "error"),
        count(mistakes, "warning"),
        count(mistakes, "info")
    );
    if mistakes.is_empty() {
        println!("No design mistakes detected!");
        return;
    }
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for m in mistakes {
        *by.entry(m.category.value()).or_default() += 1;
    }
    for (cat, n) in by {
        println!("  {cat}: {n}");
    }
}

/// `kct detect-mistakes` entry point.
pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let a: Args = super::parse_args("detect-mistakes", args);
    if a.list_categories {
        return Ok(list_categories());
    }
    let Some(pcb_file) = &a.pcb_file else {
        use clap::CommandFactory;
        Args::command().name("kct detect-mistakes").print_help()?;
        eprintln!("\nError: pcb_file required");
        return Ok(1);
    };
    let path = Path::new(pcb_file);
    if !path.exists() {
        eprintln!("Error: File not found: {}", path.display());
        return Ok(1);
    }
    if path.extension().and_then(|e| e.to_str()) != Some("kicad_pcb") {
        // Python `Path.suffix`: "" when there is no extension.
        let suffix = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.rfind('.').filter(|i| *i > 0).map(|i| &n[i..]))
            .unwrap_or("");
        eprintln!("Error: Expected .kicad_pcb file, got {suffix}");
        return Ok(1);
    }
    println!(
        "Analyzing: {}",
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    );
    let pcb = match Pcb::load(path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error loading PCB: {e}");
            return Ok(1);
        }
    };
    let detector = MistakeDetector::default();
    let (mut mistakes, coverage) = match a.category.as_deref().and_then(MistakeCategory::from_value) {
        Some(cat) => detector.detect_by_category_with_coverage(&pcb, cat),
        None => detector.detect_with_coverage(&pcb),
    };
    if let Some(sev) = &a.severity {
        let min = severity_rank(sev);
        mistakes.retain(|m| severity_rank(&m.severity) <= min);
    }
    match a.format.as_str() {
        "json" => output_json(&mistakes, &coverage),
        "tree" => output_tree(&mistakes),
        "summary" => output_summary(&mistakes),
        _ => output_table(&mistakes, a.verbose),
    }
    let incomplete: Vec<&CheckCoverage> = coverage.iter().filter(|c| c.status == "incomplete").collect();
    if !incomplete.is_empty() && a.format != "json" {
        println!("\n{}", "-".repeat(60));
        println!(
            "INCOMPLETE COVERAGE: {} check(s) could not run:",
            incomplete.len()
        );
        for c in &incomplete {
            println!(
                "  - {} ({}): {}",
                c.check_name,
                c.category.value(),
                c.reason.as_deref().unwrap_or("None")
            );
        }
    }
    if count(&mistakes, "error") > 0 {
        return Ok(1);
    }
    if (count(&mistakes, "warning") > 0 || !incomplete.is_empty()) && a.strict {
        return Ok(2);
    }
    Ok(0)
}
