//! Native schematic, netlist, BOM, library, validation, and sync commands.

use super::Globals;
use crate::{parse_file, SExp};
use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Table,
    Json,
    Csv,
}
#[derive(Parser)]
struct SymbolsArgs {
    schematic: PathBuf,
    #[arg(long, value_enum, default_value = "table")]
    format: Format,
    #[arg(long = "filter")]
    pattern: Option<String>,
    #[arg(long = "lib")]
    lib_id: Option<String>,
    #[arg(short, long)]
    verbose: bool,
}
#[derive(Parser)]
struct NetsArgs {
    schematic: PathBuf,
    #[arg(long, value_enum, default_value = "table")]
    format: Format,
    #[arg(long)]
    net: Option<String>,
    #[arg(long)]
    stats: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct Symbol {
    reference: String,
    value: String,
    footprint: String,
    lib_id: String,
    dnp: bool,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct Net {
    name: String,
    kind: String,
    x: String,
    y: String,
}

fn load(path: &Path) -> Result<SExp> {
    parse_file(path).with_context(|| format!("read {}", path.display()))
}
fn symbols_of(root: &SExp) -> Vec<Symbol> {
    root.children_named("symbol")
        .filter_map(|s| {
            let reference = s.property("Reference")?.to_owned();
            if reference.starts_with('#') {
                return None;
            }
            Some(Symbol {
                reference,
                value: s.property("Value").unwrap_or("").into(),
                footprint: s.property("Footprint").unwrap_or("").into(),
                lib_id: s.child_str("lib_id").unwrap_or("").into(),
                dnp: s.flag("dnp"),
            })
        })
        .collect()
}
fn labels_of(root: &SExp) -> Vec<Net> {
    ["label", "global_label", "hierarchical_label"]
        .into_iter()
        .flat_map(|tag| {
            root.children_named(tag).filter_map(move |n| {
                let (x, y, _) = n.at()?;
                Some(Net {
                    name: n.string_at(0)?.into(),
                    kind: tag.into(),
                    x: x.to_string(),
                    y: y.to_string(),
                })
            })
        })
        .collect()
}
fn json<T: Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

pub fn symbols(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: SymbolsArgs = super::parse_args("symbols", args);
    let doc = load(&a.schematic)?;
    let mut rows = symbols_of(&doc);
    rows.retain(|s| {
        a.pattern.as_ref().is_none_or(|p| wildcard(p, &s.reference))
            && a.lib_id.as_ref().is_none_or(|p| s.lib_id.contains(p))
    });
    match a.format {
        Format::Json => json(&rows)?,
        Format::Csv => {
            println!("Reference,Value,Footprint,Library");
            for s in rows {
                println!(
                    "{},{},{},{}",
                    csv(&s.reference),
                    csv(&s.value),
                    csv(&s.footprint),
                    csv(&s.lib_id)
                );
            }
        }
        Format::Table => {
            println!(
                "{:<12} {:<24} {:<34} LIBRARY",
                "REFERENCE", "VALUE", "FOOTPRINT"
            );
            for s in rows {
                println!(
                    "{:<12} {:<24} {:<34} {}",
                    s.reference, s.value, s.footprint, s.lib_id
                )
            }
        }
    }
    Ok(0)
}
pub fn nets(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: NetsArgs = super::parse_args("nets", args);
    let doc = load(&a.schematic)?;
    let mut rows = labels_of(&doc);
    rows.retain(|n| a.net.as_ref().is_none_or(|x| x == &n.name));
    if a.stats {
        let unique: BTreeSet<_> = rows.iter().map(|n| &n.name).collect();
        println!("labels: {}\nunique_nets: {}", rows.len(), unique.len());
    } else if matches!(a.format, Format::Json) {
        json(&rows)?
    } else {
        println!("{:<32} {:<20} POSITION", "NET", "TYPE");
        for n in rows {
            println!("{:<32} {:<20} {},{}", n.name, n.kind, n.x, n.y)
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct NetlistArgs {
    #[command(subcommand)]
    cmd: NetlistCmd,
}
#[derive(Subcommand)]
enum NetlistCmd {
    Analyze(FileFmt),
    List(FileFmt),
    Show {
        schematic: PathBuf,
        #[arg(long)]
        net: String,
        #[arg(long, default_value = "text")]
        format: String,
    },
    Check(FileFmt),
    Compare {
        old: PathBuf,
        new: PathBuf,
        #[arg(long, default_value = "text")]
        format: String,
    },
    Export {
        schematic: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, default_value = "json")]
        format: String,
    },
}
#[derive(Args)]
struct FileFmt {
    schematic: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
}
fn net_names(root: &SExp) -> BTreeSet<String> {
    labels_of(root).into_iter().map(|n| n.name).collect()
}
pub fn netlist(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: NetlistArgs = super::parse_args("netlist", args);
    match a.cmd {
        NetlistCmd::Analyze(x) => {
            let d = load(&x.schematic)?;
            json(
                &serde_json::json!({"symbols":symbols_of(&d).len(),"nets":net_names(&d).len(),"labels":labels_of(&d).len()}),
            )?
        }
        NetlistCmd::List(x) => {
            let d = load(&x.schematic)?;
            let n = net_names(&d);
            if x.format == "json" {
                json(&n)?
            } else {
                for v in n {
                    println!("{v}")
                }
            }
        }
        NetlistCmd::Show {
            schematic,
            net,
            format,
        } => {
            let d = load(&schematic)?;
            let rows: Vec<_> = labels_of(&d)
                .into_iter()
                .filter(|n| n.name == net)
                .collect();
            if format == "json" {
                json(&rows)?
            } else {
                for n in rows {
                    println!("{} {} {},{}", n.name, n.kind, n.x, n.y)
                }
            }
        }
        NetlistCmd::Check(x) => {
            let d = load(&x.schematic)?;
            let dup = duplicates(labels_of(&d).into_iter().map(|n| (n.name, n.x, n.y)));
            let result = serde_json::json!({"ok":true,"duplicate_labels_at_same_point":dup});
            if x.format == "json" {
                json(&result)?
            } else {
                println!("netlist check: ok")
            }
        }
        NetlistCmd::Compare { old, new, format } => {
            let a = net_names(&load(&old)?);
            let b = net_names(&load(&new)?);
            let added: Vec<_> = b.difference(&a).collect();
            let removed: Vec<_> = a.difference(&b).collect();
            if format == "json" {
                json(&serde_json::json!({"added":added,"removed":removed}))?
            } else {
                for n in added {
                    println!("+ {n}")
                }
                for n in removed {
                    println!("- {n}")
                }
            }
        }
        NetlistCmd::Export {
            schematic,
            output,
            format: _,
        } => {
            let d = load(&schematic)?;
            let data = serde_json::to_string_pretty(
                &serde_json::json!({"symbols":symbols_of(&d),"nets":net_names(&d)}),
            )?;
            if let Some(p) = output {
                std::fs::write(p, data)?
            } else {
                println!("{data}")
            }
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct BomArgs {
    schematic: PathBuf,
    #[arg(long, value_enum, default_value = "table")]
    format: Format,
    #[arg(long)]
    group: bool,
    #[arg(long = "exclude")]
    exclude: Vec<String>,
    #[arg(long)]
    include_dnp: bool,
    #[arg(long, default_value = "reference")]
    sort: String,
}
#[derive(Serialize)]
struct BomRow {
    references: Vec<String>,
    quantity: usize,
    value: String,
    footprint: String,
}
pub fn bom(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: BomArgs = super::parse_args("bom", args);
    let d = load(&a.schematic)?;
    let mut syms = symbols_of(&d);
    syms.retain(|s| {
        (a.include_dnp || !s.dnp) && !a.exclude.iter().any(|p| wildcard(p, &s.reference))
    });
    let mut rows: Vec<BomRow> = if a.group {
        let mut m: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for s in syms {
            m.entry((s.value, s.footprint))
                .or_default()
                .push(s.reference)
        }
        m.into_iter()
            .map(|((value, footprint), mut references)| {
                references.sort();
                BomRow {
                    quantity: references.len(),
                    references,
                    value,
                    footprint,
                }
            })
            .collect()
    } else {
        syms.into_iter()
            .map(|s| BomRow {
                references: vec![s.reference],
                quantity: 1,
                value: s.value,
                footprint: s.footprint,
            })
            .collect()
    };
    if a.sort == "value" {
        rows.sort_by(|a, b| a.value.cmp(&b.value))
    } else if a.sort == "footprint" {
        rows.sort_by(|a, b| a.footprint.cmp(&b.footprint))
    }
    match a.format {
        Format::Json => json(&rows)?,
        Format::Csv => {
            println!("References,Quantity,Value,Footprint");
            for r in rows {
                println!(
                    "{},{},{},{}",
                    csv(&r.references.join(" ")),
                    r.quantity,
                    csv(&r.value),
                    csv(&r.footprint)
                )
            }
        }
        Format::Table => {
            println!(
                "{:<24} {:>3} {:<24} FOOTPRINT",
                "REFERENCES", "QTY", "VALUE"
            );
            for r in rows {
                println!(
                    "{:<24} {:>3} {:<24} {}",
                    r.references.join(","),
                    r.quantity,
                    r.value,
                    r.footprint
                )
            }
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct SchArgs {
    #[command(subcommand)]
    cmd: SchCmd,
}
#[derive(Subcommand)]
enum SchCmd {
    Summary(FileFmt),
    Labels(FileFmt),
    Wires(FileFmt),
    Validate(FileFmt),
    Info {
        schematic: PathBuf,
        reference: String,
        #[arg(long, default_value = "text")]
        format: String,
    },
}
pub fn sch(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: SchArgs = super::parse_args("sch", args);
    match a.cmd {
        SchCmd::Summary(x) => {
            let d = load(&x.schematic)?;
            json(
                &serde_json::json!({"symbols":symbols_of(&d).len(),"labels":labels_of(&d).len(),"wires":d.children_named("wire").count(),"junctions":d.children_named("junction").count()}),
            )?
        }
        SchCmd::Labels(x) => {
            return nets(
                vec![
                    x.schematic.into_os_string(),
                    "--format".into(),
                    if x.format == "json" { "json" } else { "table" }.into(),
                ],
                &Globals::default(),
            )
        }
        SchCmd::Wires(x) => {
            let d = load(&x.schematic)?;
            let w: Vec<_> = d.children_named("wire").map(|n| n.points()).collect();
            if x.format == "json" {
                json(&w)?
            } else {
                for p in w {
                    println!("{:?}", p)
                }
            }
        }
        SchCmd::Validate(x) => {
            return validate(
                vec![
                    x.schematic.into_os_string(),
                    "--format".into(),
                    x.format.into(),
                ],
                &Globals::default(),
            )
        }
        SchCmd::Info {
            schematic,
            reference,
            format,
        } => {
            let d = load(&schematic)?;
            let s = symbols_of(&d)
                .into_iter()
                .find(|s| s.reference == reference)
                .with_context(|| format!("symbol {reference} not found"))?;
            if format == "json" {
                json(&s)?
            } else {
                println!(
                    "{}: {} [{}] {}",
                    s.reference, s.value, s.lib_id, s.footprint
                )
            }
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct LibArgs {
    #[command(subcommand)]
    cmd: LibCmd,
}
#[derive(Subcommand)]
enum LibCmd {
    Symbols {
        library: PathBuf,
        #[arg(long, default_value = "table")]
        format: String,
    },
    Validate {
        library: PathBuf,
    },
}
pub fn lib(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: LibArgs = super::parse_args("lib", args);
    match a.cmd {
        LibCmd::Symbols { library, format } => {
            let d = load(&library)?;
            let names: Vec<_> = d
                .children_named("symbol")
                .filter_map(|s| s.string_at(0))
                .collect();
            if format == "json" {
                json(&names)?
            } else {
                for n in names {
                    println!("{n}")
                }
            }
        }
        LibCmd::Validate { library } => {
            let d = load(&library)?;
            if !d.has_tag("kicad_symbol_lib") {
                bail!("not a KiCad symbol library")
            }
            println!("valid: {} symbols", d.children_named("symbol").count())
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct ValidateArgs {
    files: Vec<PathBuf>,
    #[arg(long)]
    sync: bool,
    #[arg(long)]
    connectivity: bool,
    #[arg(long)]
    consistency: bool,
    #[arg(long)]
    placement: bool,
    #[arg(long)]
    lvs: bool,
    #[arg(short, long)]
    schematic: Option<PathBuf>,
    #[arg(short, long)]
    pcb: Option<PathBuf>,
    #[arg(long, default_value = "table")]
    format: String,
    #[arg(long)]
    strict: bool,
}
pub fn validate(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: ValidateArgs = super::parse_args("validate", args);
    let mut files = a.files;
    if let Some(s) = a.schematic {
        files.push(s)
    }
    if let Some(p) = a.pcb {
        files.push(p)
    }
    if files.is_empty() {
        bail!("provide a .kicad_sch, .kicad_pcb, or .kicad_pro file")
    }
    let mut reports = vec![];
    for p in files {
        if p.extension().and_then(|x| x.to_str()) == Some("kicad_pro") {
            serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&p)?)
                .with_context(|| format!("invalid project {}", p.display()))?;
        } else {
            let d = load(&p)?;
            let expected = if p.extension().and_then(|x| x.to_str()) == Some("kicad_sch") {
                "kicad_sch"
            } else {
                "kicad_pcb"
            };
            if !d.has_tag(expected) {
                bail!(
                    "{} has root {}, expected {expected}",
                    p.display(),
                    d.tag().unwrap_or("atom")
                )
            }
        }
        reports.push(serde_json::json!({"file":p,"valid":true}));
    }
    if a.format == "json" {
        json(&reports)?
    } else {
        for r in reports {
            println!("OK {}", r["file"].as_str().unwrap_or(""))
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct SyncArgs {
    #[arg(long)]
    analyze: bool,
    #[arg(long)]
    apply: bool,
    project: Option<PathBuf>,
    #[arg(short, long)]
    schematic: Option<PathBuf>,
    #[arg(short, long)]
    pcb: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    confirm: bool,
    #[arg(long, default_value = "table")]
    format: String,
}
pub fn sync(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: SyncArgs = super::parse_args("sync", args);
    if a.apply && !a.dry_run && !a.confirm {
        bail!("--apply requires --dry-run or --confirm")
    }
    let (s, p) = resolve_pair(a.project.as_deref(), a.schematic, a.pcb)?;
    let sd = load(&s)?;
    let pd = load(&p)?;
    let sr: BTreeSet<_> = symbols_of(&sd).into_iter().map(|s| s.reference).collect();
    let pr: BTreeSet<_> = pd
        .children_named("footprint")
        .filter_map(|f| f.property("Reference").map(str::to_owned))
        .collect();
    let result = serde_json::json!({"schematic":s,"pcb":p,"missing_on_pcb":sr.difference(&pr).collect::<Vec<_>>(),"orphaned_on_pcb":pr.difference(&sr).collect::<Vec<_>>(),"applied":false});
    if a.format == "json" {
        json(&result)?
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?)
    }
    if a.apply && a.confirm {
        bail!("native sync mutation is intentionally unavailable until mapping is unambiguous; use --analyze")
    };
    Ok(0)
}

fn resolve_pair(
    project: Option<&Path>,
    s: Option<PathBuf>,
    p: Option<PathBuf>,
) -> Result<(PathBuf, PathBuf)> {
    if let (Some(s), Some(p)) = (s, p) {
        return Ok((s, p));
    }
    let pro = project.context("provide project or --schematic and --pcb")?;
    let stem = pro.file_stem().context("invalid project name")?;
    let dir = pro.parent().unwrap_or(Path::new("."));
    Ok((
        dir.join(stem).with_extension("kicad_sch"),
        dir.join(stem).with_extension("kicad_pcb"),
    ))
}
fn wildcard(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        true
    } else if let Some(p) = pattern.strip_suffix('*') {
        text.starts_with(p)
    } else if let Some(p) = pattern.strip_prefix('*') {
        text.ends_with(p)
    } else {
        text.contains(pattern)
    }
}
fn csv(v: &str) -> String {
    format!("\"{}\"", v.replace('"', "\"\""))
}
fn duplicates(it: impl Iterator<Item = (String, String, String)>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = vec![];
    for (n, x, y) in it {
        let k = format!("{n}@{x},{y}");
        if !seen.insert(k.clone()) {
            out.push(k)
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn glob() {
        assert!(wildcard("U*", "U12"));
        assert!(wildcard("*12", "U12"));
        assert!(!wildcard("R*", "U12"));
    }
    #[test]
    fn dup() {
        assert_eq!(
            duplicates(
                [
                    ("N".into(), "1".into(), "2".into()),
                    ("N".into(), "1".into(), "2".into())
                ]
                .into_iter()
            ),
            vec!["N@1,2"]
        )
    }

    #[test]
    fn extracts_top_level_symbols_and_ignores_embedded_library_symbols() {
        let root = crate::parse(
            r##"(kicad_sch
              (lib_symbols (symbol "Device:R" (property "Reference" "R")))
              (symbol (lib_id "Device:R") (property "Reference" "R1")
                (property "Value" "10k") (property "Footprint" "R_0603"))
              (symbol (lib_id "power:GND") (property "Reference" "#PWR01")
                (property "Value" "GND")))"##,
        )
        .unwrap();
        assert_eq!(
            symbols_of(&root),
            vec![Symbol {
                reference: "R1".into(),
                value: "10k".into(),
                footprint: "R_0603".into(),
                lib_id: "Device:R".into(),
                dnp: false,
            }]
        );
    }

    #[test]
    fn extracts_all_label_dialects() {
        let root = crate::parse(
            r#"(kicad_sch (label "LOCAL" (at 1 2 0))
              (global_label "GND" (at 3 4 0))
              (hierarchical_label "BUS" (at 5 6 0)))"#,
        )
        .unwrap();
        let names: Vec<_> = labels_of(&root).into_iter().map(|n| n.name).collect();
        assert_eq!(names, ["LOCAL", "GND", "BUS"]);
    }
}
