//! `kct explain` (port of `kicad_tools.cli.explain_cmd`): explain design
//! rules, rule searches, net constraints and DRC-report violations.

use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use clap::Parser;

use super::Globals;
use crate::explain::{
    explain, explain_net_constraints, explain_violations, format_result, format_violations,
    list_rules, search_rules,
};
use crate::pyjson::{dumps_indent, Json};

#[derive(Parser, Debug)]
#[command(about = "Explain design rules and DRC violations")]
struct Args {
    /// Rule ID to explain (e.g., trace_clearance, via_drill)
    rule: Option<String>,
    /// List all available rule IDs
    #[arg(long, short = 'l')]
    list: bool,
    /// Search for rules matching a query
    #[arg(long, short = 's', value_name = "QUERY")]
    search: Option<String>,
    /// Current/actual value for contextualized explanation
    #[arg(long, short = 'v', allow_negative_numbers = true)]
    value: Option<f64>,
    /// Required/minimum value
    #[arg(long, short = 'r', allow_negative_numbers = true)]
    required: Option<f64>,
    /// Unit of measurement (default: mm)
    #[arg(long, short = 'u', default_value = "mm")]
    unit: String,
    /// First net name for context
    #[arg(long)]
    net1: Option<String>,
    /// Second net name for context
    #[arg(long)]
    net2: Option<String>,
    /// Path to DRC report file to explain all violations
    #[arg(long = "drc-report", short = 'd', value_name = "FILE")]
    drc_report: Option<String>,
    /// Output format (default: text)
    #[arg(long, short = 'f', default_value = "text",
          value_parser = ["text", "tree", "json", "markdown"])]
    format: String,
    /// Explain constraints for a specific net
    #[arg(long, short = 'n', value_name = "NAME")]
    net: Option<String>,
    /// Specify interface type for net (usb, i2c, spi)
    #[arg(long, short = 'i')]
    interface: Option<String>,
}

fn list() -> i32 {
    let rules = list_rules();
    if rules.is_empty() {
        println!("No rules found. Check that spec YAML files are present.");
        return 1;
    }
    println!("Available Rules:");
    println!("{}", "=".repeat(40));
    for r in &rules {
        println!("  {r}");
    }
    println!();
    println!("Total: {} rules", rules.len());
    println!("\nUse 'kct explain <rule>' to see details.");
    0
}

fn search(query: &str, format: &str) -> i32 {
    let matches = search_rules(query);
    if matches.is_empty() {
        println!("No rules found matching '{query}'");
        return 1;
    }
    println!("Rules matching '{query}':");
    println!("{}", "=".repeat(40));
    for e in &matches {
        if format == "json" {
            println!("{}", dumps_indent(&e.to_dict(), 2));
        } else {
            println!("\n{}: {}", e.rule_id, e.title);
            // `exp.explanation[:100]` (code points).
            let head: String = e.explanation.chars().take(100).collect();
            println!("  {head}...");
        }
    }
    println!();
    println!("Total: {} matches", matches.len());
    0
}

fn drc_report(path: &str, format: &str) -> Result<i32> {
    if !Path::new(path).exists() {
        eprintln!("Error: File not found: {path}");
        return Ok(1);
    }
    let report = match crate::drc::report::DRCReport::load(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error loading DRC report: {e}");
            return Ok(1);
        }
    };
    if report.violations.is_empty() {
        println!("No violations found in report.");
        return Ok(0);
    }
    let explained = explain_violations(&report.violations);
    match format_violations(&explained, format) {
        Ok(out) => {
            println!("{out}");
            Ok(0)
        }
        Err(e) => anyhow::bail!("ValueError: {e}"),
    }
}

/// `kct explain` entry point.
pub fn run(args: Vec<OsString>, _g: &Globals) -> Result<i32> {
    let a: Args = super::parse_args("explain", args);
    if a.list {
        return Ok(list());
    }
    if let Some(q) = &a.search {
        return Ok(search(q, &a.format));
    }
    if let Some(d) = &a.drc_report {
        return drc_report(d, &a.format);
    }
    if let Some(net) = &a.net {
        let r = explain_net_constraints(net, a.interface.as_deref());
        println!(
            "{}",
            format_result(&r, &a.format).map_err(anyhow::Error::msg)?
        );
        return Ok(0);
    }
    let Some(rule) = &a.rule else {
        use clap::CommandFactory;
        Args::command().name("kct explain").print_help()?;
        return Ok(0);
    };
    let mut ctx = Json::obj();
    if let Some(v) = a.value {
        ctx.set("value", Json::Float(v));
    }
    if let Some(v) = a.required {
        ctx.set("required_value", Json::Float(v));
    }
    if !a.unit.is_empty() {
        ctx.set("unit", a.unit.as_str());
    }
    if let Some(n) = a.net1.as_deref().filter(|n| !n.is_empty()) {
        ctx.set("net1", n);
    }
    if let Some(n) = a.net2.as_deref().filter(|n| !n.is_empty()) {
        ctx.set("net2", n);
    }
    let has_ctx = matches!(&ctx, Json::Obj(items) if !items.is_empty());
    match explain(rule, has_ctx.then_some(ctx)) {
        Ok(r) => {
            println!(
                "{}",
                format_result(&r, &a.format).map_err(anyhow::Error::msg)?
            );
            Ok(0)
        }
        Err(e) => {
            eprintln!("Error: {e}");
            println!("\nUse 'kct explain --list' to see available rules.");
            Ok(1)
        }
    }
}
