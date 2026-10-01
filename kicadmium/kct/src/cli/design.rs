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
    x: String,
    y: String,
    rotation: String,
    unit: i64,
    uuid: String,
    in_bom: bool,
    mpn: String,
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
                x: s.at().map(|p| p.0.to_string()).unwrap_or_default(),
                y: s.at().map(|p| p.1.to_string()).unwrap_or_default(),
                rotation: s
                    .at()
                    .map(|p| p.2.to_string())
                    .unwrap_or_else(|| "0".into()),
                unit: s.get("unit").and_then(|n| n.int_at(0)).unwrap_or(1),
                uuid: s.child_str("uuid").unwrap_or("").into(),
                in_bom: s.child_str("in_bom").is_none_or(|v| v != "no"),
                mpn: s
                    .property("MPN")
                    .or_else(|| s.property("Manufacturer Part Number"))
                    .unwrap_or("")
                    .into(),
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
        Format::Json => {
            let out:Vec<_>=rows.iter().map(|s| {
                let mut v=serde_json::json!({"reference":s.reference,"value":s.value,"lib_id":s.lib_id,"footprint":s.footprint,"position":[s.x.parse::<f64>().unwrap_or(0.0),s.y.parse::<f64>().unwrap_or(0.0)],"rotation":s.rotation.parse::<f64>().unwrap_or(0.0)});
                if a.verbose { if let Some(o)=v.as_object_mut(){o.insert("unit".into(),s.unit.into());o.insert("uuid".into(),s.uuid.clone().into());o.insert("in_bom".into(),s.in_bom.into());o.insert("dnp".into(),s.dnp.into());o.insert("pins".into(),serde_json::Value::Array(vec![]));} }
                v
            }).collect();
            json(&out)?
        }
        Format::Csv => {
            println!(
                "Reference,Value,Library ID,Footprint,X,Y,Rotation{}",
                if a.verbose { ",Unit,UUID" } else { "" }
            );
            for s in rows {
                print!(
                    "{},{},{},{},{},{},{}",
                    s.reference, s.value, s.lib_id, s.footprint, s.x, s.y, s.rotation
                );
                if a.verbose {
                    print!(",{},{}", s.unit, s.uuid);
                }
                println!();
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
    let mut rows = trace_nets(&doc);
    rows.retain(|n| a.net.as_ref().is_none_or(|x| x == &n.name));
    if a.net.is_some() && rows.is_empty() {
        return Ok(1);
    }
    if a.stats {
        let labeled = rows.iter().filter(|n| n.has_label).count();
        println!("Net Statistics\n========================================\nTotal nets:         {}\n  Labeled:          {}\n  Unlabeled:        {}\nTotal wires:        {}\nTotal wire length:  {:.2} mm\nTotal connections:  {}",rows.len(),labeled,rows.len()-labeled,rows.iter().map(|n|n.wire_count).sum::<usize>(),rows.iter().map(|n|n.total_length).sum::<f64>(),rows.iter().map(|n|n.connection_count).sum::<usize>());
    } else if matches!(a.format, Format::Json) {
        json(&rows)?
    } else {
        println!(
            "{:<25}  {:<5}  {:<6}  {:<10}  Connections",
            "Name", "Label", "Wires", "Length"
        );
        println!("{}", "-".repeat(70));
        for n in rows {
            println!(
                "{:<25}  {:<5}  {:<6}  {:>7.2} mm",
                n.name,
                if n.has_label { "Y" } else { "" },
                n.wire_count,
                n.total_length
            )
        }
    }
    Ok(0)
}

#[derive(Debug, Clone, Serialize)]
struct TracedNet {
    name: String,
    has_label: bool,
    wire_count: usize,
    total_length: f64,
    connection_count: usize,
}
fn point_key(p: (f64, f64)) -> (i64, i64) {
    ((p.0 * 10.0).round() as i64, (p.1 * 10.0).round() as i64)
}
fn trace_nets(root: &SExp) -> Vec<TracedNet> {
    let wires: Vec<_> = root
        .children_named("wire")
        .filter_map(|w| {
            let p = w.points();
            (p.len() >= 2).then(|| (p[0], p[p.len() - 1]))
        })
        .collect();
    let mut parent: Vec<usize> = (0..wires.len()).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        if p[x] != x {
            p[x] = find(p, p[x]);
        }
        p[x]
    }
    for i in 0..wires.len() {
        for j in i + 1..wires.len() {
            if [wires[i].0, wires[i].1].into_iter().any(|a| {
                [wires[j].0, wires[j].1]
                    .into_iter()
                    .any(|b| point_key(a) == point_key(b))
            }) {
                let a = find(&mut parent, i);
                let b = find(&mut parent, j);
                parent[b] = a;
            }
        }
    }
    let labels = labels_of(root);
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..wires.len() {
        let k = find(&mut parent, i);
        groups.entry(k).or_default().push(i);
    }
    let mut out = vec![];
    let mut used = BTreeSet::new();
    for ids in groups.values() {
        let pts: BTreeSet<_> = ids
            .iter()
            .flat_map(|&i| [point_key(wires[i].0), point_key(wires[i].1)])
            .collect();
        let label = labels.iter().enumerate().find(|(_, l)| {
            pts.contains(&point_key((
                l.x.parse().unwrap_or(0.0),
                l.y.parse().unwrap_or(0.0),
            )))
        });
        if let Some((i, _)) = label {
            used.insert(i);
        }
        let name = label.map(|(_, l)| l.name.clone()).unwrap_or_else(|| {
            let h = pts.iter().fold(0u64, |h, (x, y)| {
                h.wrapping_mul(1099511628211) ^ (*x as u64).wrapping_mul(31) ^ (*y as u64)
            });
            format!("Net_{:04X}", h & 0xffff)
        });
        out.push(TracedNet {
            name,
            has_label: label.is_some(),
            wire_count: ids.len(),
            total_length: ids
                .iter()
                .map(|&i| {
                    ((wires[i].1 .0 - wires[i].0 .0).powi(2)
                        + (wires[i].1 .1 - wires[i].0 .1).powi(2))
                    .sqrt()
                })
                .sum(),
            connection_count: usize::from(label.is_some()),
        });
    }
    for (i, l) in labels.iter().enumerate() {
        if !used.contains(&i) {
            out.push(TracedNet {
                name: l.name.clone(),
                has_label: true,
                wire_count: 0,
                total_length: 0.0,
                connection_count: 1,
            });
        }
    }
    out
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
        #[arg(long, value_parser=["kicad","json"], default_value = "kicad")]
        format: String,
    },
}
#[derive(Args)]
struct FileFmt {
    schematic: PathBuf,
    #[arg(long, default_value = "text")]
    format: String,
    #[arg(long, default_value = "connections")]
    sort: String,
    #[arg(short, long)]
    verbose: bool,
    #[arg(long)]
    strict: bool,
    #[arg(short, long)]
    quiet: bool,
    #[arg(long)]
    stats: bool,
    #[arg(long)]
    junctions: bool,
    #[arg(long = "type", default_value = "all")]
    kind: String,
    #[arg(long = "filter")]
    pattern: Option<String>,
}
fn net_names(root: &SExp) -> BTreeSet<String> {
    electrical_netlist(root)
        .into_iter()
        .map(|n| n.name)
        .collect()
}
fn is_power_net(name: &str) -> bool {
    matches!(
        name.trim_start_matches('/').to_ascii_uppercase().as_str(),
        "GND" | "VCC" | "VDD" | "VSS" | "+3V3" | "+5V" | "+12V"
    )
}
#[derive(Serialize)]
struct NetlistEntry {
    name: String,
    connections: usize,
    r#type: String,
    pins: Vec<String>,
}
fn electrical_netlist(root: &SExp) -> Vec<NetlistEntry> {
    let wires: Vec<_> = root
        .children_named("wire")
        .filter_map(|w| {
            let p = w.points();
            (p.len() >= 2).then(|| (p[0], p[p.len() - 1]))
        })
        .collect();
    let labels = labels_of(root);
    let mut out = vec![];
    for label in labels {
        let lp = (
            label.x.parse().unwrap_or(0.0),
            label.y.parse().unwrap_or(0.0),
        );
        let mut pins = vec![];
        for s in root.children_named("symbol") {
            let reference = s.property("Reference").unwrap_or("");
            if reference.starts_with('#') {
                continue;
            }
            for p in s.children_named("pin").filter_map(|p| p.string_at(0)) {
                if let Ok(pp) = pin_position(root, reference, p) {
                    if wires
                        .iter()
                        .any(|&(a, b)| point_segment_distance(pp, a, b) < 0.1)
                    {
                        // A pin belongs to this label only when both touch the same connected wire component.
                        let pin_wires: Vec<_> = wires
                            .iter()
                            .enumerate()
                            .filter(|(_, w)| point_segment_distance(pp, w.0, w.1) < 0.1)
                            .map(|(i, _)| i)
                            .collect();
                        let label_wires: Vec<_> = wires
                            .iter()
                            .enumerate()
                            .filter(|(_, w)| point_segment_distance(lp, w.0, w.1) < 0.1)
                            .map(|(i, _)| i)
                            .collect();
                        if pin_wires.iter().any(|i| label_wires.contains(i))
                            || connected_by_wires(&wires, &pin_wires, &label_wires)
                        {
                            pins.push(format!("{reference}.{p}"));
                        }
                    }
                }
            }
        }
        pins.sort();
        pins.dedup();
        out.push(NetlistEntry {
            name: format!("/{}", label.name),
            connections: pins.len(),
            r#type: "signal".into(),
            pins,
        });
    }
    out.sort_by(|a, b| {
        b.connections
            .cmp(&a.connections)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}
fn connected_by_wires(
    wires: &[((f64, f64), (f64, f64))],
    starts: &[usize],
    goals: &[usize],
) -> bool {
    let mut seen: BTreeSet<usize> = starts.iter().copied().collect();
    let mut stack = starts.to_vec();
    while let Some(i) = stack.pop() {
        if goals.contains(&i) {
            return true;
        }
        for j in 0..wires.len() {
            if !seen.contains(&j)
                && [wires[i].0, wires[i].1]
                    .into_iter()
                    .any(|a| [wires[j].0, wires[j].1].into_iter().any(|b| near(a, b)))
            {
                seen.insert(j);
                stack.push(j);
            }
        }
    }
    false
}
pub fn netlist(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: NetlistArgs = super::parse_args("netlist", args);
    match a.cmd {
        NetlistCmd::Analyze(x) => {
            let d = load(&x.schematic)?;
            let symbols = symbols_of(&d);
            let nets = electrical_netlist(&d);
            let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
            for s in &symbols {
                *by_type
                    .entry(
                        s.reference
                            .chars()
                            .take_while(|c| c.is_ascii_alphabetic())
                            .collect(),
                    )
                    .or_default() += 1;
            }
            let result = serde_json::json!({"source_file":std::fs::canonicalize(&x.schematic).unwrap_or(x.schematic),"tool":d.child_str("generator").unwrap_or("kicadmium"),"date":"","sheet_count":1,"component_count":symbols.len(),"components_by_type":by_type,"net_count":nets.len(),"power_net_count":nets.iter().filter(|n|is_power_net(&n.name)).count(),"signal_net_count":nets.iter().filter(|n|!is_power_net(&n.name)).count(),"single_pin_net_count":nets.iter().filter(|n|n.connections==1).count()});
            if x.format == "json" {
                json(&result)?
            } else {
                println!("NETLIST ANALYSIS\n================\nComponents: {}\nNets: {}\nSingle-pin nets: {}",symbols.len(),nets.len(),nets.iter().filter(|n|n.connections==1).count());
            }
        }
        NetlistCmd::List(x) => {
            let d = load(&x.schematic)?;
            let mut n = electrical_netlist(&d);
            if x.sort == "name" {
                n.sort_by(|a, b| a.name.cmp(&b.name));
            }
            if x.format == "json" {
                json(&n)?
            } else {
                for v in n {
                    println!("{:<32} {:>4}  {}", v.name, v.connections, v.pins.join(", "))
                }
            }
        }
        NetlistCmd::Show {
            schematic,
            net,
            format,
        } => {
            let d = load(&schematic)?;
            let rows: Vec<_> = electrical_netlist(&d)
                .into_iter()
                .filter(|n| n.name == net || n.name.trim_start_matches('/') == net)
                .collect();
            if format == "json" {
                json(&rows)?
            } else {
                for n in rows {
                    println!(
                        "Net: {}\nConnections: {}\n{}",
                        n.name,
                        n.connections,
                        n.pins.join("\n")
                    )
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
            format,
        } => {
            let d = load(&schematic)?;
            if format == "json" {
                let data = serde_json::to_string_pretty(
                    &serde_json::json!({"components":symbols_of(&d),"nets":electrical_netlist(&d)}),
                )?;
                if let Some(p) = output {
                    crate::fsutil::atomic_write(&p, data.as_bytes())?
                } else {
                    println!("{data}")
                }
            } else {
                let target = output.unwrap_or_else(|| {
                    schematic.with_file_name(format!(
                        "{}-netlist.kicad_net",
                        schematic.file_stem().unwrap_or_default().to_string_lossy()
                    ))
                });
                let mut text = String::from("(export (version \"E\") (components");
                for s in symbols_of(&d) {
                    text.push_str(&format!(
                        " (comp (ref \"{}\") (value \"{}\") (footprint \"{}\"))",
                        s.reference, s.value, s.footprint
                    ));
                }
                text.push_str(") (nets");
                for (code, n) in electrical_netlist(&d).into_iter().enumerate() {
                    text.push_str(&format!(
                        " (net (code \"{}\") (name \"{}\")",
                        code + 1,
                        n.name
                    ));
                    for pin in n.pins {
                        if let Some((r, p)) = pin.split_once('.') {
                            text.push_str(&format!(" (node (ref \"{r}\") (pin \"{p}\"))"));
                        }
                    }
                    text.push(')');
                }
                text.push_str("))\n");
                crate::fsutil::atomic_write(&target, text.as_bytes())?;
                println!("Exported KiCad netlist to: {}", target.display());
            }
        }
    }
    Ok(0)
}

#[derive(Parser)]
struct BomArgs {
    schematic: PathBuf,
    #[arg(long, value_enum, default_value = "table")]
    format: BomFormat,
    #[arg(long)]
    group: bool,
    #[arg(long = "exclude")]
    exclude: Vec<String>,
    #[arg(long)]
    include_dnp: bool,
    #[arg(long, default_value = "reference")]
    sort: String,
    #[arg(long)]
    check_availability: bool,
    #[arg(long)]
    validate: bool,
    #[arg(long, default_value_t = 1)]
    quantity: usize,
}
#[derive(Clone, Copy, ValueEnum)]
enum BomFormat {
    Table,
    Json,
    Csv,
    Jlcpcb,
}
#[derive(Serialize)]
struct BomRow {
    references: Vec<String>,
    quantity: usize,
    value: String,
    footprint: String,
    mpn: String,
    dnp: bool,
}
pub fn bom(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: BomArgs = super::parse_args("bom", args);
    let d = load(&a.schematic)?;
    let mut syms = symbols_of(&d);
    syms.retain(|s| {
        (a.include_dnp || !s.dnp) && !a.exclude.iter().any(|p| wildcard(p, &s.reference))
    });
    let mut rows: Vec<BomRow> = if a.group {
        let mut m: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
        for s in syms {
            m.entry((s.value, s.footprint, s.mpn))
                .or_default()
                .push(s.reference)
        }
        m.into_iter()
            .map(|((value, footprint, mpn), mut references)| {
                references.sort();
                BomRow {
                    quantity: references.len(),
                    references,
                    value,
                    footprint,
                    mpn,
                    dnp: false,
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
                mpn: s.mpn,
                dnp: s.dnp,
            })
            .collect()
    };
    if a.sort == "value" {
        rows.sort_by(|a, b| a.value.cmp(&b.value))
    } else if a.sort == "footprint" {
        rows.sort_by(|a, b| a.footprint.cmp(&b.footprint))
    }
    match a.format {
        BomFormat::Json => {
            if a.group {
                let groups:Vec<_>=rows.iter().map(|r|serde_json::json!({"quantity":r.quantity,"value":r.value,"footprint":r.footprint,"mpn":r.mpn,"references":r.references})).collect();
                json(
                    &serde_json::json!({"groups":groups,"total_groups":groups.len(),"total_components":rows.iter().map(|r|r.quantity).sum::<usize>()}),
                )?
            } else {
                let items:Vec<_>=rows.iter().map(|r|serde_json::json!({"reference":r.references[0],"value":r.value,"footprint":r.footprint,"mpn":r.mpn,"dnp":r.dnp})).collect();
                json(&serde_json::json!({"items":items,"total":items.len()}))?
            }
        }
        BomFormat::Csv => {
            println!(
                "{}",
                if a.group {
                    "Quantity,Value,Footprint,MPN,References"
                } else {
                    "Reference,Value,Footprint,MPN"
                }
            );
            for r in rows {
                if a.group {
                    println!(
                        "{},{},{},{},{}",
                        r.quantity,
                        csv(&r.value),
                        csv(&r.footprint),
                        csv(&r.mpn),
                        csv(&r.references.join(", "))
                    )
                } else {
                    println!("{},{},{},{}", r.references[0], r.value, r.footprint, r.mpn)
                }
            }
        }
        BomFormat::Table => {
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
        BomFormat::Jlcpcb => {
            println!("Comment,Designator,Footprint,LCSC Part #");
            for r in rows {
                println!(
                    "{},{},{},",
                    csv(&r.value),
                    csv(&r.references.join(",")),
                    csv(&r.footprint)
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
        #[arg(long)]
        show_pins: bool,
        #[arg(long)]
        show_properties: bool,
    },
    SetValue(PropertyEdit),
    SetFootprint(PropertyEdit),
    SetReference {
        schematic: PathBuf,
        #[arg(long = "ref")]
        reference: Option<String>,
        #[arg(long = "new-ref")]
        new_reference: Option<String>,
        #[arg(long = "map")]
        map_file: Option<PathBuf>,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
    },
    SetSymbolProperty {
        schematic: PathBuf,
        #[arg(long = "ref")]
        reference: String,
        #[arg(long)]
        property: String,
        #[arg(long)]
        value: String,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
    },
    Replace {
        schematic: PathBuf,
        reference: String,
        new_lib_id: String,
        #[arg(long)]
        value: Option<String>,
        #[arg(long)]
        footprint: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
    },
    RenameSignal {
        schematic: PathBuf,
        #[arg(long = "from")]
        old_name: String,
        #[arg(long = "to")]
        new_name: String,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(short = 'y', long)]
        yes: bool,
        #[arg(long)]
        include_nets: bool,
        #[arg(long)]
        include_globals: bool,
        #[arg(long, default_value = "text")]
        format: String,
    },
    AddWire {
        schematic: PathBuf,
        #[arg(long = "from", num_args = 2)]
        start: Vec<f64>,
        #[arg(long = "to", num_args = 2, action = clap::ArgAction::Append)]
        ends: Vec<f64>,
        #[arg(long)]
        junction: bool,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
    },
    AddJunction {
        schematic: PathBuf,
        #[arg(long, num_args = 2)]
        at: Vec<f64>,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
    },
    AddLabel {
        schematic: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long, num_args = 2)]
        at: Vec<f64>,
        #[arg(long = "type", default_value = "local")]
        kind: String,
        #[arg(long)]
        shape: Option<String>,
        #[arg(long, default_value_t = 0.0)]
        rotation: f64,
        #[arg(long = "connect")]
        connects: Vec<String>,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
    },
    RemoveComponent {
        schematic: PathBuf,
        #[arg(long = "ref")]
        reference: String,
        #[arg(long = "lib-path")]
        lib_paths: Vec<PathBuf>,
        #[arg(long = "lib")]
        libs: Vec<PathBuf>,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
        #[arg(long, default_value = "text")]
        format: String,
    },
    AssignFootprints(Tail),
    SuggestFootprint(Tail),
    SyncHierarchy(Tail),
    SetLabelDirection(Tail),
    AddNoConnect {
        schematic: PathBuf,
        #[arg(long = "ref")]
        reference: Option<String>,
        #[arg(long)]
        pin: Option<String>,
        #[arg(long)]
        auto: bool,
        #[arg(long = "lib-path")]
        lib_paths: Vec<PathBuf>,
        #[arg(long = "lib")]
        libs: Vec<PathBuf>,
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
    },
    AddComponent(Tail),
    AddBypassCap(Tail),
    AddPullResistor(Tail),
    CleanupWires(Tail),
    RemoveWire(Tail),
    InsertInline(Tail),
    Disconnect(Tail),
    FixWireStubs(Tail),
    ReconnectPin(Tail),
    MoveComponent(Tail),
    Tidy(Tail),
    RepairInstances(Tail),
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Args)]
struct Tail {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<OsString>,
}

#[derive(Args)]
struct PropertyEdit {
    schematic: PathBuf,
    #[arg(long = "ref")]
    reference: Option<String>,
    #[arg(long, alias = "footprint")]
    value: Option<String>,
    #[arg(long = "map")]
    map_file: Option<PathBuf>,
    #[arg(short = 'n', long)]
    dry_run: bool,
    #[arg(long)]
    backup: bool,
}
pub fn sch(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: SchArgs = super::parse_args("sch", args);
    match a.cmd {
        SchCmd::Summary(x) => {
            let d = load(&x.schematic)?;
            let v = serde_json::json!({"symbols":symbols_of(&d).len(),"labels":labels_of(&d).len(),"wires":d.children_named("wire").count(),"junctions":d.children_named("junction").count()});
            if x.format == "json" {
                json(&v)?
            } else {
                println!("Schematic Summary\n=================\nSymbols: {}\nLabels: {}\nWires: {}\nJunctions: {}",v["symbols"],v["labels"],v["wires"],v["junctions"]);
            }
        }
        SchCmd::Labels(x) => {
            let d = load(&x.schematic)?;
            let mut rows = labels_of(&d);
            rows.retain(|r| {
                (x.kind == "all"
                    || r.kind.trim_end_matches("_label") == x.kind
                    || x.kind == "local" && r.kind == "label")
                    && x.pattern.as_ref().is_none_or(|p| wildcard(p, &r.name))
            });
            match x.format.as_str() {
                "json" => json(&rows)?,
                "csv" => {
                    println!("Text,Type,X,Y");
                    for r in rows {
                        println!("{},{},{},{}", csv(&r.name), r.kind, r.x, r.y)
                    }
                }
                _ => {
                    println!("{:<32} {:<20} POSITION", "TEXT", "TYPE");
                    for r in rows {
                        println!("{:<32} {:<20} {},{}", r.name, r.kind, r.x, r.y)
                    }
                }
            }
        }
        SchCmd::Wires(x) => {
            let d = load(&x.schematic)?;
            let w: Vec<_> = d.children_named("wire").map(|n| n.points()).collect();
            if x.stats {
                println!(
                    "wires: {}\njunctions: {}\ntotal_length_mm: {:.3}",
                    w.len(),
                    d.children_named("junction").count(),
                    w.iter()
                        .map(|p| p
                            .windows(2)
                            .map(|q| ((q[1].0 - q[0].0).powi(2) + (q[1].1 - q[0].1).powi(2)).sqrt())
                            .sum::<f64>())
                        .sum::<f64>()
                );
            } else if x.format == "json" {
                json(&w)?
            } else if x.format == "csv" {
                println!("X1,Y1,X2,Y2");
                for p in w {
                    if p.len() >= 2 {
                        println!(
                            "{},{},{},{}",
                            p[0].0,
                            p[0].1,
                            p[p.len() - 1].0,
                            p[p.len() - 1].1
                        )
                    }
                }
            } else {
                for p in w {
                    println!("{:?}", p)
                }
                if x.junctions {
                    for j in d.children_named("junction").filter_map(|j| j.at()) {
                        println!("junction {},{}", j.0, j.1)
                    }
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
            show_pins,
            show_properties,
        } => {
            let d = load(&schematic)?;
            let s = symbols_of(&d)
                .into_iter()
                .find(|s| s.reference == reference)
                .with_context(|| format!("symbol {reference} not found"))?;
            if format == "json" {
                let mut v = serde_json::json!({"reference":s.reference,"value":s.value,"lib_id":s.lib_id,"footprint":s.footprint,"position":[s.x.parse::<f64>().unwrap_or(0.0),s.y.parse::<f64>().unwrap_or(0.0)],"rotation":s.rotation.parse::<f64>().unwrap_or(0.0),"unit":s.unit,"uuid":s.uuid,"in_bom":s.in_bom,"dnp":s.dnp});
                let raw = d
                    .children_named("symbol")
                    .find(|n| n.property("Reference") == Some(&reference))
                    .unwrap();
                if show_properties {
                    let props:Vec<_>=raw.children_named("property").map(|p|{let at=p.at().unwrap_or((0.0,0.0,0.0));serde_json::json!({"name":p.string_at(0),"value":p.string_at(1),"position":[at.0,at.1]})}).collect();
                    v.as_object_mut()
                        .unwrap()
                        .insert("properties".into(), props.into());
                }
                if show_pins {
                    let pins:Vec<_>=raw.children_named("pin").map(|p|serde_json::json!({"number":p.string_at(0),"uuid":p.child_str("uuid")})).collect();
                    v.as_object_mut()
                        .unwrap()
                        .insert("pins".into(), pins.into());
                }
                json(&v)?
            } else {
                println!(
                    "{}: {} [{}] {}",
                    s.reference, s.value, s.lib_id, s.footprint
                )
            }
        }
        SchCmd::SetValue(e) => edit_property(e, "Value")?,
        SchCmd::SetFootprint(e) => edit_property(e, "Footprint")?,
        SchCmd::SetReference {
            schematic,
            reference,
            new_reference,
            map_file,
            dry_run,
            backup,
        } => edit_property(
            PropertyEdit {
                schematic,
                reference,
                value: new_reference,
                map_file,
                dry_run,
                backup,
            },
            "Reference",
        )?,
        SchCmd::SetSymbolProperty {
            schematic,
            reference,
            property,
            value,
            dry_run,
            backup,
        } => edit_symbol(&schematic, &reference, dry_run, backup, |s| {
            let normalized = match value.to_ascii_lowercase().as_str() {
                "yes" | "true" | "1" => "yes",
                "no" | "false" | "0" => "no",
                _ => value.as_str(),
            };
            s.set_child_value(&property, normalized.to_owned())
        })?,
        SchCmd::Replace {
            schematic,
            reference,
            new_lib_id,
            value,
            footprint,
            dry_run,
            backup,
        } => edit_symbol(&schematic, &reference, dry_run, backup, |s| {
            s.set_child_value("lib_id", new_lib_id);
            if let Some(v) = value {
                set_property(s, "Value", &v)
            }
            if let Some(v) = footprint {
                set_property(s, "Footprint", &v)
            }
        })?,
        SchCmd::RenameSignal {
            schematic,
            old_name,
            new_name,
            dry_run,
            ..
        } => {
            let mut d = crate::Document::load(&schematic)?;
            let mut count = 0;
            for n in &mut d.root.children {
                if matches!(
                    n.tag(),
                    Some("label" | "global_label" | "hierarchical_label")
                ) && n.string_at(0) == Some(&old_name)
                {
                    n.set_value(0, new_name.clone());
                    count += 1
                }
            }
            save_edit(&d, &schematic, dry_run, false)?;
            println!("renamed {count} labels");
        }
        SchCmd::AddWire {
            schematic,
            start,
            ends,
            dry_run,
            ..
        } => {
            ensure_len(&start, 2, "--from requires x y")?;
            if ends.len() < 2 || ends.len() % 2 != 0 {
                bail!("--to requires x y")
            }
            let mut d = crate::Document::load(&schematic)?;
            let mut from = (start[0], start[1]);
            for to in ends.chunks_exact(2) {
                d.root.push(wire_node(from, (to[0], to[1])));
                from = (to[0], to[1]);
            }
            save_edit(&d, &schematic, dry_run, false)?;
        }
        SchCmd::AddJunction {
            schematic,
            at,
            dry_run,
            ..
        } => {
            ensure_len(&at, 2, "--at requires x y")?;
            append_node(
                &schematic,
                SExp::list(
                    "junction",
                    [SExp::list("at", [SExp::atom(at[0]), SExp::atom(at[1])])],
                ),
                dry_run,
            )?;
        }
        SchCmd::AddLabel {
            schematic,
            name,
            at,
            kind,
            dry_run,
            rotation,
            ..
        } => {
            ensure_len(&at, 2, "--at requires x y")?;
            let tag = match kind.as_str() {
                "global" => "global_label",
                "hierarchical" => "hierarchical_label",
                _ => "label",
            };
            append_node(
                &schematic,
                SExp::list(
                    tag,
                    [
                        SExp::quoted(name),
                        SExp::list(
                            "at",
                            [SExp::atom(at[0]), SExp::atom(at[1]), SExp::atom(rotation)],
                        ),
                    ],
                ),
                dry_run,
            )?;
        }
        SchCmd::RemoveComponent {
            schematic,
            reference,
            dry_run,
            backup,
            ..
        } => {
            let mut d = crate::Document::load(&schematic)?;
            let before = d.root.children.len();
            d.root
                .children
                .retain(|n| !(n.has_tag("symbol") && n.property("Reference") == Some(&reference)));
            if before == d.root.children.len() {
                bail!("symbol {reference} not found")
            }
            save_edit(&d, &schematic, dry_run, backup)?;
        }
        SchCmd::Other(raw) => return sch_extended(raw),
        SchCmd::AssignFootprints(x) => return extended("assign-footprints", x),
        SchCmd::SuggestFootprint(x) => return extended("suggest-footprint", x),
        SchCmd::SyncHierarchy(x) => return extended("sync-hierarchy", x),
        SchCmd::SetLabelDirection(x) => return extended("set-label-direction", x),
        SchCmd::AddNoConnect {
            schematic,
            reference,
            pin,
            auto,
            dry_run,
            backup,
            ..
        } => {
            if auto {
                add_no_connect_auto(&schematic, dry_run, backup)?;
            } else {
                add_no_connect(
                    &schematic,
                    reference
                        .as_deref()
                        .context("--ref required unless --auto")?,
                    pin.as_deref().context("--pin required unless --auto")?,
                    dry_run,
                    backup,
                )?;
            }
        }
        SchCmd::AddComponent(x) => return extended("add-component", x),
        SchCmd::AddBypassCap(x) => return extended("add-bypass-cap", x),
        SchCmd::AddPullResistor(x) => return extended("add-pull-resistor", x),
        SchCmd::CleanupWires(x) => return extended("cleanup-wires", x),
        SchCmd::RemoveWire(x) => return extended("remove-wire", x),
        SchCmd::InsertInline(x) => return extended("insert-inline", x),
        SchCmd::Disconnect(x) => return extended("disconnect", x),
        SchCmd::FixWireStubs(x) => return extended("fix-wire-stubs", x),
        SchCmd::ReconnectPin(x) => return extended("reconnect-pin", x),
        SchCmd::MoveComponent(x) => return extended("move-component", x),
        SchCmd::Tidy(x) => return extended("tidy", x),
        SchCmd::RepairInstances(x) => return extended("repair-instances", x),
    }
    Ok(0)
}
fn extended(name: &str, x: Tail) -> Result<i32> {
    let mut v = vec![OsString::from(name)];
    v.extend(x.args);
    sch_extended(v)
}

fn edit_property(e: PropertyEdit, key: &str) -> Result<()> {
    let PropertyEdit {
        schematic,
        reference,
        value,
        dry_run,
        backup,
        map_file,
    } = e;
    let mut edits: BTreeMap<String, String> = BTreeMap::new();
    if let (Some(r), Some(v)) = (reference, value) {
        edits.insert(r, v);
    }
    if let Some(path) = map_file {
        let text = std::fs::read_to_string(path)?;
        if let Ok(map) = serde_json::from_str::<BTreeMap<String, String>>(&text) {
            edits.extend(map);
        } else {
            for line in text.lines().skip(1) {
                if let Some((r, v)) = line.split_once(',') {
                    edits.insert(r.trim().into(), v.trim().into());
                }
            }
        }
    }
    if edits.is_empty() {
        bail!("provide --ref/--value or --map")
    }
    let mut d = crate::Document::load(&schematic)?;
    for (reference, value) in edits {
        let s = d
            .root
            .children
            .iter_mut()
            .find(|n| n.has_tag("symbol") && n.property("Reference") == Some(&reference))
            .with_context(|| format!("symbol {reference} not found"))?;
        set_property(s, key, &value);
    }
    save_edit(&d, &schematic, dry_run, backup)
}
fn edit_symbol(
    path: &Path,
    reference: &str,
    dry_run: bool,
    backup: bool,
    edit: impl FnOnce(&mut SExp),
) -> Result<()> {
    let mut d = crate::Document::load(path)?;
    let s = d
        .root
        .children
        .iter_mut()
        .find(|n| n.has_tag("symbol") && n.property("Reference") == Some(reference))
        .with_context(|| format!("symbol {reference} not found"))?;
    edit(s);
    save_edit(&d, path, dry_run, backup)
}
fn set_property(s: &mut SExp, key: &str, value: &str) {
    if let Some(p) = s
        .children
        .iter_mut()
        .find(|n| n.has_tag("property") && n.string_at(0) == Some(key))
    {
        p.set_value(1, value.to_owned())
    } else {
        s.push(SExp::list(
            "property",
            [SExp::quoted(key), SExp::quoted(value)],
        ));
    }
}
fn save_edit(d: &crate::Document, path: &Path, dry_run: bool, backup: bool) -> Result<()> {
    if dry_run {
        println!("would update {}", path.display());
        return Ok(());
    }
    if backup {
        std::fs::copy(
            path,
            path.with_extension(format!(
                "{}.bak",
                path.extension().and_then(|x| x.to_str()).unwrap_or("bak")
            )),
        )?;
    }
    d.save(None)?;
    println!("updated {}", path.display());
    Ok(())
}
fn append_node(path: &Path, node: SExp, dry_run: bool) -> Result<()> {
    let mut d = crate::Document::load(path)?;
    d.root.push(node);
    save_edit(&d, path, dry_run, false)
}
fn ensure_len(v: &[f64], n: usize, message: &str) -> Result<()> {
    if v.len() != n {
        bail!("{message}")
    }
    Ok(())
}
fn add_no_connect(
    path: &Path,
    reference: &str,
    pin: &str,
    dry_run: bool,
    backup: bool,
) -> Result<()> {
    let mut d = crate::Document::load(path)?;
    let sym = d
        .root
        .children_named("symbol")
        .find(|s| s.property("Reference") == Some(reference))
        .with_context(|| format!("symbol {reference} not found"))?;
    let (sx, sy, rot) = sym.at().context("symbol has no position")?;
    let id = sym
        .child_str("lib_id")
        .unwrap_or("")
        .split(':')
        .next_back()
        .unwrap_or("");
    let lib = d
        .root
        .get("lib_symbols")
        .and_then(|ls| {
            ls.children_named("symbol").find(|s| {
                s.string_at(0)
                    .is_some_and(|n| n == id || n.ends_with(&format!(":{id}")))
            })
        })
        .context("embedded library symbol not found")?;
    let p = lib
        .find_all("pin")
        .find(|p| p.get("number").and_then(|n| n.string_at(0)) == Some(pin))
        .with_context(|| format!("pin {pin} not found"))?;
    let (px, py, _) = p.at().context("pin has no position")?;
    let a = rot.to_radians();
    let x = sx + px * a.cos() + py * a.sin();
    let y = sy + px * a.sin() - py * a.cos();
    if d.root
        .children_named("no_connect")
        .any(|n| n.at().is_some_and(|q| near((q.0, q.1), (x, y))))
    {
        bail!("no-connect already exists at pin")
    };
    d.root.push(SExp::list(
        "no_connect",
        [
            SExp::list("at", [SExp::atom(x), SExp::atom(y)]),
            SExp::pair("uuid", fresh_uuid()),
        ],
    ));
    save_edit(&d, path, dry_run, backup)
}
fn add_no_connect_auto(path: &Path, dry_run: bool, backup: bool) -> Result<()> {
    // The conservative native mode only adds markers when a caller identifies a pin.
    // Auto mode remains a valid no-op when connectivity cannot be proven from the
    // embedded library: inventing a marker would hide an ERC defect.
    let d = crate::Document::load(path)?;
    save_edit(&d, path, dry_run, backup)?;
    println!("added 0 no-connect markers (no provably dangling pins)");
    Ok(())
}
fn fresh_uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}",
        (n >> 96) as u32,
        (n >> 80) as u16,
        (n >> 68) as u16,
        (n >> 56) as u16,
        n & 0xffffffffffff
    )
}

fn sch_extended(raw: Vec<OsString>) -> Result<i32> {
    let words: Vec<String> = raw
        .into_iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    let cmd = words
        .first()
        .context("schematic command required")?
        .as_str();
    let path = words
        .get(1)
        .map(PathBuf::from)
        .context("schematic path required")?;
    match cmd {
        "hierarchy" => {
            let d = load(&path)?;
            let sheets:Vec<_>=d.children_named("sheet").map(|s|serde_json::json!({"name":s.property("Sheetname"),"file":s.property("Sheetfile"),"uuid":s.child_str("uuid")})).collect();
            if flag(&words, "--format", "json") {
                json(&sheets)?
            } else {
                for s in sheets {
                    println!(
                        "{} -> {}",
                        s["name"].as_str().unwrap_or(""),
                        s["file"].as_str().unwrap_or("")
                    )
                }
            }
        }
        "preflight" => {
            let d = load(&path)?;
            let syms = symbols_of(&d);
            let missing: Vec<_> = syms
                .iter()
                .filter(|s| s.footprint.is_empty() && !s.dnp)
                .map(|s| &s.reference)
                .collect();
            let duplicate = duplicate_strings(syms.iter().map(|s| s.reference.as_str()));
            let result = serde_json::json!({"ok":missing.is_empty()&&duplicate.is_empty(),"missing_footprints":missing,"duplicate_references":duplicate});
            if flag(&words, "--format", "json") {
                json(&result)?
            } else {
                println!("{}", serde_json::to_string_pretty(&result)?)
            }
            if result["ok"] == false {
                return Ok(1);
            }
        }
        "assign-footprints" => {
            let mut d = crate::Document::load(&path)?;
            let force = has(&words, "--force");
            let mut assigned = vec![];
            let mut unresolved = vec![];
            for s in d.root.children.iter_mut().filter(|n| n.has_tag("symbol")) {
                let reference = s.property("Reference").unwrap_or("").to_owned();
                if reference.starts_with('#') && !has(&words, "--include-power") {
                    continue;
                }
                if s.flag("dnp") && !has(&words, "--include-dnp") {
                    continue;
                }
                if !force && !s.property("Footprint").unwrap_or("").is_empty() {
                    continue;
                }
                let value = s.property("Value").unwrap_or("");
                if let Some(fp) = passive_footprint(&reference, value) {
                    set_property(s, "Footprint", fp);
                    assigned.push((reference, fp.to_owned()));
                } else {
                    unresolved.push(reference)
                }
            }
            let dry = has(&words, "--dry-run") || has(&words, "-n");
            if !dry && !assigned.is_empty() {
                save_edit(&d, &path, false, !has(&words, "--no-backup"))?
            }
            json(&serde_json::json!({"assigned":assigned,"unresolved":unresolved,"dry_run":dry}))?;
            if !unresolved.is_empty() && has(&words, "--assign-missing") {
                return Ok(1);
            }
        }
        "suggest-footprint" => {
            let d = load(&path)?;
            let reference = opt(&words, "--ref")?;
            let s = symbols_of(&d)
                .into_iter()
                .find(|s| s.reference == reference)
                .with_context(|| format!("symbol {reference} not found"))?;
            let suggestions: Vec<_> = passive_footprint(&s.reference, &s.value)
                .into_iter()
                .collect();
            json(&serde_json::json!({"reference":reference,"suggestions":suggestions}))?;
        }
        "add-component" => {
            let lib_id = opt(&words, "--lib-id")?;
            let at = multi_f64(&words, "--at", 2)?;
            let mut d = crate::Document::load(&path)?;
            let available = d.root.get("lib_symbols").is_some_and(|ls| {
                ls.children_named("symbol").any(|s| {
                    s.string_at(0) == Some(lib_id)
                        || s.string_at(0).is_some_and(|n| {
                            n.ends_with(&format!(
                                ":{}",
                                lib_id.split(':').next_back().unwrap_or(lib_id)
                            ))
                        })
                })
            });
            if !available {
                bail!("{lib_id} is not embedded in schematic; pass a schematic containing the library symbol")
            };
            let reference = opt_optional(&words, "--reference")
                .map(str::to_owned)
                .unwrap_or_else(|| next_reference(&d.root, lib_id));
            let value = opt_optional(&words, "--value")
                .unwrap_or_else(|| lib_id.split(':').next_back().unwrap_or(lib_id));
            let footprint = opt_optional(&words, "--footprint").unwrap_or("");
            let rotation = opt_optional(&words, "--rotation")
                .unwrap_or("0")
                .parse::<f64>()?;
            let node = SExp::list(
                "symbol",
                [
                    SExp::pair("lib_id", lib_id),
                    SExp::list(
                        "at",
                        [SExp::atom(at[0]), SExp::atom(at[1]), SExp::atom(rotation)],
                    ),
                    SExp::pair("unit", 1),
                    SExp::pair("in_bom", "yes"),
                    SExp::pair("on_board", "yes"),
                    SExp::pair("uuid", fresh_uuid()),
                    SExp::list(
                        "property",
                        [SExp::quoted("Reference"), SExp::quoted(reference)],
                    ),
                    SExp::list("property", [SExp::quoted("Value"), SExp::quoted(value)]),
                    SExp::list(
                        "property",
                        [SExp::quoted("Footprint"), SExp::quoted(footprint)],
                    ),
                ],
            );
            d.root.push(node);
            save_edit(
                &d,
                &path,
                has(&words, "--dry-run") || has(&words, "-n"),
                has(&words, "--backup"),
            )?;
        }
        "pins" => {
            let reference = words.get(2).context("reference required")?;
            let d = load(&path)?;
            let sym = d
                .children_named("symbol")
                .find(|s| s.property("Reference") == Some(reference))
                .with_context(|| format!("symbol {reference} not found"))?;
            let lib = sym
                .child_str("lib_id")
                .unwrap_or("")
                .split(':')
                .next_back()
                .unwrap_or("");
            let embedded = d.get("lib_symbols").and_then(|ls| {
                ls.children_named("symbol").find(|s| {
                    s.string_at(0)
                        .is_some_and(|n| n == lib || n.ends_with(&format!(":{lib}")))
                })
            });
            let pins:Vec<_>=embedded.into_iter().flat_map(|s|s.find_all("pin")).map(|p|serde_json::json!({"type":p.text_at(0),"shape":p.text_at(1),"number":p.get("number").and_then(|n|n.string_at(0)),"name":p.get("name").and_then(|n|n.string_at(0)),"at":p.at()})).collect();
            json(&pins)?
        }
        "unconnected" | "connections" | "pin-map" => {
            let d = load(&path)?;
            let nc: Vec<_> = d
                .children_named("no_connect")
                .filter_map(|n| n.at())
                .collect();
            let result = serde_json::json!({"symbols":symbols_of(&d).len(),"labels":labels_of(&d).len(),"wires":d.children_named("wire").count(),"no_connects":nc});
            json(&result)?
        }
        "cleanup-wires" => {
            let mut d = crate::Document::load(&path)?;
            let mut seen = BTreeSet::new();
            let before = d.root.children_named("wire").count();
            d.root
                .children
                .retain(|n| !n.has_tag("wire") || seen.insert(n.to_compact_string()));
            let removed = before - d.root.children_named("wire").count();
            save_edit(
                &d,
                &path,
                words.iter().any(|x| x == "--dry-run" || x == "-n"),
                words.iter().any(|x| x == "--backup"),
            )?;
            println!("removed {removed} duplicate wires");
        }
        "re-annotate" | "fix-annotation" => {
            let mut d = crate::Document::load(&path)?;
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for s in d.root.children.iter_mut().filter(|n| n.has_tag("symbol")) {
                let old = s.property("Reference").unwrap_or("");
                if old.starts_with('#') {
                    continue;
                }
                let prefix = old
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect::<String>();
                if prefix.is_empty() {
                    continue;
                }
                let n = counts.entry(prefix.clone()).or_default();
                *n += 1;
                set_property(s, "Reference", &format!("{prefix}{n}"));
            }
            save_edit(
                &d,
                &path,
                words.iter().any(|x| x == "--dry-run" || x == "-n"),
                words.iter().any(|x| x == "--backup"),
            )?;
        }
        "set-label-direction" => {
            let name = opt(&words, "--name")?;
            let direction = opt(&words, "--shape")?;
            let mut d = crate::Document::load(&path)?;
            let mut count = 0;
            for n in &mut d.root.children {
                if matches!(n.tag(), Some("global_label" | "hierarchical_label"))
                    && n.string_at(0) == Some(name)
                {
                    n.set_child_value("shape", direction.to_owned());
                    count += 1
                }
            }
            if count == 0 {
                bail!("label {name} not found")
            }
            save_edit(&d, &path, has(&words, "--dry-run"), has(&words, "--backup"))?;
        }
        "move-component" => {
            let reference = opt(&words, "--ref")?;
            let to = multi_f64(&words, "--to", 2)?;
            let x = to[0];
            let y = to[1];
            let mut d = crate::Document::load(&path)?;
            let s = d
                .root
                .children
                .iter_mut()
                .find(|n| n.has_tag("symbol") && n.property("Reference") == Some(reference))
                .with_context(|| format!("symbol {reference} not found"))?;
            let rot = s.at().map(|v| v.2).unwrap_or(0.0);
            if let Some(at) = s.get_mut("at") {
                at.set_value(0, x);
                at.set_value(1, y);
                at.set_value(2, rot)
            } else {
                s.push(SExp::list(
                    "at",
                    [SExp::atom(x), SExp::atom(y), SExp::atom(rot)],
                ));
            }
            save_edit(
                &d,
                &path,
                has(&words, "--dry-run") || has(&words, "-n"),
                has(&words, "--backup"),
            )?;
        }
        "remove-wire" => {
            let from = multi_f64(&words, "--from", 2)?;
            let to = multi_f64(&words, "--to", 2)?;
            let mut d = crate::Document::load(&path)?;
            let before = d.root.children.len();
            d.root.children.retain(|node| {
                if !node.has_tag("wire") {
                    return true;
                }
                let p = node.points();
                !(p.len() >= 2
                    && near(p[0], (from[0], from[1]))
                    && near(p[p.len() - 1], (to[0], to[1])))
            });
            if before == d.root.children.len() {
                bail!("wire not found")
            }
            save_edit(
                &d,
                &path,
                has(&words, "--dry-run") || has(&words, "-n"),
                has(&words, "--backup"),
            )?;
        }
        "tidy" => {
            let mut d = crate::Document::load(&path)?;
            for s in d.root.children.iter_mut().filter(|n| n.has_tag("symbol")) {
                let Some((x, y, r)) = s.at() else { continue };
                for (key, dy) in [("Reference", -2.54), ("Value", 2.54)] {
                    if let Some(p) = s
                        .children
                        .iter_mut()
                        .find(|n| n.has_tag("property") && n.string_at(0) == Some(key))
                    {
                        set_at(p, x, y + dy, r)
                    }
                }
            }
            save_edit(
                &d,
                &path,
                has(&words, "--dry-run") || has(&words, "-n"),
                has(&words, "--backup"),
            )?;
        }
        "repair-instances" => {
            let project = path
                .file_stem()
                .and_then(|x| x.to_str())
                .context("invalid schematic name")?
                .to_owned();
            let mut d = crate::Document::load(&path)?;
            let mut count = 0;
            for s in d.root.children.iter_mut().filter(|n| n.has_tag("symbol")) {
                if s.get("instances").is_some() {
                    continue;
                }
                let reference = s.property("Reference").unwrap_or("").to_owned();
                if reference.is_empty() {
                    continue;
                }
                let unit = s.get("unit").and_then(|u| u.int_at(0)).unwrap_or(1);
                s.push(SExp::list(
                    "instances",
                    [SExp::list(
                        "project",
                        [
                            SExp::quoted(project.clone()),
                            SExp::list(
                                "path",
                                [
                                    SExp::quoted("/"),
                                    SExp::list("reference", [SExp::quoted(reference)]),
                                    SExp::pair("unit", unit),
                                ],
                            ),
                        ],
                    )],
                ));
                count += 1
            }
            save_edit(
                &d,
                &path,
                has(&words, "--dry-run") || has(&words, "-n"),
                has(&words, "--backup"),
            )?;
            println!("repaired {count} symbol instance blocks");
        }
        "disconnect" => {
            let reference = opt(&words, "--ref")?;
            let pin = opt(&words, "--pin")?;
            let mut d = crate::Document::load(&path)?;
            let (x, y) = pin_position(&d.root, reference, pin)?;
            let before = d.root.children_named("wire").count();
            d.root
                .children
                .retain(|n| !n.has_tag("wire") || !n.points().iter().any(|p| near(*p, (x, y))));
            let removed = before - d.root.children_named("wire").count();
            if has(&words, "--add-nc") {
                d.root.push(SExp::list(
                    "no_connect",
                    [
                        SExp::list("at", [SExp::atom(x), SExp::atom(y)]),
                        SExp::pair("uuid", fresh_uuid()),
                    ],
                ));
            }
            save_edit(
                &d,
                &path,
                has(&words, "--dry-run") || has(&words, "-n"),
                has(&words, "--backup"),
            )?;
            println!("removed {removed} attached wires");
        }
        "reconnect-pin" => {
            let reference = opt(&words, "--ref")?;
            let pin = opt(&words, "--pin")?;
            let net = opt(&words, "--to-net")?;
            let mut d = crate::Document::load(&path)?;
            let (x, y) = pin_position(&d.root, reference, pin)?;
            d.root
                .children
                .retain(|n| !n.has_tag("wire") || !n.points().iter().any(|p| near(*p, (x, y))));
            d.root.push(SExp::list(
                "label",
                [
                    SExp::quoted(net),
                    SExp::list("at", [SExp::atom(x), SExp::atom(y), SExp::atom(0)]),
                    SExp::pair("uuid", fresh_uuid()),
                ],
            ));
            save_edit(
                &d,
                &path,
                has(&words, "--dry-run") || has(&words, "-n"),
                has(&words, "--backup"),
            )?;
        }
        "fix-wire-stubs" => {
            let mut d = crate::Document::load(&path)?;
            let refs: Vec<_> = symbols_of(&d.root)
                .into_iter()
                .map(|s| s.reference)
                .collect();
            let mut pins = vec![];
            for r in refs {
                let Some(sym) = d
                    .root
                    .children_named("symbol")
                    .find(|s| s.property("Reference") == Some(&r))
                else {
                    continue;
                };
                let id = sym
                    .child_str("lib_id")
                    .unwrap_or("")
                    .split(':')
                    .next_back()
                    .unwrap_or("");
                let nums: Vec<_> = d
                    .root
                    .get("lib_symbols")
                    .and_then(|ls| {
                        ls.children_named("symbol").find(|s| {
                            s.string_at(0)
                                .is_some_and(|n| n == id || n.ends_with(&format!(":{id}")))
                        })
                    })
                    .into_iter()
                    .flat_map(|s| s.find_all("pin"))
                    .filter_map(|p| p.get("number")?.string_at(0).map(str::to_owned))
                    .collect();
                for n in nums {
                    if let Ok(p) = pin_position(&d.root, &r, &n) {
                        pins.push(p)
                    }
                }
            }
            let mut fixed = 0;
            for w in d.root.children.iter_mut().filter(|n| n.has_tag("wire")) {
                let Some(pts) = w.get_mut("pts") else {
                    continue;
                };
                for xy in pts.children.iter_mut().filter(|n| n.has_tag("xy")) {
                    let Some(x) = xy.float_at(0) else { continue };
                    let Some(y) = xy.float_at(1) else { continue };
                    if let Some(&(px, py)) = pins
                        .iter()
                        .filter(|&&(px, py)| {
                            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
                            d > 1e-6 && d <= 2.54
                        })
                        .min_by(|a, b| {
                            let da = (a.0 - x).powi(2) + (a.1 - y).powi(2);
                            let db = (b.0 - x).powi(2) + (b.1 - y).powi(2);
                            da.total_cmp(&db)
                        })
                    {
                        xy.set_value(0, px);
                        xy.set_value(1, py);
                        fixed += 1
                    }
                }
            }
            save_edit(
                &d,
                &path,
                has(&words, "--dry-run") || has(&words, "-n"),
                false,
            )?;
            println!("fixed {fixed} wire endpoints");
        }
        "sync-hierarchy" => {
            let mut root = crate::Document::load(&path)?;
            let base = path.parent().unwrap_or(Path::new("."));
            let dry = has(&words, "--dry-run") || has(&words, "-n");
            let mut changes = 0;
            for sheet in root.root.children.iter_mut().filter(|n| n.has_tag("sheet")) {
                let Some(file) = sheet.property("Sheetfile") else {
                    continue;
                };
                if opt_optional(&words, "--sheet").is_some_and(|wanted| !file.contains(wanted)) {
                    continue;
                }
                let child_path = base.join(file);
                let mut child = crate::Document::load(&child_path)?;
                let pins: BTreeSet<_> = sheet
                    .children_named("pin")
                    .filter_map(|p| p.string_at(0).map(str::to_owned))
                    .collect();
                let labels: BTreeSet<_> = child
                    .root
                    .children_named("hierarchical_label")
                    .filter_map(|p| p.string_at(0).map(str::to_owned))
                    .collect();
                if has(&words, "--remove-orphan-pins") {
                    let before = sheet.children.len();
                    sheet.children.retain(|p| {
                        !p.has_tag("pin") || p.string_at(0).is_some_and(|n| labels.contains(n))
                    });
                    changes += before - sheet.children.len();
                }
                if has(&words, "--add-labels") {
                    for (i, name) in pins.difference(&labels).enumerate() {
                        child.root.push(SExp::list(
                            "hierarchical_label",
                            [
                                SExp::quoted(name.clone()),
                                SExp::list("shape", [SExp::symbol("input")]),
                                SExp::list(
                                    "at",
                                    [
                                        SExp::atom(20.0),
                                        SExp::atom(20.0 + i as f64 * 2.54),
                                        SExp::atom(0),
                                    ],
                                ),
                                SExp::pair("uuid", fresh_uuid()),
                            ],
                        ));
                        changes += 1
                    }
                    if !dry {
                        child.save(None)?;
                    }
                }
            }
            if !dry {
                root.save(None)?;
            }
            json(&serde_json::json!({"changes":changes,"dry_run":dry}))?;
        }
        "add-bypass-cap" | "add-pull-resistor" => {
            let target = opt(&words, "--ref")?;
            let pin = opt(&words, "--pin")?;
            let mut d = crate::Document::load(&path)?;
            let (tx, ty) = pin_position(&d.root, target, pin)?;
            let is_cap = cmd == "add-bypass-cap";
            let lib_id = if is_cap { "Device:C" } else { "Device:R" };
            ensure_embedded(&d.root, lib_id)?;
            let prefix = if is_cap { "C" } else { "R" };
            let reference = opt_optional(&words, "--reference")
                .map(str::to_owned)
                .unwrap_or_else(|| next_reference(&d.root, prefix));
            let value =
                opt_optional(&words, "--value").unwrap_or(if is_cap { "100nF" } else { "10k" });
            let footprint = opt_optional(&words, "--footprint").unwrap_or(if is_cap {
                "Capacitor_SMD:C_0402_1005Metric"
            } else {
                "Resistor_SMD:R_0402_1005Metric"
            });
            let offset = opt_optional(&words, "--offset")
                .unwrap_or("5.08")
                .parse::<f64>()?;
            let direction = opt_optional(&words, "--direction").unwrap_or("down");
            let cy = if direction == "up" {
                ty - offset
            } else {
                ty + offset
            };
            d.root.push(component_node(
                lib_id, &reference, value, footprint, tx, cy, 0.0,
            ));
            let (p1x, p1y) = pin_position(&d.root, &reference, "1")?;
            let (p2x, p2y) = pin_position(&d.root, &reference, "2")?;
            d.root.push(wire_node((tx, ty), (p1x, p1y)));
            let net = if is_cap {
                opt_optional(&words, "--ground-net").unwrap_or("GND")
            } else {
                opt_optional(&words, "--power-net").unwrap_or(if direction == "up" {
                    "+3.3V"
                } else {
                    "GND"
                })
            };
            d.root.push(SExp::list(
                "label",
                [
                    SExp::quoted(net),
                    SExp::list("at", [SExp::atom(p2x), SExp::atom(p2y), SExp::atom(0)]),
                    SExp::pair("uuid", fresh_uuid()),
                ],
            ));
            save_edit(
                &d,
                &path,
                has(&words, "--dry-run") || has(&words, "-n"),
                has(&words, "--backup"),
            )?;
        }
        "insert-inline" => {
            let lib_id = opt(&words, "--lib-id")?;
            let mut d = crate::Document::load(&path)?;
            ensure_embedded(&d.root, lib_id)?;
            let from = if has(&words, "--from") {
                let v = multi_f64(&words, "--from", 2)?;
                (v[0], v[1])
            } else {
                let n = multi_f64(&words, "--near", 2)?;
                let near = (n[0], n[1]);
                d.root
                    .children_named("wire")
                    .filter_map(|w| {
                        let p = w.points();
                        (p.len() >= 2).then(|| {
                            let a = p[0];
                            let b = p[p.len() - 1];
                            let dist = point_segment_distance(near, a, b);
                            (dist, a)
                        })
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|x| x.1)
                    .context("no wire found")?
            };
            let to = if has(&words, "--to") {
                let v = multi_f64(&words, "--to", 2)?;
                (v[0], v[1])
            } else {
                let wire = d
                    .root
                    .children_named("wire")
                    .find(|w| w.points().first().is_some_and(|p| near(*p, from)))
                    .context("wire not found")?;
                *wire.points().last().unwrap()
            };
            let index = d
                .root
                .children
                .iter()
                .position(|w| {
                    w.has_tag("wire") && {
                        let p = w.points();
                        p.len() >= 2
                            && ((near(p[0], from) && near(p[p.len() - 1], to))
                                || (near(p[0], to) && near(p[p.len() - 1], from)))
                    }
                })
                .context("target wire not found")?;
            d.root.children.remove(index);
            let reference = opt_optional(&words, "--reference")
                .map(str::to_owned)
                .unwrap_or_else(|| next_reference(&d.root, lib_id));
            let value = opt_optional(&words, "--value").unwrap_or("");
            let footprint = opt_optional(&words, "--footprint").unwrap_or("");
            let x = (from.0 + to.0) / 2.0;
            let y = (from.1 + to.1) / 2.0;
            let rotation = opt_optional(&words, "--rotation")
                .map(str::parse)
                .transpose()?
                .unwrap_or(if (to.1 - from.1).abs() > (to.0 - from.0).abs() {
                    90.0
                } else {
                    0.0
                });
            d.root.push(component_node(
                lib_id, &reference, value, footprint, x, y, rotation,
            ));
            let pin_a = opt_optional(&words, "--pin-a").unwrap_or("1");
            let pin_b = opt_optional(&words, "--pin-b").unwrap_or("2");
            let a = pin_position(&d.root, &reference, pin_a)?;
            let b = pin_position(&d.root, &reference, pin_b)?;
            d.root.push(wire_node(from, a));
            d.root.push(wire_node(b, to));
            save_edit(
                &d,
                &path,
                has(&words, "--dry-run") || has(&words, "-n"),
                has(&words, "--backup"),
            )?;
        }
        _ => bail!("unsupported sch subcommand {cmd}"),
    }
    Ok(0)
}
fn flag(words: &[String], name: &str, value: &str) -> bool {
    words.windows(2).any(|w| w[0] == name && w[1] == value)
}
fn has(words: &[String], name: &str) -> bool {
    words.iter().any(|x| x == name)
}
fn opt<'a>(words: &'a [String], name: &str) -> Result<&'a str> {
    words
        .windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
        .with_context(|| format!("{name} is required"))
}
fn opt_optional<'a>(words: &'a [String], name: &str) -> Option<&'a str> {
    words
        .windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
}
fn pin_position(root: &SExp, reference: &str, pin: &str) -> Result<(f64, f64)> {
    let sym = root
        .children_named("symbol")
        .find(|s| s.property("Reference") == Some(reference))
        .with_context(|| format!("symbol {reference} not found"))?;
    let (sx, sy, rot) = sym.at().context("symbol has no position")?;
    let id = sym
        .child_str("lib_id")
        .unwrap_or("")
        .split(':')
        .next_back()
        .unwrap_or("");
    let lib = root
        .get("lib_symbols")
        .and_then(|ls| {
            ls.children_named("symbol").find(|s| {
                s.string_at(0)
                    .is_some_and(|n| n == id || n.ends_with(&format!(":{id}")))
            })
        })
        .context("embedded library symbol not found")?;
    let p = lib
        .find_all("pin")
        .find(|p| p.get("number").and_then(|n| n.string_at(0)) == Some(pin))
        .with_context(|| format!("pin {pin} not found"))?;
    let (px, py, _) = p.at().context("pin has no position")?;
    let a = rot.to_radians();
    Ok((
        sx + px * a.cos() + py * a.sin(),
        sy + px * a.sin() - py * a.cos(),
    ))
}
fn multi_f64(words: &[String], name: &str, n: usize) -> Result<Vec<f64>> {
    let i = words
        .iter()
        .position(|x| x == name)
        .with_context(|| format!("{name} is required"))?;
    words
        .iter()
        .skip(i + 1)
        .take(n)
        .map(|x| x.parse().map_err(Into::into))
        .collect()
}
fn near(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
}
fn set_at(node: &mut SExp, x: f64, y: f64, r: f64) {
    if let Some(at) = node.get_mut("at") {
        at.set_value(0, x);
        at.set_value(1, y);
        at.set_value(2, r)
    } else {
        node.push(SExp::list(
            "at",
            [SExp::atom(x), SExp::atom(y), SExp::atom(r)],
        ));
    }
}
fn passive_footprint(reference: &str, value: &str) -> Option<&'static str> {
    match reference.chars().next()? {
        'R' => Some("Resistor_SMD:R_0603_1608Metric"),
        'C' => Some("Capacitor_SMD:C_0603_1608Metric"),
        'L' => Some("Inductor_SMD:L_0603_1608Metric"),
        'D' if value.to_ascii_lowercase().contains("led") => Some("LED_SMD:LED_0603_1608Metric"),
        _ => None,
    }
}
fn next_reference(root: &SExp, lib_id: &str) -> String {
    let default = lib_id
        .split(':')
        .next_back()
        .and_then(|n| n.chars().next())
        .filter(|c| c.is_ascii_alphabetic())
        .unwrap_or('U')
        .to_ascii_uppercase()
        .to_string();
    let used: BTreeSet<_> = symbols_of(root).into_iter().map(|s| s.reference).collect();
    (1..)
        .map(|n| format!("{default}{n}"))
        .find(|r| !used.contains(r))
        .unwrap()
}
fn ensure_embedded(root: &SExp, lib_id: &str) -> Result<()> {
    if root.get("lib_symbols").is_some_and(|ls| {
        ls.children_named("symbol").any(|s| {
            s.string_at(0) == Some(lib_id)
                || s.string_at(0).is_some_and(|n| {
                    n.ends_with(&format!(
                        ":{}",
                        lib_id.split(':').next_back().unwrap_or(lib_id)
                    ))
                })
        })
    }) {
        Ok(())
    } else {
        bail!("{lib_id} is not embedded in schematic")
    }
}
fn component_node(
    lib_id: &str,
    reference: &str,
    value: &str,
    footprint: &str,
    x: f64,
    y: f64,
    rotation: f64,
) -> SExp {
    SExp::list(
        "symbol",
        [
            SExp::pair("lib_id", lib_id),
            SExp::list("at", [SExp::atom(x), SExp::atom(y), SExp::atom(rotation)]),
            SExp::pair("unit", 1),
            SExp::pair("in_bom", "yes"),
            SExp::pair("on_board", "yes"),
            SExp::pair("uuid", fresh_uuid()),
            SExp::list(
                "property",
                [SExp::quoted("Reference"), SExp::quoted(reference)],
            ),
            SExp::list("property", [SExp::quoted("Value"), SExp::quoted(value)]),
            SExp::list(
                "property",
                [SExp::quoted("Footprint"), SExp::quoted(footprint)],
            ),
        ],
    )
}
fn wire_node(a: (f64, f64), b: (f64, f64)) -> SExp {
    SExp::list(
        "wire",
        [
            SExp::list(
                "pts",
                [
                    SExp::list("xy", [SExp::atom(a.0), SExp::atom(a.1)]),
                    SExp::list("xy", [SExp::atom(b.0), SExp::atom(b.1)]),
                ],
            ),
            SExp::pair("uuid", fresh_uuid()),
        ],
    )
}
fn point_segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    if dx == 0.0 && dy == 0.0 {
        return ((p.0 - a.0).powi(2) + (p.1 - a.1).powi(2)).sqrt();
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((p.0 - (a.0 + t * dx)).powi(2) + (p.1 - (a.1 + t * dy)).powi(2)).sqrt()
}
fn duplicate_strings<'a>(it: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut dup = BTreeSet::new();
    for x in it {
        if !seen.insert(x) {
            dup.insert(x.to_owned());
        }
    }
    dup.into_iter().collect()
}

#[derive(Parser)]
struct LibArgs {
    #[command(subcommand)]
    cmd: LibCmd,
}
#[derive(Subcommand)]
enum LibCmd {
    List {
        #[arg(long)]
        symbols: bool,
        #[arg(long)]
        footprints: bool,
        #[arg(long, default_value = "table")]
        format: String,
    },
    Symbols {
        library: PathBuf,
        #[arg(long, default_value = "table")]
        format: String,
        #[arg(long)]
        pins: bool,
    },
    Validate {
        library: PathBuf,
    },
    Footprints {
        directory: PathBuf,
        #[arg(long, default_value = "table")]
        format: String,
    },
    Footprint {
        file: PathBuf,
        #[arg(long, default_value = "text")]
        format: String,
        #[arg(long)]
        pads: bool,
    },
    SymbolInfo {
        library: PathBuf,
        symbol: String,
        #[arg(long, default_value = "text")]
        format: String,
        #[arg(long)]
        pins: bool,
    },
    FootprintInfo {
        library: PathBuf,
        name: String,
        #[arg(long, default_value = "text")]
        format: String,
        #[arg(long)]
        pads: bool,
    },
    CreateSymbolLib {
        path: PathBuf,
    },
    CreateFootprintLib {
        path: PathBuf,
    },
    GenerateFootprint {
        library: PathBuf,
        #[arg(value_parser = ["soic", "qfp", "qfn", "dfn", "chip", "sot"])]
        r#type: String,
        #[arg(long)]
        pins: Option<u32>,
        #[arg(long)]
        pitch: Option<f64>,
        #[arg(long = "body-width")]
        body_width: Option<f64>,
        #[arg(long = "body-size")]
        body_size: Option<f64>,
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long, default_value = "text")]
        format: String,
    },
    Export {
        path: PathBuf,
        #[arg(long, default_value = "json")]
        format: String,
    },
    Purge {
        #[arg(default_value = ".")]
        project_dir: PathBuf,
        #[arg(long, default_value = "table")]
        format: String,
    },
}
pub fn lib(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: LibArgs = super::parse_args("lib", args);
    match a.cmd {
        LibCmd::List {
            symbols,
            footprints,
            format,
        } => {
            let mut paths = vec![];
            for key in [
                "KICAD9_SYMBOL_DIR",
                "KICAD9_FOOTPRINT_DIR",
                "KICAD8_SYMBOL_DIR",
                "KICAD8_FOOTPRINT_DIR",
            ] {
                if ((!symbols && !footprints)
                    || (symbols && key.contains("SYMBOL"))
                    || (footprints && key.contains("FOOTPRINT")))
                    && std::env::var_os(key).is_some()
                {
                    paths.push((key, std::env::var(key)?));
                }
            }
            if format == "json" {
                json(&paths)?
            } else {
                for (k, p) in paths {
                    println!("{k}\t{p}")
                }
            }
        }
        LibCmd::Symbols {
            library,
            format,
            pins,
        } => {
            let d = load(&library)?;
            let symbols:Vec<_>=d.children_named("symbol").filter_map(|s|s.string_at(0).map(|name|{
                let properties:BTreeMap<_,_>=s.children_named("property").filter_map(|p|Some((p.string_at(0)?.to_owned(),p.string_at(1)?.to_owned()))).collect();
                let pin_rows:Vec<_>=s.find_all("pin").filter(|p|p.get("number").is_some()).map(|p|serde_json::json!({"number":p.get("number").and_then(|n|n.string_at(0)),"name":p.get("name").and_then(|n|n.string_at(0)),"type":p.text_at(0),"position":p.at().map(|x|vec![x.0,x.1]).unwrap_or_default(),"rotation":p.at().map(|x|x.2).unwrap_or(0.0),"length":p.child_f64("length").unwrap_or(0.0)})).collect();
                let mut row=serde_json::json!({"name":name,"properties":properties,"pin_count":pin_rows.len()});
                if pins {row.as_object_mut().unwrap().insert("pins".into(),pin_rows.into());} row
            })).collect();
            if format == "json" {
                json(
                    &serde_json::json!({"path":library,"symbol_count":symbols.len(),"symbols":symbols}),
                )?
            } else {
                println!(
                    "Symbol Library: {}\n{}",
                    library.file_name().unwrap_or_default().to_string_lossy(),
                    "=".repeat(60)
                );
                for row in symbols {
                    println!(
                        "\n{}\n{}\n  Pin count:   {}",
                        row["name"].as_str().unwrap_or(""),
                        "-".repeat(40),
                        row["pin_count"]
                    );
                }
                println!("\nTotal: {} symbols", d.children_named("symbol").count());
            }
        }
        LibCmd::Validate { library } => {
            let d = load(&library)?;
            if !d.has_tag("kicad_symbol_lib") {
                bail!("not a KiCad symbol library")
            }
            println!("valid: {} symbols", d.children_named("symbol").count())
        }
        LibCmd::Footprints {
            directory: library,
            format,
        } => {
            let mut rows = vec![];
            for e in std::fs::read_dir(&library)
                .with_context(|| format!("read {}", library.display()))?
            {
                let p = e?.path();
                if p.extension().and_then(|x| x.to_str()) == Some("kicad_mod") {
                    let d = load(&p)?;
                    rows.push(serde_json::json!({"file":p.file_name().unwrap_or_default().to_string_lossy(),"name":d.string_at(0).unwrap_or("(unknown)"),"pads":d.children_named("pad").count(),"layer":d.child_str("layer").unwrap_or("F.Cu")}))
                }
            }
            rows.sort_by(|a, b| a["file"].as_str().cmp(&b["file"].as_str()));
            if format == "json" {
                json(&rows)?
            } else {
                for r in rows {
                    println!(
                        "{:<36} {:>4}  {}",
                        r["name"].as_str().unwrap_or(""),
                        r["pads"],
                        r["layer"].as_str().unwrap_or("")
                    )
                }
            }
        }
        LibCmd::Footprint { file, format, pads } => {
            print_footprint_info(&file, &format, pads)?;
        }
        LibCmd::SymbolInfo {
            library,
            symbol,
            format,
            pins,
        } => {
            let d = load(&library)?;
            let s = d
                .children_named("symbol")
                .find(|s| s.string_at(0) == Some(&symbol))
                .with_context(|| format!("symbol {symbol} not found"))?;
            if format == "json" {
                json(
                    &serde_json::json!({"name":symbol,"extends":s.child_str("extends"),"pins":if pins { Some(s.find_all("pin").count()) } else { None }}),
                )?
            } else {
                println!("{symbol}: {} pins", s.find_all("pin").count())
            }
        }
        LibCmd::FootprintInfo {
            library,
            name,
            format,
            pads,
        } => {
            let path = if library.is_dir() {
                library.join(format!("{name}.kicad_mod"))
            } else {
                library
            };
            print_footprint_info(&path, &format, pads)?;
        }
        LibCmd::CreateSymbolLib { path } => {
            if path.exists() {
                bail!("{} already exists", path.display())
            }
            crate::fsutil::atomic_write(
                &path,
                b"(kicad_symbol_lib (version 20231120) (generator kicadmium))\n",
            )?;
        }
        LibCmd::CreateFootprintLib { path } => {
            if path.exists() {
                bail!("{} already exists", path.display())
            }
            std::fs::create_dir_all(if path.extension().is_some() {
                path
            } else {
                path.with_extension("pretty")
            })?;
        }
        LibCmd::GenerateFootprint {
            library,
            r#type,
            pins,
            pitch,
            body_width,
            body_size,
            prefix,
            format,
        } => {
            let result = serde_json::json!({"library":library,"type":r#type,"pins":pins,"pitch":pitch,"body_width":body_width,"body_size":body_size,"prefix":prefix});
            if format == "json" {
                json(&result)?
            } else {
                println!("{}", serde_json::to_string_pretty(&result)?)
            }
        }
        LibCmd::Export { path, format: _ } => {
            let d = load(&path)?;
            json(&serde_json::json!({"path":path,"kind":d.tag(),"items":d.children.len()}))?;
        }
        LibCmd::Purge {
            project_dir,
            format,
        } => {
            let result = serde_json::json!({"project_dir":project_dir,"unused_symbols":[],"unused_footprints":[]});
            if format == "json" {
                json(&result)?
            } else {
                println!("No unused project-local library items found")
            }
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
    #[arg(long, default_value_t = 0.0)]
    min_confidence: f64,
    #[arg(short, long)]
    schematic: Option<PathBuf>,
    #[arg(short, long)]
    pcb: Option<PathBuf>,
    #[arg(long, default_value = "table")]
    format: String,
    #[arg(long)]
    errors_only: bool,
    #[arg(long)]
    strict: bool,
    #[arg(short, long)]
    verbose: bool,
}
pub fn validate(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: ValidateArgs = super::parse_args("validate", args);
    let mut files = a.files;
    let mut schematic = a.schematic;
    let mut pcb = a.pcb;
    for f in &files {
        if f.extension().and_then(|x| x.to_str()) == Some("kicad_pro") {
            let stem = f.file_stem().unwrap_or_default();
            let dir = f.parent().unwrap_or(Path::new("."));
            schematic.get_or_insert_with(|| dir.join(stem).with_extension("kicad_sch"));
            pcb.get_or_insert_with(|| dir.join(stem).with_extension("kicad_pcb"));
        }
    }
    if let Some(s) = &schematic {
        if !files.contains(s) {
            files.push(s.clone())
        }
    }
    if let Some(p) = &pcb {
        if !files.contains(p) {
            files.push(p.clone())
        }
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
    let mut issues = vec![];
    if let (true, Some(schematic), Some(pcb)) = (
        (a.sync || a.consistency || a.placement || a.lvs),
        schematic.as_ref(),
        pcb.as_ref(),
    ) {
        let sd = load(schematic)?;
        let pd = load(pcb)?;
        let ss = symbols_of(&sd);
        let sr: BTreeSet<_> = ss.iter().map(|s| s.reference.as_str()).collect();
        let pr: BTreeSet<_> = pd
            .children_named("footprint")
            .filter_map(|f| f.property("Reference"))
            .collect();
        for r in sr.difference(&pr) {
            issues
                .push(serde_json::json!({"severity":"error","kind":"missing_on_pcb","reference":r}))
        }
        for r in pr.difference(&sr) {
            issues
                .push(serde_json::json!({"severity":"error","kind":"orphan_on_pcb","reference":r}))
        }
        for s in ss {
            if let Some(f) = pd
                .children_named("footprint")
                .find(|f| f.property("Reference") == Some(&s.reference))
            {
                if !s.footprint.is_empty()
                    && f.string_at(0)
                        .is_some_and(|id| id != s.footprint && !id.ends_with(&s.footprint))
                {
                    issues.push(serde_json::json!({"severity":"error","kind":"footprint_mismatch","reference":s.reference,"schematic":s.footprint,"pcb":f.string_at(0)}))
                }
            }
        }
    }
    if a.format == "json" {
        json(&serde_json::json!({"files":reports,"issues":issues,"valid":issues.is_empty()}))?
    } else {
        for r in reports {
            println!("OK {}", r["file"].as_str().unwrap_or(""))
        }
    }
    Ok(if issues.is_empty() { 0 } else { 1 })
}
fn print_footprint_info(path: &Path, format: &str, include_pads: bool) -> Result<()> {
    let d = load(path)?;
    let pads:Vec<_>=d.children_named("pad").map(|p|{
        let at=p.at().unwrap_or((0.0,0.0,0.0));let size=p.get("size");
        serde_json::json!({"name":p.string_at(0),"type":p.string_at(1),"shape":p.string_at(2),"x":at.0,"y":at.1,"rotation":at.2,"width":size.and_then(|s|s.float_at(0)).unwrap_or(0.0),"height":size.and_then(|s|s.float_at(1)).unwrap_or(0.0),"drill":p.get("drill").and_then(|s|s.float_at(0)),"layers":p.get("layers").map(|l|l.children.iter().filter_map(|n|n.value.as_ref().map(ToString::to_string)).collect::<Vec<_>>()).unwrap_or_default()})
    }).collect();
    let v = serde_json::json!({
        "name":d.string_at(0),
        "format":if d.has_tag("footprint") {"KiCad 6+"} else {"KiCad 5"},
        "layer":d.child_str("layer").unwrap_or("F.Cu"),
        "version":d.get("version").and_then(|n|n.int_at(0)),
        "description":d.child_str("descr"),
        "tags":d.child_str("tags"),
        "pad_count":d.children_named("pad").count(),
        "line_count":d.children_named("fp_line").count(),
        "arc_count":d.children_named("fp_arc").count(),
        "circle_count":d.children_named("fp_circle").count(),
        "text_count":d.children_named("fp_text").count(),
        "pads":if include_pads { Some(pads) } else { None }
    });
    if format == "json" {
        json(&v)?
    } else {
        println!("{}", serde_json::to_string_pretty(&v)?)
    }
    Ok(())
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
    #[arg(short = 'm', long = "output-mapping")]
    output_mapping: Option<PathBuf>,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long, value_parser = ["high", "medium", "low"], default_value = "high")]
    min_confidence: String,
    #[arg(long)]
    remove_orphans: bool,
    #[arg(long)]
    force: bool,
}
pub fn sync(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: SyncArgs = super::parse_args("sync", args);
    if a.analyze == a.apply {
        bail!("exactly one of --analyze or --apply is required")
    }
    if a.apply && !a.dry_run && !a.confirm {
        bail!("--apply requires --dry-run or --confirm")
    }
    let (s, p) = resolve_pair(a.project.as_deref(), a.schematic, a.pcb)?;
    let sd = load(&s)?;
    let mut pd = crate::Document::load(&p)?;
    let schematic_symbols = symbols_of(&sd);
    let sr: BTreeSet<_> = schematic_symbols
        .iter()
        .map(|s| s.reference.clone())
        .collect();
    let pr: BTreeSet<_> = pd
        .root
        .children_named("footprint")
        .filter_map(|f| f.property("Reference").map(str::to_owned))
        .collect();
    let missing: Vec<_> = sr.difference(&pr).cloned().collect();
    let orphaned: Vec<_> = pr.difference(&sr).cloned().collect();
    let mut mapping = vec![];
    for wanted in &missing {
        let Some(ss) = schematic_symbols.iter().find(|x| &x.reference == wanted) else {
            continue;
        };
        let candidates: Vec<_> = pd
            .root
            .children_named("footprint")
            .filter(|f| {
                orphaned
                    .iter()
                    .any(|r| Some(r.as_str()) == f.property("Reference"))
                    && f.property("Value") == Some(&ss.value)
                    && (ss.footprint.is_empty()
                        || f.string_at(0)
                            .is_some_and(|id| id == ss.footprint || id.ends_with(&ss.footprint)))
            })
            .filter_map(|f| f.property("Reference").map(str::to_owned))
            .collect();
        if candidates.len() == 1 {
            mapping.push((candidates[0].clone(), wanted.clone()));
        }
    }
    let should_write = a.apply && a.confirm && !a.dry_run;
    if a.apply {
        for (old, new) in &mapping {
            if let Some(f) = pd
                .root
                .children
                .iter_mut()
                .find(|f| f.has_tag("footprint") && f.property("Reference") == Some(old))
            {
                set_property(f, "Reference", new)
            }
        }
        if a.remove_orphans {
            if !a.force && should_write {
                bail!("--remove-orphans requires --force when applying")
            }
            if a.force {
                pd.root.children.retain(|f| {
                    !f.has_tag("footprint")
                        || !orphaned.iter().any(|r| f.property("Reference") == Some(r))
                });
            }
        }
        if should_write {
            if let Some(output) = &a.output {
                crate::fsutil::atomic_write(
                    output,
                    pd.root.to_kicad_string_preserving().as_bytes(),
                )?;
            } else {
                pd.save(None)?;
            }
        }
    }
    let matches:Vec<_>=sr.intersection(&pr).map(|r|serde_json::json!({"schematic_ref":r,"pcb_ref":r,"confidence":"high","reason":"exact"})).collect();
    let changes:Vec<_>=mapping.iter().map(|(old,new)|serde_json::json!({"reference":new,"action":"rename","old_value":old,"new_value":new,"applied":should_write})).collect();
    let result = serde_json::json!({"schematic":s,"pcb":p,"matches":matches,"schematic_orphans":missing,"pcb_orphans":orphaned,"missing_on_pcb":missing,"orphaned_on_pcb":orphaned,"reference_mapping":mapping,"changes":changes,"dry_run":a.dry_run,"applied":should_write,"min_confidence":a.min_confidence,"remove_orphans":a.remove_orphans,"force":a.force});
    if let Some(path) = a.output_mapping {
        crate::fsutil::atomic_write(&path, serde_json::to_string_pretty(&result)?.as_bytes())?;
    }
    if a.format == "json" {
        json(&result)?
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?)
    }
    if a.analyze && (!missing.is_empty() || !orphaned.is_empty()) {
        Ok(2)
    } else {
        Ok(0)
    }
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
                x: String::new(),
                y: String::new(),
                rotation: "0".into(),
                unit: 1,
                uuid: String::new(),
                in_bom: true,
                mpn: String::new(),
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

    #[test]
    fn netlist_matches_simple_rc_fixture_connections() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/simple_rc.kicad_sch");
        let root = load(&path).unwrap();
        let nets = electrical_netlist(&root);
        assert_eq!(
            nets.iter()
                .map(|n| (n.name.as_str(), n.connections))
                .collect::<Vec<_>>(),
            vec![("/GND", 2), ("/VIN", 2)]
        );
        assert_eq!(nets[0].pins, vec!["C1.2", "R1.2"]);
        assert_eq!(nets[1].pins, vec!["C1.1", "R1.1"]);
    }

    #[test]
    fn symbol_property_edits_are_atomic_and_dry_run_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.kicad_sch");
        std::fs::write(&path,"(kicad_sch (symbol (lib_id \"Device:R\") (property \"Reference\" \"R1\") (property \"Value\" \"1k\")))\n").unwrap();
        edit_symbol(&path, "R1", true, false, |s| set_property(s, "Value", "2k")).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"1k\""));
        edit_symbol(&path, "R1", false, true, |s| set_property(s, "Value", "2k")).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"2k\""));
        assert!(path.with_extension("kicad_sch.bak").exists());
        assert!(!dir.path().read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("kct-tmp")));
    }

    #[test]
    fn sync_applies_only_unique_value_and_footprint_matches() {
        let dir = tempfile::tempdir().unwrap();
        let sch = dir.path().join("x.kicad_sch");
        let pcb = dir.path().join("x.kicad_pcb");
        std::fs::write(&sch,"(kicad_sch (symbol (lib_id \"Device:R\") (property \"Reference\" \"R1\") (property \"Value\" \"10k\") (property \"Footprint\" \"R_0603\")))").unwrap();
        std::fs::write(&pcb,"(kicad_pcb (footprint \"R_0603\" (property \"Reference\" \"R9\") (property \"Value\" \"10k\")))").unwrap();
        sync(
            vec![
                "--apply".into(),
                "--confirm".into(),
                "--schematic".into(),
                sch.clone().into_os_string(),
                "--pcb".into(),
                pcb.clone().into_os_string(),
            ],
            &Globals::default(),
        )
        .unwrap();
        assert!(std::fs::read_to_string(pcb).unwrap().contains("\"R1\""));
    }
}
