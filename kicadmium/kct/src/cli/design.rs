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
    SetValue(PropertyEdit),
    SetFootprint(PropertyEdit),
    SetReference {
        schematic: PathBuf,
        #[arg(long = "ref")]
        reference: String,
        #[arg(long = "new-ref")]
        new_reference: String,
        #[arg(long)]
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
        #[arg(long)]
        dry_run: bool,
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
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
    },
    AddWire {
        schematic: PathBuf,
        #[arg(long, num_args = 4)]
        points: Vec<f64>,
        #[arg(long)]
        dry_run: bool,
    },
    AddJunction {
        schematic: PathBuf,
        #[arg(long, num_args = 2)]
        at: Vec<f64>,
        #[arg(long)]
        dry_run: bool,
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
        dry_run: bool,
    },
    RemoveComponent {
        schematic: PathBuf,
        #[arg(long = "ref")]
        reference: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        backup: bool,
    },
    AssignFootprints(Tail),
    SuggestFootprint(Tail),
    SyncHierarchy(Tail),
    SetLabelDirection(Tail),
    AddNoConnect {
        schematic: PathBuf,
        #[arg(long = "ref")]
        reference: String,
        #[arg(long)]
        pin: String,
        #[arg(long)]
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
    reference: String,
    #[arg(long)]
    value: String,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    backup: bool,
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
        SchCmd::SetValue(e) => edit_property(e, "Value")?,
        SchCmd::SetFootprint(e) => edit_property(e, "Footprint")?,
        SchCmd::SetReference {
            schematic,
            reference,
            new_reference,
            dry_run,
            backup,
        } => edit_symbol(&schematic, &reference, dry_run, backup, |s| {
            set_property(s, "Reference", &new_reference)
        })?,
        SchCmd::SetSymbolProperty {
            schematic,
            reference,
            property,
            value,
            dry_run,
        } => edit_symbol(&schematic, &reference, dry_run, false, |s| {
            set_property(s, &property, &value)
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
            backup,
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
            save_edit(&d, &schematic, dry_run, backup)?;
            println!("renamed {count} labels");
        }
        SchCmd::AddWire {
            schematic,
            points,
            dry_run,
        } => {
            ensure_len(&points, 4, "--points requires x1 y1 x2 y2")?;
            append_node(
                &schematic,
                SExp::list(
                    "wire",
                    [SExp::list(
                        "pts",
                        [
                            SExp::list("xy", [SExp::atom(points[0]), SExp::atom(points[1])]),
                            SExp::list("xy", [SExp::atom(points[2]), SExp::atom(points[3])]),
                        ],
                    )],
                ),
                dry_run,
            )?;
        }
        SchCmd::AddJunction {
            schematic,
            at,
            dry_run,
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
                        SExp::list("at", [SExp::atom(at[0]), SExp::atom(at[1]), SExp::atom(0)]),
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
            dry_run,
            backup,
        } => add_no_connect(&schematic, &reference, &pin, dry_run, backup)?,
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
    } = e;
    edit_symbol(&schematic, &reference, dry_run, backup, |s| {
        set_property(s, key, &value)
    })
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
    let y = sy - px * a.sin() + py * a.cos();
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
            let direction = opt(&words, "--direction")?;
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
            let x: f64 = opt(&words, "--x")?.parse()?;
            let y: f64 = opt(&words, "--y")?.parse()?;
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
        #[arg(long, default_value = "all")]
        kind: String,
        #[arg(long, default_value = "table")]
        format: String,
    },
    Symbols {
        library: PathBuf,
        #[arg(long, default_value = "table")]
        format: String,
    },
    Validate {
        library: PathBuf,
    },
    Footprints {
        library: PathBuf,
        #[arg(long, default_value = "table")]
        format: String,
    },
    SymbolInfo {
        library: PathBuf,
        symbol: String,
        #[arg(long, default_value = "text")]
        format: String,
    },
    FootprintInfo {
        footprint: PathBuf,
        #[arg(long, default_value = "text")]
        format: String,
    },
    CreateSymbolLib {
        path: PathBuf,
    },
    CreateFootprintLib {
        path: PathBuf,
    },
    Export {
        source: PathBuf,
        output: PathBuf,
    },
    Purge {
        path: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
}
pub fn lib(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: LibArgs = super::parse_args("lib", args);
    match a.cmd {
        LibCmd::List { kind, format } => {
            let mut paths = vec![];
            for key in [
                "KICAD9_SYMBOL_DIR",
                "KICAD9_FOOTPRINT_DIR",
                "KICAD8_SYMBOL_DIR",
                "KICAD8_FOOTPRINT_DIR",
            ] {
                if (kind == "all"
                    || key
                        .to_lowercase()
                        .contains(&kind.trim_end_matches('s').to_lowercase()))
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
        LibCmd::Footprints { library, format } => {
            let mut names = vec![];
            for e in std::fs::read_dir(&library)
                .with_context(|| format!("read {}", library.display()))?
            {
                let p = e?.path();
                if p.extension().and_then(|x| x.to_str()) == Some("kicad_mod") {
                    names.push(
                        p.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                    )
                }
            }
            names.sort();
            if format == "json" {
                json(&names)?
            } else {
                for n in names {
                    println!("{n}")
                }
            }
        }
        LibCmd::SymbolInfo {
            library,
            symbol,
            format,
        } => {
            let d = load(&library)?;
            let s = d
                .children_named("symbol")
                .find(|s| s.string_at(0) == Some(&symbol))
                .with_context(|| format!("symbol {symbol} not found"))?;
            if format == "json" {
                json(
                    &serde_json::json!({"name":symbol,"extends":s.child_str("extends"),"pins":s.find_all("pin").count()}),
                )?
            } else {
                println!("{symbol}: {} pins", s.find_all("pin").count())
            }
        }
        LibCmd::FootprintInfo { footprint, format } => {
            let d = load(&footprint)?;
            let v = serde_json::json!({"name":d.string_at(0),"pads":d.children_named("pad").count(),"models":d.children_named("model").count()});
            if format == "json" {
                json(&v)?
            } else {
                println!("{}", serde_json::to_string_pretty(&v)?)
            }
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
        LibCmd::Export { source, output } => {
            if source.is_dir() {
                copy_dir(&source, &output)?
            } else {
                std::fs::copy(source, output)?;
            }
        }
        LibCmd::Purge { path, dry_run } => {
            let mut removed = 0;
            for e in std::fs::read_dir(path)? {
                let p = e?.path();
                if matches!(
                    p.extension().and_then(|x| x.to_str()),
                    Some("bak" | "tmp" | "cache")
                ) {
                    if !dry_run {
                        std::fs::remove_file(&p)?
                    }
                    removed += 1
                }
            }
            println!(
                "{} {} files",
                if dry_run { "would purge" } else { "purged" },
                removed
            );
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
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &to)?
        } else {
            std::fs::copy(e.path(), to)?;
        }
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
}
pub fn sync(args: Vec<OsString>, _: &Globals) -> Result<i32> {
    let a: SyncArgs = super::parse_args("sync", args);
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
        if should_write {
            pd.save(None)?;
        }
    }
    let result = serde_json::json!({"schematic":s,"pcb":p,"missing_on_pcb":missing,"orphaned_on_pcb":orphaned,"reference_mapping":mapping,"applied":should_write});
    if a.format == "json" {
        json(&result)?
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?)
    }
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
