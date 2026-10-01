//! `kct erc {parse,explain}` (port of `kicad_tools.cli.erc_cmd`,
//! `cli.erc_explain_cmd`, and the `kct erc <file>` -> `kct erc parse <file>`
//! back-compat shim from `cli/__init__.py`).
//!
//! `parse` accepts the full `erc_cmd` flag set (a superset of what the outer
//! upstream parser forwarded).

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};

use super::runner::{find_kicad_cli, run_erc};
use super::{parse_args, Globals};
use crate::erc::cross_sheet::{
    check_cross_sheet_duplicates, filter_cross_sheet_global_labels_objs, gather_hierarchical_labels,
};
use crate::erc::violation::{erc_type_description, ERCViolation, ERCViolationType, ERC_CATEGORIES};
use crate::erc::ERCReport;
use crate::jobj;
use crate::pyjson::{dumps_indent, py_round, Json};
use crate::validate::filters::{load_filters_from_toml, FilterEngine, FilterLoadError};

#[derive(Parser, Debug)]
#[command(about = "Run ERC on schematics or analyze ERC reports")]
struct Args {
    #[command(subcommand)]
    command: ErcCommand,
}

#[derive(Subcommand, Debug)]
enum ErcCommand {
    /// Parse ERC report (default command)
    Parse(ParseArgs),
    /// Detailed ERC error analysis with root cause and fixes
    Explain(ExplainArgs),
}

#[derive(Parser, Debug)]
struct ParseArgs {
    /// Schematic (.kicad_sch) to check or ERC report (.json/.rpt) to parse
    input: Option<PathBuf>,
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
    /// Filter by sheet path
    #[arg(long)]
    sheet: Option<String>,
    /// Group violations by sheet
    #[arg(long)]
    by_sheet: bool,
    /// Show detailed violation information
    #[arg(short = 'v', long)]
    verbose: bool,
    /// TOML file with [[erc.filters]] rules to suppress or reclassify violations
    #[arg(long)]
    filter_config: Option<PathBuf>,
    /// Keep the ERC report file after running
    #[arg(long)]
    keep_report: bool,
    /// Save ERC report to this path
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
    /// List all known ERC violation types and exit
    #[arg(long)]
    list_types: bool,
}

#[derive(Parser, Debug)]
struct ExplainArgs {
    /// Schematic (.kicad_sch) to check or ERC report (.json/.rpt) to analyze
    input: PathBuf,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
    /// Show only errors, not warnings
    #[arg(long)]
    errors_only: bool,
    /// Filter by violation type
    #[arg(short = 't', long = "type")]
    filter_type: Option<String>,
    /// Keep the ERC report file after running
    #[arg(long)]
    keep_report: bool,
}

pub fn run(mut args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    // Back-compat: `kct erc <file>` -> `kct erc parse <file>`.
    if let Some(first) = args.first().and_then(|a| a.to_str()) {
        if !matches!(first, "parse" | "explain" | "-h" | "--help") {
            args.insert(0, "parse".into());
        }
    }
    let args: Args = parse_args("erc", args);
    match args.command {
        ErcCommand::Parse(a) => run_parse(a),
        ErcCommand::Explain(a) => run_explain(a),
    }
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

fn run_parse(args: ParseArgs) -> Result<i32> {
    if args.list_types {
        print_types();
        return Ok(0);
    }
    let Some(input) = args.input.clone() else {
        use clap::CommandFactory;
        let _ = ParseArgs::command().name("kct erc parse").print_help();
        eprintln!("\nError: input file required");
        return Ok(1);
    };
    let ext = suffix(&input);
    let mut schematic_for_filter = None;
    let mut report = if ext == ".kicad_sch" {
        let Some(mut r) = run_erc_on_schematic(&input, args.output.as_deref(), args.keep_report)
        else {
            return Ok(1);
        };
        schematic_for_filter = Some(input.clone());
        r.violations.extend(check_cross_sheet_duplicates(&input));
        r
    } else if ext == ".json" || ext == ".rpt" {
        if !input.exists() {
            eprintln!("Error: File not found: {}", input.display());
            return Ok(1);
        }
        match ERCReport::load(&input) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error loading report: {e}");
                return Ok(1);
            }
        }
    } else {
        eprintln!("Error: Unsupported file type: {ext}");
        eprintln!("Expected .kicad_sch (schematic) or .json/.rpt (report)");
        return Ok(1);
    };

    if let Some(sch) = &schematic_for_filter {
        report.violations =
            filter_cross_sheet_global_labels_objs(std::mem::take(&mut report.violations), sch);
    }

    let mut filtered = 0;
    if let Some(cfg) = &args.filter_config {
        match load_filters_from_toml(cfg) {
            Ok((_, erc_filters)) => {
                if !erc_filters.is_empty() {
                    let result = FilterEngine::new(erc_filters).apply(&report.violations);
                    filtered = result.ignored_count();
                    report.violations = result.kept;
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

    let mut violations: Vec<ERCViolation> = report
        .violations
        .iter()
        .filter(|v| !v.excluded)
        .cloned()
        .collect();
    if args.errors_only {
        violations.retain(ERCViolation::is_error);
    }
    if let Some(t) = &args.filter_type {
        let f = t.to_lowercase();
        violations.retain(|v| {
            v.type_str.to_lowercase().contains(&f)
                || v.description.to_lowercase().contains(&f)
                || v.type_description().to_lowercase().contains(&f)
        });
    }
    if let Some(sheet) = &args.sheet {
        violations.retain(|v| v.sheet.contains(sheet.as_str()));
    }

    match args.format.as_str() {
        "json" => output_json(&violations, &report, filtered),
        "summary" => output_summary(&violations, &report, filtered),
        _ => output_table(&violations, &report, args.verbose, args.by_sheet, filtered),
    }

    let errors = violations.iter().filter(|v| v.is_error()).count();
    let warnings = violations.len() - errors;
    Ok(if errors > 0 {
        1
    } else if warnings > 0 && args.strict {
        2
    } else {
        0
    })
}

/// Run kicad-cli ERC on a schematic and parse the JSON report.
pub fn run_erc_on_schematic(
    schematic: &Path,
    output: Option<&Path>,
    keep_report: bool,
) -> Option<ERCReport> {
    if !schematic.exists() {
        eprintln!("Error: Schematic not found: {}", schematic.display());
        return None;
    }
    if find_kicad_cli().is_none() {
        eprintln!("Error: kicad-cli not found");
        eprintln!("Install KiCad 8 from: https://www.kicad.org/download/");
        eprintln!("\nmacOS: brew install --cask kicad");
        return None;
    }
    println!(
        "Running ERC on: {}",
        schematic.file_name().unwrap_or_default().to_string_lossy()
    );
    let result = run_erc(schematic, output, "json", true, None);
    if !result.success {
        eprintln!("Error running ERC: {}", result.stderr);
        return None;
    }
    let out = result.output_path?;
    let report = match ERCReport::load(&out) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error parsing ERC report: {e}");
            return None;
        }
    };
    if !keep_report && output.is_none() {
        let _ = std::fs::remove_file(&out);
    }
    Some(report)
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n != 1 { "s" } else { "" })
}

fn count_by_type(violations: &[ERCViolation]) -> Vec<(String, usize, usize)> {
    let mut out: Vec<(String, usize, usize)> = Vec::new();
    for v in violations {
        let idx = match out.iter().position(|(t, _, _)| *t == v.type_str) {
            Some(i) => i,
            None => {
                out.push((v.type_str.clone(), 0, 0));
                out.len() - 1
            }
        };
        if v.is_error() {
            out[idx].1 += 1;
        } else {
            out[idx].2 += 1;
        }
    }
    out
}

fn output_table(
    violations: &[ERCViolation],
    report: &ERCReport,
    verbose: bool,
    by_sheet: bool,
    filtered: usize,
) {
    let errors_n = violations.iter().filter(|v| v.is_error()).count();
    let warnings_n = violations.len() - errors_n;
    let eq = "=".repeat(60);
    let dash = "-".repeat(60);
    println!("\n{eq}");
    println!("ERC VALIDATION SUMMARY");
    println!("{eq}");
    if !report.source_file.is_empty() {
        println!("File: {}", file_name(&report.source_file));
    }
    if !report.kicad_version.is_empty() {
        println!("KiCad: {}", report.kicad_version);
    }
    println!("\nResults:");
    println!("  Errors:     {errors_n}");
    println!("  Warnings:   {warnings_n}");
    if filtered > 0 {
        println!("  Filtered:   {filtered} violations filtered");
    }
    if report.exclusion_count() > 0 {
        println!("  Excluded:   {} (not counted)", report.exclusion_count());
    }
    if violations.is_empty() {
        println!("\n{eq}");
        println!("ERC PASSED - No violations found");
        return;
    }
    let mut by_type = count_by_type(violations);
    by_type.sort_by_key(|(_, e, w)| std::cmp::Reverse(e + w));
    println!("\n{dash}");
    println!("BY TYPE:");
    for (t, e, w) in &by_type {
        let desc = erc_type_description(t).unwrap_or(t);
        let mut parts = Vec::new();
        if *e > 0 {
            parts.push(plural(*e, "error"));
        }
        if *w > 0 {
            parts.push(plural(*w, "warning"));
        }
        println!("  {desc}: {}", parts.join(", "));
    }
    let errors: Vec<&ERCViolation> = violations.iter().filter(|v| v.is_error()).collect();
    let warnings: Vec<&ERCViolation> = violations.iter().filter(|v| !v.is_error()).collect();
    if !errors.is_empty() {
        println!("\n{dash}");
        println!("ERRORS (must fix):");
        print_violations(&errors, verbose, by_sheet);
    }
    if !warnings.is_empty() {
        println!("\n{dash}");
        println!("WARNINGS (review recommended):");
        let shown = if verbose {
            &warnings[..]
        } else {
            &warnings[..warnings.len().min(20)]
        };
        print_violations(shown, verbose, by_sheet);
        if warnings.len() > 20 && !verbose {
            println!(
                "\n  ... and {} more warnings (use --verbose)",
                warnings.len() - 20
            );
        }
    }
    println!("\n{eq}");
    if !errors.is_empty() {
        println!("ERC FAILED - Fix errors before proceeding");
    } else {
        println!("ERC WARNING - Review warnings");
    }
}

fn print_violations(violations: &[&ERCViolation], verbose: bool, by_sheet: bool) {
    if by_sheet {
        let mut grouped: Vec<(String, Vec<&ERCViolation>)> = Vec::new();
        for v in violations {
            let sheet = if v.sheet.is_empty() { "root" } else { &v.sheet };
            match grouped.iter_mut().find(|(s, _)| s == sheet) {
                Some((_, l)) => l.push(v),
                None => grouped.push((sheet.to_string(), vec![v])),
            }
        }
        grouped.sort_by(|a, b| a.0.cmp(&b.0));
        for (sheet, list) in grouped {
            println!("\n  [{sheet}]");
            for v in list {
                print_single(v, verbose, "    ");
            }
        }
    } else {
        for v in violations {
            print_single(v, verbose, "  ");
        }
    }
}

fn print_single(v: &ERCViolation, verbose: bool, indent: &str) {
    let symbol = if v.is_error() { "X" } else { "!" };
    println!("\n{indent}[{symbol}] {}", v.type_description());
    println!("{indent}    {}", v.description);
    if verbose {
        for item in &v.items {
            println!("{indent}    -> {item}");
        }
        let loc = v.location_str();
        if !loc.is_empty() {
            println!("{indent}    Location: {loc}");
        }
        if !v.suggestions.is_empty() {
            println!("{indent}    Suggestions:");
            for s in v.suggestions.iter().take(3) {
                println!("{indent}      - {s}");
            }
        }
    }
}

fn output_json(violations: &[ERCViolation], report: &ERCReport, filtered: usize) {
    let mut summary = jobj! {
        "errors" => violations.iter().filter(|v| v.is_error()).count(),
        "warnings" => violations.iter().filter(|v| !v.is_error()).count(),
    };
    if filtered > 0 {
        summary.set("filtered", filtered);
    }
    let data = jobj! {
        "source" => report.source_file.as_str(),
        "kicad_version" => report.kicad_version.as_str(),
        "summary" => summary,
        "violations" => Json::Arr(violations.iter().map(ERCViolation::to_json).collect()),
    };
    println!("{}", dumps_indent(&data, 2));
}

fn output_summary(violations: &[ERCViolation], report: &ERCReport, filtered: usize) {
    if violations.is_empty() {
        if filtered > 0 {
            println!("No ERC violations found ({filtered} violations filtered).");
        } else {
            println!("No ERC violations found.");
        }
        return;
    }
    println!("ERC Summary: {}", report.source_file);
    println!("{}", "=".repeat(50));
    let mut by_type = count_by_type(violations);
    by_type.sort_by(|a, b| a.0.cmp(&b.0));
    println!("{:<35} {:<8} {:<8}", "Type", "Errors", "Warnings");
    println!("{}", "-".repeat(50));
    let (mut te, mut tw) = (0, 0);
    for (t, e, w) in &by_type {
        println!("{t:<35} {e:<8} {w:<8}");
        te += e;
        tw += w;
    }
    println!("{}", "-".repeat(50));
    println!("{:<35} {te:<8} {tw:<8}", "TOTAL");
}

fn print_types() {
    println!("\nKnown ERC Violation Types:");
    println!("{}", "=".repeat(60));
    for (category, types) in ERC_CATEGORIES {
        println!("\n{category}:");
        for t in *types {
            println!("  {t:30} {}", erc_type_description(t).unwrap_or(t));
        }
    }
}

// ---------------------------------------------------------------------------
// erc explain
// ---------------------------------------------------------------------------

/// `difflib.SequenceMatcher(None, a, b).ratio()` (no junk; autojunk only
/// applies to sequences of 200+ items, which are popular-element pruned).
pub fn sequence_ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    // b2j with autojunk popularity pruning.
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, c) in b.iter().enumerate() {
        b2j.entry(*c).or_default().push(j);
    }
    if b.len() >= 200 {
        let ntest = b.len() / 100 + 1;
        b2j.retain(|_, idxs| idxs.len() <= ntest);
    }
    let find_longest = |alo: usize, ahi: usize, blo: usize, bhi: usize| -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, ch) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(idxs) = b2j.get(ch) {
                for &j in idxs {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        let isjunk_b = |j: usize| !b2j.contains_key(&b[j]);
        // Extend with non-popular (junk) matching elements as difflib does.
        while besti > alo && bestj > blo && !isjunk_b(bestj - 1) && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && !isjunk_b(bestj + bestsize)
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        while besti > alo && bestj > blo && isjunk_b(bestj - 1) && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && isjunk_b(bestj + bestsize)
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    };
    let mut matches = 0usize;
    let mut queue = vec![(0, a.len(), 0, b.len())];
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = find_longest(alo, ahi, blo, bhi);
        if k > 0 {
            matches += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    2.0 * matches as f64 / total as f64
}

/// A single diagnostic check result.
#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosisItem {
    pub check: String,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub status: String,
}

fn diag(
    check: &str,
    expected: Option<String>,
    actual: Option<String>,
    status: &str,
) -> DiagnosisItem {
    DiagnosisItem {
        check: check.into(),
        expected,
        actual,
        status: status.into(),
    }
}

/// A similar label that might be a typo.
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarLabel {
    pub name: String,
    pub similarity: f64,
    pub location: String,
    pub direction: String,
}

/// An actionable fix suggestion.
#[derive(Debug, Clone, PartialEq)]
pub struct ExplainFix {
    pub description: String,
    pub command: Option<String>,
    pub priority: i64,
}

fn fix(description: impl Into<String>, priority: i64) -> ExplainFix {
    ExplainFix {
        description: description.into(),
        command: None,
        priority,
    }
}

/// Detailed explanation of an ERC violation.
#[derive(Debug, Clone, PartialEq)]
pub struct ViolationExplanation {
    pub violation: ERCViolation,
    pub summary: String,
    pub diagnosis: Vec<DiagnosisItem>,
    pub possible_causes: Vec<String>,
    pub similar_labels: Vec<SimilarLabel>,
    pub fixes: Vec<ExplainFix>,
    pub related_violations: Vec<String>,
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

impl ViolationExplanation {
    fn new(v: &ERCViolation, summary: impl Into<String>) -> Self {
        ViolationExplanation {
            violation: v.clone(),
            summary: summary.into(),
            diagnosis: Vec::new(),
            possible_causes: Vec::new(),
            similar_labels: Vec::new(),
            fixes: Vec::new(),
            related_violations: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Json {
        let v = &self.violation;
        let (x, y) = v.pos_json();
        jobj! {
            "type" => v.type_str.as_str(),
            "type_description" => v.type_description(),
            "severity" => v.severity.value(),
            "description" => v.description.as_str(),
            "location" => jobj!{
                "sheet" => if v.sheet.is_empty() { "/" } else { v.sheet.as_str() },
                "x" => x,
                "y" => y,
            },
            "items" => &v.items,
            "summary" => self.summary.as_str(),
            "diagnosis" => Json::Arr(self.diagnosis.iter().map(|d| jobj!{
                "check" => d.check.as_str(),
                "expected" => d.expected.clone(),
                "actual" => d.actual.clone(),
                "status" => d.status.as_str(),
            }).collect()),
            "possible_causes" => &self.possible_causes,
            "similar_labels" => Json::Arr(self.similar_labels.iter().map(|s| jobj!{
                "name" => s.name.as_str(),
                "similarity" => py_round(s.similarity, 2),
                "location" => s.location.as_str(),
                "direction" => s.direction.as_str(),
            }).collect()),
            "fixes" => Json::Arr(self.fixes.iter().map(|f| jobj!{
                "description" => f.description.as_str(),
                "command" => f.command.clone(),
                "priority" => f.priority,
            }).collect()),
            "related_violations" => &self.related_violations,
        }
    }
}

/// Python f-string of `Optional[str]` (`None` renders as "None").
fn opt(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("None")
}

/// Generates detailed explanations for ERC violations.
#[derive(Debug, Clone, Default)]
pub struct ERCExplainer {
    pub schematic_path: Option<PathBuf>,
    pub all_labels: Vec<String>,
}

impl ERCExplainer {
    pub fn new(schematic_path: Option<PathBuf>, all_labels: Vec<String>) -> Self {
        ERCExplainer {
            schematic_path,
            all_labels,
        }
    }

    pub fn explain(&self, v: &ERCViolation) -> ViolationExplanation {
        use ERCViolationType::*;
        match v.vtype {
            HIER_LABEL_MISMATCH => self.hier_label_mismatch(v),
            PIN_NOT_CONNECTED => pin_not_connected(v),
            PIN_NOT_DRIVEN => simple(
                v,
                "Input pin is connected but not driven by any output",
                diag(
                    "Input pin connection",
                    Some("Driven by output/bidirectional pin".into()),
                    Some("No driver found on net".into()),
                    "error",
                ),
                &[
                    "Output pin not connected to the same net",
                    "Net has only input pins (no driver)",
                    "Missing pull-up/pull-down resistor for floating inputs",
                    "Symbol pin type incorrectly set to 'input'",
                ],
                &[
                    "Connect an output or bidirectional pin to drive this input",
                    "Add a pull-up or pull-down resistor if floating is acceptable",
                    "Check symbol pin electrical type in symbol editor",
                ],
            ),
            POWER_PIN_NOT_DRIVEN => power_not_driven(v),
            LABEL_DANGLING => {
                let name = extract_label_name(v);
                simple(
                    v,
                    format!("Label '{}' is not connected to anything", opt(&name)),
                    diag(
                        "Label connection",
                        Some("Connected to wire or pin".into()),
                        Some("No connection found".into()),
                        "warning",
                    ),
                    &[
                        "Label placed but wire not connected",
                        "Wire was deleted leaving orphaned label",
                        "Label endpoint doesn't touch wire (grid alignment)",
                        "Label intended for future connection",
                    ],
                    &[
                        "Connect a wire to the label",
                        "Delete the label if not needed",
                        "Move label to connect to existing wire",
                    ],
                )
            }
            GLOBAL_LABEL_DANGLING => self.global_label_dangling(v),
            SIMILAR_LABELS => similar_labels(v),
            DUPLICATE_REFERENCE => duplicate_reference(v),
            WIRE_DANGLING => simple(
                v,
                "Wire endpoint is not connected to anything",
                diag(
                    "Wire endpoint connection",
                    Some("Connected to pin, junction, or label".into()),
                    Some("Dangling endpoint".into()),
                    "warning",
                ),
                &[
                    "Wire drawn but not completed to destination",
                    "Symbol moved leaving wire behind",
                    "Wire endpoint off-grid (doesn't touch pin)",
                ],
                &[
                    "Extend wire to connect to destination pin",
                    "Delete the dangling wire segment",
                    "Add No-Connect flag if intentionally unconnected",
                ],
            ),
            NO_CONNECT_CONNECTED => simple(
                v,
                "No-connect flag is connected to something",
                diag(
                    "No-connect usage",
                    Some("NC flag on unconnected pin only".into()),
                    Some("NC flag has connection".into()),
                    "error",
                ),
                &[
                    "Wire accidentally connected to NC pin",
                    "NC flag placed on wrong pin",
                    "Design changed but NC flag not removed",
                ],
                &[
                    "Remove the wire from the no-connect pin",
                    "Remove the NC flag if connection is intentional",
                ],
            ),
            NO_CONNECT_DANGLING => simple(
                v,
                "No-connect flag is not on a pin",
                diag(
                    "No-connect placement",
                    Some("NC flag directly on unconnected pin".into()),
                    Some("NC flag floating/not on pin".into()),
                    "warning",
                ),
                &[
                    "NC flag placed near but not on pin",
                    "Symbol moved leaving NC flag behind",
                    "NC flag off-grid (doesn't touch pin)",
                ],
                &[
                    "Move NC flag directly onto the pin endpoint",
                    "Delete NC flag if not needed",
                ],
            ),
            DIFFERENT_UNIT_NET => simple(
                v,
                "Same pin on different units connected to different nets",
                diag(
                    "Multi-unit pin consistency",
                    Some("Same pin connected to same net across units".into()),
                    Some("Different nets on same pin".into()),
                    "error",
                ),
                &[
                    "Different units accidentally connected to wrong nets",
                    "Symbol definition error (pin should be per-unit)",
                    "Wiring mistake in one unit",
                ],
                &[
                    "Connect all units' pins to the same net",
                    "Check symbol definition if pins should be different",
                ],
            ),
            MISSING_UNIT => simple(
                v,
                "Multi-unit symbol is missing one or more units",
                diag(
                    "Unit completeness",
                    Some("All units of multi-unit symbol placed".into()),
                    Some("Some units missing".into()),
                    "error",
                ),
                &[
                    "Not all units were placed from symbol",
                    "Unit was deleted accidentally",
                    "Design doesn't need all units (may be OK)",
                ],
                &[
                    "Add missing unit(s) from the symbol library",
                    "Verify all required units are present for your design",
                ],
            ),
            UNANNOTATED => {
                let mut e = simple(
                    v,
                    "Symbol has no reference designator assigned",
                    diag(
                        "Reference annotation",
                        Some("Unique reference like R1, C2, U3".into()),
                        Some("Reference is '?' or empty".into()),
                        "error",
                    ),
                    &[
                        "Symbol newly placed and not yet annotated",
                        "Annotation was skipped or failed",
                        "Reference field cleared accidentally",
                    ],
                    &[
                        "Run 'Annotate Schematic' from Tools menu",
                        "Manually enter reference designator (press E to edit)",
                    ],
                );
                e.fixes[0].command = Some("kct sch annotate <schematic>".into());
                e
            }
            MULTIPLE_NET_NAMES => simple(
                v,
                "Wire has multiple conflicting net names assigned",
                diag(
                    "Net name uniqueness",
                    Some("Single net name per wire segment".into()),
                    Some("Multiple labels on same wire".into()),
                    "error",
                ),
                &[
                    "Multiple labels placed on same wire",
                    "Hierarchical connection creates name conflict",
                    "Net tie needed but not placed",
                ],
                &[
                    "Remove duplicate labels, keep only one",
                    "Use net tie symbol if nets should be connected",
                    "Review hierarchical connections for conflicts",
                ],
            ),
            ENDPOINT_OFF_GRID => simple(
                v,
                "Wire or pin endpoint is not aligned to the grid",
                diag(
                    "Grid alignment",
                    Some("Endpoint on grid intersection".into()),
                    Some("Off-grid position".into()),
                    "warning",
                ),
                &[
                    "Wire drawn with grid snap disabled",
                    "Symbol with non-standard pin grid",
                    "Imported schematic with different grid",
                ],
                &[
                    "Move endpoint to snap to grid",
                    "Use Edit > Cleanup Graphics to fix all off-grid items",
                ],
            ),
            _ => generic(v),
        }
    }

    /// Labels whose `SequenceMatcher` ratio to `target` is >= `threshold`,
    /// best first, top 5.
    pub fn find_similar_labels(&self, target: &str, threshold: f64) -> Vec<SimilarLabel> {
        let t = target.to_lowercase();
        let mut out: Vec<SimilarLabel> = self
            .all_labels
            .iter()
            .filter(|l| l.as_str() != target)
            .filter_map(|l| {
                let ratio = sequence_ratio(&t, &l.to_lowercase());
                (ratio >= threshold).then(|| SimilarLabel {
                    name: l.clone(),
                    similarity: ratio,
                    location: String::new(),
                    direction: String::new(),
                })
            })
            .collect();
        out.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
        out.truncate(5);
        out
    }

    fn hier_label_mismatch(&self, v: &ERCViolation) -> ViolationExplanation {
        let name = extract_label_name(v);
        let n = opt(&name).to_string();
        let mut e = ViolationExplanation::new(
            v,
            "Sheet pin has no matching hierarchical label in sub-schematic",
        );
        e.diagnosis = vec![
            diag(
                "Sheet pin exists in parent",
                Some(format!("Pin '{n}' defined")),
                Some("Defined".into()),
                "ok",
            ),
            diag(
                "Hierarchical label in sub-schematic",
                Some(format!("Label '{n}' exists")),
                Some("Not found".into()),
                "error",
            ),
        ];
        e.possible_causes = vec![
            format!("Hierarchical label '{n}' was deleted or never created"),
            "Label name mismatch (typo or case difference)".into(),
            "Sub-schematic file was replaced with different version".into(),
            "Sheet instance UUID doesn't match sub-schematic".into(),
        ];
        if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
            let similar = self.find_similar_labels(label, 0.6);
            if let Some(best) = similar.first() {
                e.possible_causes.insert(
                    0,
                    format!("Label name mismatch: found '{}' (similar)", best.name),
                );
                e.similar_labels = similar;
            }
        }
        e.fixes = vec![fix(
            format!("Add hierarchical label '{n}' in the sub-schematic"),
            1,
        )];
        if name.as_deref().is_some_and(|s| !s.is_empty()) {
            if let Some(best) = e.similar_labels.first().cloned() {
                e.fixes.push(fix(
                    format!("Rename label '{}' to '{n}' in sub-schematic", best.name),
                    2,
                ));
                e.fixes.push(fix(
                    format!("Rename sheet pin '{n}' to '{}' in parent", best.name),
                    3,
                ));
            }
        }
        e.fixes
            .push(fix("Delete the sheet pin if no longer needed", 4));
        e.related_violations = strs(&["pin_not_connected (often accompanies this error)"]);
        e
    }

    fn global_label_dangling(&self, v: &ERCViolation) -> ViolationExplanation {
        let name = extract_label_name(v);
        let mut e = simple(
            v,
            format!("Global label '{}' is not connected to anything", opt(&name)),
            diag(
                "Global label connection",
                Some("Connected to wire and used on other sheets".into()),
                Some("No connection found".into()),
                "error",
            ),
            &[
                "Global label not connected to wire",
                "Label only exists on one sheet (no other sheets use it)",
                "Spelling mismatch with labels on other sheets",
            ],
            &[
                "Connect a wire to the global label",
                "Verify label spelling matches other sheets",
                "Delete if this global net is no longer needed",
            ],
        );
        if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
            let similar = self.find_similar_labels(label, 0.6);
            if let Some(best) = similar.first() {
                e.possible_causes.insert(
                    0,
                    format!("Possible typo: found similar label '{}'", best.name),
                );
                e.similar_labels = similar;
            }
        }
        e
    }
}

fn simple(
    v: &ERCViolation,
    summary: impl Into<String>,
    d: DiagnosisItem,
    causes: &[&str],
    fixes: &[&str],
) -> ViolationExplanation {
    let mut e = ViolationExplanation::new(v, summary);
    e.diagnosis = vec![d];
    e.possible_causes = strs(causes);
    e.fixes = fixes
        .iter()
        .enumerate()
        .map(|(i, f)| fix(*f, i as i64 + 1))
        .collect();
    e
}

/// Text between the first pair of single quotes, if non-empty.
fn first_quoted(s: &str) -> Option<String> {
    let start = s.find('\'')? + 1;
    let end = s[start..].find('\'')? + start;
    (end > start).then(|| s[start..end].to_string())
}

/// Label name from items (`Label 'NAME'` / `Label NAME`) or description.
fn extract_label_name(v: &ERCViolation) -> Option<String> {
    for item in &v.items {
        if item.to_lowercase().contains("label") {
            if item.contains('\'') {
                if let Some(n) = first_quoted(item) {
                    return Some(n);
                }
            }
            let parts: Vec<&str> = item.split_whitespace().collect();
            for (i, p) in parts.iter().enumerate() {
                if p.to_lowercase() == "label" && i + 1 < parts.len() {
                    return Some(
                        parts[i + 1]
                            .trim_matches(|c| c == '\'' || c == '"')
                            .to_string(),
                    );
                }
            }
        }
    }
    if v.description.contains('\'') {
        return first_quoted(&v.description);
    }
    None
}

#[derive(Default)]
struct PinInfo {
    component: Option<String>,
    name: Option<String>,
    ptype: Option<String>,
}

fn extract_pin_info(v: &ERCViolation) -> Option<PinInfo> {
    let item = v.items.iter().find(|i| i.to_lowercase().contains("pin"))?;
    let mut info = PinInfo::default();
    if item.contains(" of ") {
        let parts: Vec<&str> = item.split(" of ").collect();
        if parts.len() >= 2 {
            info.component = Some(parts[parts.len() - 1].trim().to_string());
        }
        let pin_part = parts[0];
        if let (Some(s), Some(e)) = (pin_part.find('('), pin_part.find(')')) {
            info.ptype = Some(pin_part.get(s + 1..e).unwrap_or_default().to_string());
        }
        let mut name = pin_part.replace("Pin ", "").trim().to_string();
        if let Some(p) = name.find('(') {
            name = name[..p].trim().to_string();
        }
        info.name = Some(name);
    }
    Some(info)
}

fn pin_fields(v: &ERCViolation) -> (String, String, String) {
    let info = extract_pin_info(v);
    let get = |f: fn(&PinInfo) -> &Option<String>| {
        info.as_ref()
            .and_then(|i| f(i).clone())
            .unwrap_or_else(|| "unknown".into())
    };
    (get(|i| &i.component), get(|i| &i.name), get(|i| &i.ptype))
}

fn pin_not_connected(v: &ERCViolation) -> ViolationExplanation {
    let (component, pin_name, pin_type) = pin_fields(v);
    let mut e = ViolationExplanation::new(
        v,
        format!("Pin '{pin_name}' on {component} is not connected to any net"),
    );
    e.diagnosis = vec![
        diag(
            "Pin connection status",
            Some("Connected to wire or label".into()),
            Some("No connection".into()),
            "error",
        ),
        diag("Pin electrical type", None, Some(pin_type.clone()), "info"),
    ];
    e.possible_causes = strs(&[
        "Wire endpoint doesn't reach the pin",
        "Pin intentionally left unconnected (needs no-connect flag)",
        "Connection was accidentally deleted",
        "Symbol placed but not wired",
    ]);
    e.fixes = vec![
        fix(
            format!("Connect a wire to pin '{pin_name}' of {component}"),
            1,
        ),
        fix("Add a No-Connect (X) flag if pin should be unconnected", 2),
        fix("Check wire endpoint alignment with pin", 3),
    ];
    if pin_type == "power_in" {
        e.related_violations = strs(&["power_pin_not_driven (if this is a power input)"]);
    }
    e
}

fn power_not_driven(v: &ERCViolation) -> ViolationExplanation {
    let (component, pin_name, _) = pin_fields(v);
    let mut e = ViolationExplanation::new(
        v,
        format!("Power pin '{pin_name}' on {component} not connected to power source"),
    );
    e.diagnosis = vec![
        diag(
            "Power pin connection",
            Some("Connected to power output (VCC, +3V3, etc.)".into()),
            Some("No power source on net".into()),
            "error",
        ),
        diag("Pin electrical type", None, Some("power_in".into()), "info"),
    ];
    e.possible_causes = strs(&[
        "Power symbol (VCC, GND, etc.) not placed on net",
        "Power symbol net name doesn't match (e.g., '+3V3' vs 'VCC')",
        "Power is supplied from PCB (needs PWR_FLAG)",
        "Wire not connected to power pin",
    ]);
    e.fixes = vec![
        fix(
            format!("Add power symbol (VCC, +3V3, GND) to drive '{pin_name}'"),
            1,
        ),
        fix(
            "Add PWR_FLAG if power comes from external source (PCB, connector)",
            2,
        ),
        fix("Check that power symbol net name matches pin net", 3),
    ];
    e
}

fn similar_labels(v: &ERCViolation) -> ViolationExplanation {
    let labels: Vec<String> = v
        .items
        .iter()
        .filter(|i| i.to_lowercase().contains("label") && i.contains('\''))
        .filter_map(|i| first_quoted(i))
        .collect();
    let summary = if labels.len() >= 2 {
        format!(
            "Labels '{}' and '{}' are similar (possible typo)",
            labels[0], labels[1]
        )
    } else {
        "Similar labels detected".into()
    };
    let mut e = ViolationExplanation::new(v, summary);
    e.diagnosis = vec![diag(
        "Label similarity",
        Some("Unique, clearly different label names".into()),
        Some(format!("Found similar: {}", labels.join(", "))),
        "warning",
    )];
    e.possible_causes = strs(&[
        "Typo in one of the label names",
        "Intentional similar names for related signals",
        "Case sensitivity issue (SIG vs sig)",
    ]);
    if labels.len() >= 2 {
        e.fixes = vec![
            fix(
                format!(
                    "Rename '{}' to '{}' if they should be same net",
                    labels[0], labels[1]
                ),
                1,
            ),
            fix("Make names more distinct if intentionally different", 2),
            fix("Add comments to clarify intentional similar names", 3),
        ];
    }
    e
}

fn duplicate_reference(v: &ERCViolation) -> ViolationExplanation {
    let mut refs = Vec::new();
    for item in &v.items {
        if item.to_lowercase().contains("symbol") {
            let parts: Vec<&str> = item.split_whitespace().collect();
            for (i, p) in parts.iter().enumerate() {
                if p.to_lowercase() == "symbol" && i + 1 < parts.len() {
                    refs.push(parts[i + 1].to_string());
                }
            }
        }
    }
    let r = refs.first().cloned().unwrap_or_else(|| "unknown".into());
    let mut e = ViolationExplanation::new(
        v,
        format!("Reference designator '{r}' is used by multiple symbols"),
    );
    e.diagnosis = vec![diag(
        "Reference uniqueness",
        Some("Unique reference per symbol".into()),
        Some(format!("'{r}' used multiple times")),
        "error",
    )];
    e.possible_causes = strs(&[
        "Copy-paste created duplicate symbols",
        "Manual annotation conflict",
        "Multi-unit symbol with wrong unit assignment",
    ]);
    e.fixes = vec![
        ExplainFix {
            description: "Run 'Annotate Schematic' to reassign references".into(),
            command: Some("kct sch annotate <schematic>".into()),
            priority: 1,
        },
        fix("Manually edit one symbol's reference to be unique", 2),
        fix("Delete duplicate symbol if unintended", 3),
    ];
    e
}

fn generic(v: &ERCViolation) -> ViolationExplanation {
    let mut e =
        ViolationExplanation::new(v, format!("{}: {}", v.type_description(), v.description));
    e.diagnosis = vec![diag(
        "Violation detected",
        None,
        Some(v.description.clone()),
        if v.is_error() { "error" } else { "warning" },
    )];
    e.possible_causes = strs(&[
        "Review the specific error message for details",
        "Check KiCad ERC documentation for this error type",
    ]);
    e.fixes = if v.suggestions.is_empty() {
        vec![
            fix("Review and fix the reported issue", 1),
            fix(
                format!(
                    "Check ERC settings for '{}' if this is a false positive",
                    v.type_str
                ),
                2,
            ),
        ]
    } else {
        v.suggestions
            .iter()
            .enumerate()
            .map(|(i, s)| fix(s.clone(), i as i64 + 1))
            .collect()
    };
    e
}

/// Run ERC (schematic input) or load a report; returns the report and the
/// schematic used for label context.
pub fn run_erc_explain(input: &Path, keep_report: bool) -> (Option<ERCReport>, Option<PathBuf>) {
    let ext = suffix(input);
    if ext == ".kicad_sch" {
        if find_kicad_cli().is_none() {
            eprintln!("Error: kicad-cli not found");
            return (None, None);
        }
        let result = run_erc(input, None, "json", true, None);
        if !result.success {
            eprintln!("Error running ERC: {}", result.stderr);
            return (None, None);
        }
        let Some(out) = result.output_path else {
            eprintln!("Error: ERC did not produce output");
            return (None, None);
        };
        let loaded = ERCReport::load(&out);
        if !keep_report {
            let _ = std::fs::remove_file(&out);
        }
        match loaded {
            Ok(r) => (Some(r), Some(input.to_path_buf())),
            Err(e) => {
                eprintln!("Error parsing ERC report: {e}");
                (None, None)
            }
        }
    } else if ext == ".json" || ext == ".rpt" {
        match ERCReport::load(input) {
            Ok(r) => {
                let mut sch = None;
                if !r.source_file.is_empty() {
                    let candidate = crate::drc::waivers::py_parent(input).join(&r.source_file);
                    if candidate.exists() {
                        sch = Some(candidate);
                    }
                }
                (Some(r), sch)
            }
            Err(e) => {
                eprintln!("Error loading report: {e}");
                (None, None)
            }
        }
    } else {
        eprintln!("Error: Unsupported file type: {ext}");
        (None, None)
    }
}

fn output_text(explanations: &[ViolationExplanation], report: &ERCReport) {
    let errors = explanations
        .iter()
        .filter(|e| e.violation.is_error())
        .count();
    let warnings = explanations.len() - errors;
    println!("\n{}", "=".repeat(70));
    println!("ERC ERROR ANALYSIS");
    println!("{}", "=".repeat(70));
    if !report.source_file.is_empty() {
        println!("File: {}", file_name(&report.source_file));
    }
    println!("Total: {errors} errors, {warnings} warnings\n");
    for (i, exp) in explanations.iter().enumerate() {
        let v = &exp.violation;
        let (severity, icon) = if v.is_error() {
            ("ERROR", "X")
        } else {
            ("WARNING", "!")
        };
        println!("{}", "-".repeat(70));
        println!("\n[{icon}] {severity} #{}: {}", i + 1, v.type_description());
        println!("    {}", v.description);
        let has_pos = v.pos_x != 0.0 || v.pos_y != 0.0;
        if !v.sheet.is_empty() || has_pos {
            let mut parts = Vec::new();
            if !v.sheet.is_empty() {
                parts.push(format!("Sheet: {}", v.sheet));
            }
            if has_pos {
                parts.push(format!("@ ({:.2}, {:.2})", v.pos_x, v.pos_y));
            }
            println!("    Location: {}", parts.join(" "));
        }
        println!("\n    Summary: {}", exp.summary);
        if !exp.diagnosis.is_empty() {
            println!("\n    Diagnosis:");
            for d in &exp.diagnosis {
                let icon = match d.status.as_str() {
                    "ok" => "✓",
                    "error" => "✗",
                    "warning" => "!",
                    _ => "•",
                };
                println!("      {icon} {}", d.check);
                if let Some(x) = d.expected.as_deref().filter(|s| !s.is_empty()) {
                    println!("        Expected: {x}");
                }
                if let Some(x) = d.actual.as_deref().filter(|s| !s.is_empty()) {
                    println!("        Actual:   {x}");
                }
            }
        }
        if !exp.similar_labels.is_empty() {
            println!("\n    Similar labels found:");
            for s in &exp.similar_labels {
                println!(
                    "      - '{}' ({}% similar)",
                    s.name,
                    (s.similarity * 100.0) as i64
                );
            }
        }
        if !exp.possible_causes.is_empty() {
            println!("\n    Possible causes:");
            for c in &exp.possible_causes {
                println!("      • {c}");
            }
        }
        if !exp.fixes.is_empty() {
            println!("\n    Suggested fixes:");
            for (j, f) in exp.fixes.iter().enumerate() {
                println!("      {}. {}", j + 1, f.description);
                if let Some(cmd) = f.command.as_deref().filter(|s| !s.is_empty()) {
                    println!("         Command: {cmd}");
                }
            }
        }
        if !exp.related_violations.is_empty() {
            println!("\n    Related violations:");
            for r in &exp.related_violations {
                println!("      → {r}");
            }
        }
    }
    println!("\n{}", "=".repeat(70));
    if errors > 0 {
        println!("Fix errors before proceeding with design.");
    } else {
        println!("Review warnings and fix if needed.");
    }
}

fn run_explain(args: ExplainArgs) -> Result<i32> {
    if !args.input.exists() {
        eprintln!("Error: File not found: {}", args.input.display());
        return Ok(1);
    }
    let (report, schematic) = run_erc_explain(&args.input, args.keep_report);
    let Some(report) = report else {
        return Ok(1);
    };
    let all_labels = schematic
        .as_deref()
        .map(gather_hierarchical_labels)
        .unwrap_or_default();
    let mut violations: Vec<&ERCViolation> =
        report.violations.iter().filter(|v| !v.excluded).collect();
    if args.errors_only {
        violations.retain(|v| v.is_error());
    }
    if let Some(t) = &args.filter_type {
        let f = t.to_lowercase();
        violations.retain(|v| {
            v.type_str.to_lowercase().contains(&f) || v.description.to_lowercase().contains(&f)
        });
    }
    if violations.is_empty() {
        println!("No ERC violations to explain.");
        return Ok(0);
    }
    let explainer = ERCExplainer::new(schematic, all_labels);
    let explanations: Vec<ViolationExplanation> =
        violations.iter().map(|v| explainer.explain(v)).collect();
    if args.format == "json" {
        let data = jobj! {
            "source" => report.source_file.as_str(),
            "kicad_version" => report.kicad_version.as_str(),
            "summary" => jobj!{
                "errors" => explanations.iter().filter(|e| e.violation.is_error()).count(),
                "warnings" => explanations.iter().filter(|e| !e.violation.is_error()).count(),
            },
            "explanations" => Json::Arr(explanations.iter().map(ViolationExplanation::to_json).collect()),
        };
        println!("{}", dumps_indent(&data, 2));
    } else {
        output_text(&explanations, &report);
    }
    let errors = explanations
        .iter()
        .filter(|e| e.violation.is_error())
        .count();
    Ok(if errors > 0 { 1 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::sequence_ratio;

    #[test]
    fn ratio_matches_difflib() {
        // difflib.SequenceMatcher(None, a, b).ratio()
        assert!((sequence_ratio("abcd", "bcde") - 0.75).abs() < 1e-12);
        assert!((sequence_ratio("spi_mosi", "spi_miso") - 0.75).abs() < 1e-12);
        assert_eq!(sequence_ratio("", ""), 1.0);
        assert_eq!(sequence_ratio("abc", "xyz"), 0.0);
    }
}
