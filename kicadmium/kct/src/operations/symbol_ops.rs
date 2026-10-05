//! Port of `kicad_tools.operations.symbol_ops`: modify, replace and update
//! schematic symbol instances on the raw s-expression tree.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::{bail, Result};

use crate::drc::repair_silkscreen::{descendant_paths, node_at, node_at_mut, NodePath};
use crate::schema::library::{LibrarySymbol, SymbolLibrary};
use crate::schema::schematic::Schematic;
use crate::sexp::{SExp, Value};

/// A change in pin electrical type between old and new symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinTypeChange {
    pub pin_number: String,
    pub pin_name: String,
    pub old_type: String,
    pub new_type: String,
}

/// Details of a symbol replacement operation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SymbolReplacement {
    pub reference: String,
    pub old_lib_id: String,
    pub new_lib_id: String,
    pub old_pin_count: usize,
    pub new_pin_count: usize,
    pub preserved_properties: Vec<String>,
    pub changes_made: Vec<String>,
    pub lib_symbol_updated: bool,
    pub pin_type_changes: Vec<PinTypeChange>,
    pub wires_adjusted: usize,
}

/// Tolerance for matching wire endpoints to pin positions (mm).
pub const POINT_TOLERANCE: f64 = 0.127;

fn gs(node: &SExp, i: usize) -> Option<String> {
    node.text_at(i)
}

fn gf(node: &SExp, i: usize) -> f64 {
    node.float_at(i).unwrap_or(0.0)
}

/// Path of the first `symbol` with `(property "Reference" <reference>)`.
pub fn find_symbol_path_by_reference(sexp: &SExp, reference: &str) -> Option<NodePath> {
    descendant_paths(sexp, &|n| n.has_tag("symbol"))
        .into_iter()
        .find(|p| {
            node_at(sexp, p).find_all("property").any(|prop| {
                gs(prop, 0).as_deref() == Some("Reference")
                    && gs(prop, 1).as_deref() == Some(reference)
            })
        })
}

/// `find_symbol_by_reference`.
pub fn find_symbol_by_reference<'a>(sexp: &'a SExp, reference: &str) -> Option<&'a SExp> {
    find_symbol_path_by_reference(sexp, reference).map(|p| node_at(sexp, &p))
}

pub fn get_symbol_lib_id(symbol: &SExp) -> String {
    symbol
        .find("lib_id")
        .filter(|n| !n.children.is_empty())
        .and_then(|n| gs(n, 0))
        .unwrap_or_default()
}

pub fn get_symbol_pins(symbol: &SExp) -> Vec<&SExp> {
    symbol.find_all("pin").collect()
}

/// Wires with an endpoint within `tolerance` of `point`:
/// `(wire path, endpoint index)`, one entry per wire.
pub fn find_wires_at_point(
    sexp: &SExp,
    point: (f64, f64),
    tolerance: f64,
) -> Vec<(NodePath, usize)> {
    let mut out = Vec::new();
    for path in descendant_paths(sexp, &|n| n.has_tag("wire")) {
        let wire = node_at(sexp, &path);
        let Some(pts) = wire.find("pts").filter(|n| !n.children.is_empty()) else {
            continue;
        };
        let xys: Vec<&SExp> = pts.find_all("xy").collect();
        if xys.len() < 2 {
            continue;
        }
        if let Some(idx) = xys.iter().position(|xy| {
            (gf(xy, 0) - point.0).abs() <= tolerance && (gf(xy, 1) - point.1).abs() <= tolerance
        }) {
            out.push((path, idx));
        }
    }
    out
}

fn find_lib_symbol_sexp<'a>(sexp: &'a SExp, lib_id: &str) -> Option<&'a SExp> {
    sexp.find("lib_symbols")?
        .find_all("symbol")
        .find(|s| gs(s, 0).as_deref() == Some(lib_id))
}

/// Effective (base-resolved) library symbol embedded in the schematic.
fn embedded_effective(sexp: &SExp, lib_id: &str) -> Option<LibrarySymbol> {
    let sym = LibrarySymbol::from_sexp(find_lib_symbol_sexp(sexp, lib_id)?);
    if let Some(ext) = &sym.extends {
        if let Some(base) = find_lib_symbol_sexp(sexp, ext) {
            return Some(LibrarySymbol::from_sexp(base));
        }
    }
    Some(sym)
}

/// Replace a symbol's library ID (and optionally value/footprint). With
/// `lib_path`, the embedded `lib_symbols` entry is replaced from the
/// library, instance pins reconciled, and wire endpoints moved to the new
/// pin positions. Writes the schematic unless `dry_run`.
#[allow(clippy::too_many_arguments)]
pub fn replace_symbol_lib_id(
    schematic_path: &Path,
    reference: &str,
    new_lib_id: &str,
    new_value: Option<&str>,
    new_footprint: Option<&str>,
    dry_run: bool,
    lib_path: Option<&Path>,
) -> Result<SymbolReplacement> {
    if !schematic_path.exists() {
        bail!("Schematic not found: {}", schematic_path.display());
    }
    let text = std::fs::read_to_string(schematic_path)?;
    let mut sexp = crate::sexp::parse(&text)?;
    let Some(sym_path) = find_symbol_path_by_reference(&sexp, reference) else {
        bail!("Symbol '{reference}' not found in schematic");
    };

    let symbol = node_at(&sexp, &sym_path);
    let old_lib_id = get_symbol_lib_id(symbol);
    let old_pin_numbers: Vec<String> = get_symbol_pins(symbol)
        .iter()
        .map(|p| gs(p, 0).unwrap_or_default())
        .collect();
    let old_pin_count = old_pin_numbers.len();
    let mut changes = Vec::new();
    let mut preserved = Vec::new();
    let mut pin_type_changes = Vec::new();
    let mut lib_symbol_updated = false;

    let (mut instance_pos, mut instance_rot, mut mirror) = ((0.0, 0.0), 0.0, String::new());
    if let Some(at) = symbol.find("at").filter(|n| !n.children.is_empty()) {
        instance_pos = (gf(at, 0), gf(at, 1));
        instance_rot = gf(at, 2);
    }
    if let Some(m) = symbol.find("mirror").filter(|n| !n.children.is_empty()) {
        mirror = gs(m, 0).unwrap_or_default();
    }

    {
        let symbol = node_at_mut(&mut sexp, &sym_path);
        if let Some(p) = first_path(symbol, "lib_id") {
            let n = node_at_mut(symbol, &p);
            if !n.children.is_empty() {
                n.set_value(0, new_lib_id);
                changes.push(format!("lib_id: {old_lib_id} → {new_lib_id}"));
            }
        }
        for p in descendant_paths(symbol, &|n| n.has_tag("property")) {
            let prop = node_at_mut(symbol, &p);
            let name = gs(prop, 0).unwrap_or_default();
            match (name.as_str(), new_value, new_footprint) {
                ("Value", Some(v), _) if !v.is_empty() => {
                    let old = gs(prop, 1).unwrap_or_default();
                    prop.set_value(1, v);
                    changes.push(format!("Value: {old} → {v}"));
                }
                ("Footprint", _, Some(f)) if !f.is_empty() => {
                    let old = gs(prop, 1).unwrap_or_default();
                    prop.set_value(1, f);
                    changes.push(format!("Footprint: {old} → {f}"));
                }
                _ => preserved.push(name),
            }
        }
    }

    let mut wires_adjusted = 0;
    let mut new_pin_count = old_pin_count;
    if let Some(lib_path) = lib_path {
        if !lib_path.exists() {
            bail!("Library not found: {}", lib_path.display());
        }
        let lib = SymbolLibrary::load(lib_path)?;
        let short = new_lib_id.split_once(':').map_or(new_lib_id, |(_, s)| s);
        let Some(new_sym) = lib.get_symbol(new_lib_id).or_else(|| lib.get_symbol(short)) else {
            bail!(
                "Symbol '{new_lib_id}' not found in library '{}'",
                lib_path.display()
            );
        };
        let base_sym = match &new_sym.extends {
            Some(_) => Some(lib.resolve_base(new_sym)?.clone()),
            None => None,
        };
        let effective = base_sym.as_ref().unwrap_or(new_sym).clone();

        let old_effective = embedded_effective(&sexp, &old_lib_id);
        if let Some(old) = &old_effective {
            let old_map: HashMap<&str, &str> = old
                .pins
                .iter()
                .map(|p| (p.number.as_str(), p.pin_type.as_str()))
                .collect();
            for np in &effective.pins {
                if let Some(ot) = old_map.get(np.number.as_str()) {
                    if *ot != np.pin_type {
                        pin_type_changes.push(PinTypeChange {
                            pin_number: np.number.clone(),
                            pin_name: np.name.clone(),
                            old_type: ot.to_string(),
                            new_type: np.pin_type.clone(),
                        });
                    }
                }
            }
        }

        let mut sch = Schematic::new(sexp, None)?;
        if let Some(base) = &base_sym {
            let mut embed = base.clone();
            embed.extends = None;
            sch.embed_lib_symbol(&embed);
            changes.push(format!("lib_symbols: embedded base symbol '{}'", base.name));
        }
        let mut renamed = new_sym.clone();
        renamed.name = new_lib_id.to_string();
        sch.replace_lib_symbol(&old_lib_id, &renamed);
        lib_symbol_updated = true;
        changes.push(format!(
            "lib_symbols: replaced embedded definition for {old_lib_id}"
        ));
        sexp = sch.sexp().clone();

        let new_numbers: BTreeSet<&str> =
            effective.pins.iter().map(|p| p.number.as_str()).collect();
        let old_set: BTreeSet<&str> = old_pin_numbers.iter().map(String::as_str).collect();
        {
            let symbol = node_at_mut(&mut sexp, &sym_path);
            let mut kept = Vec::with_capacity(symbol.children.len());
            for c in std::mem::take(&mut symbol.children) {
                if c.has_tag("pin") {
                    let num = gs(&c, 0).unwrap_or_default();
                    if !new_numbers.contains(num.as_str()) {
                        changes.push(format!("Removed instance pin {num} (not in new symbol)"));
                        continue;
                    }
                }
                kept.push(c);
            }
            symbol.children = kept;
            for p in &effective.pins {
                if !old_set.contains(p.number.as_str()) {
                    add_symbol_pin(symbol, &p.number);
                    changes.push(format!(
                        "Added instance pin {} (new in replacement symbol)",
                        p.number
                    ));
                }
            }
            new_pin_count = get_symbol_pins(symbol).len();
        }

        if let Some(old) = &old_effective {
            let old_pos = old.get_all_pin_positions(instance_pos, instance_rot, &mirror);
            let new_pos = effective.get_all_pin_positions(instance_pos, instance_rot, &mirror);
            let mut adjusted: BTreeSet<NodePath> = BTreeSet::new();
            for (num, op) in old_pos.iter() {
                let Some(np) = new_pos.get(num) else {
                    continue;
                };
                if (np.0 - op.0).abs() < 0.001 && (np.1 - op.1).abs() < 0.001 {
                    continue;
                }
                for (wpath, idx) in find_wires_at_point(&sexp, *op, POINT_TOLERANCE) {
                    if adjusted.contains(&wpath) {
                        continue;
                    }
                    let wire = node_at_mut(&mut sexp, &wpath);
                    let Some(pp) = first_path(wire, "pts") else {
                        continue;
                    };
                    let pts = node_at_mut(wire, &pp);
                    let xy_paths = descendant_paths(pts, &|n| n.has_tag("xy"));
                    let Some(xp) = xy_paths.get(idx) else {
                        continue;
                    };
                    let xy = node_at_mut(pts, xp);
                    xy.set_value(0, Value::Float(np.0));
                    xy.set_value(1, Value::Float(np.1));
                    adjusted.insert(wpath);
                    wires_adjusted += 1;
                }
            }
            if wires_adjusted > 0 {
                changes.push(format!(
                    "Adjusted {wires_adjusted} wire endpoint(s) to match new pin positions"
                ));
            }
        }
    }

    let result = SymbolReplacement {
        reference: reference.to_string(),
        old_lib_id,
        new_lib_id: new_lib_id.to_string(),
        old_pin_count,
        new_pin_count,
        preserved_properties: preserved,
        changes_made: changes,
        lib_symbol_updated,
        pin_type_changes,
        wires_adjusted,
    };
    if !dry_run {
        crate::fsutil::atomic_write(schematic_path, sexp.to_kicad_string().as_bytes())?;
    }
    Ok(result)
}

fn first_path(node: &SExp, tag: &str) -> Option<NodePath> {
    descendant_paths(node, &|n| n.has_tag(tag))
        .into_iter()
        .next()
}

/// Renumber pins per `pin_mapping` (old -> new), refreshing pin UUIDs.
pub fn update_symbol_pins(symbol: &mut SExp, pin_mapping: &HashMap<String, String>) -> Vec<String> {
    let mut changes = Vec::new();
    for p in descendant_paths(symbol, &|n| n.has_tag("pin")) {
        let pin = node_at_mut(symbol, &p);
        let old = gs(pin, 0).unwrap_or_default();
        if let Some(new) = pin_mapping.get(&old) {
            pin.set_value(0, new.as_str());
            changes.push(format!("Pin {old} → {new}"));
            if let Some(up) = first_path(pin, "uuid") {
                let u = node_at_mut(pin, &up);
                if !u.children.is_empty() {
                    u.set_value(0, uuid::Uuid::new_v4().to_string());
                }
            }
        }
    }
    changes
}

/// Remove all direct `pin` children; returns the count.
pub fn clear_symbol_pins(symbol: &mut SExp) -> usize {
    let mut count = 0;
    while symbol.remove_child("pin") {
        count += 1;
    }
    count
}

/// Append `(pin "<n>" (uuid "<uuid4>"))`.
pub fn add_symbol_pin(symbol: &mut SExp, pin_number: &str) {
    symbol.push(SExp::list(
        "pin",
        [
            SExp::atom(pin_number),
            SExp::list("uuid", [SExp::atom(uuid::Uuid::new_v4().to_string())]),
        ],
    ));
}

/// New symbol instance from the first symbol of a template library.
#[allow(clippy::too_many_arguments)]
pub fn create_replacement_symbol(
    template_path: &Path,
    position: (f64, f64),
    rotation: f64,
    reference: &str,
    value: &str,
    footprint: &str,
    unit: i64,
) -> Result<SExp> {
    let text = std::fs::read_to_string(template_path)?;
    let template = crate::sexp::parse(&text)?;
    let Some(sym_def) = template.find("symbol") else {
        bail!("No symbol found in {}", template_path.display());
    };
    let lib_id = gs(sym_def, 0).unwrap_or_default();
    let num = |v: f64| SExp::atom(Value::Float(v));
    let mut inst = SExp::list(
        "symbol",
        [
            SExp::list("lib_id", [SExp::atom(lib_id.as_str())]),
            SExp::list("at", [num(position.0), num(position.1), num(rotation)]),
            SExp::list("unit", [SExp::atom(unit)]),
            SExp::list("in_bom", [SExp::atom("yes")]),
            SExp::list("on_board", [SExp::atom("yes")]),
            SExp::list("dnp", [SExp::atom("no")]),
            SExp::list("uuid", [SExp::atom(uuid::Uuid::new_v4().to_string())]),
        ],
    );
    let mut add_property = |name: &str, val: &str, dx: f64, dy: f64| {
        inst.push(SExp::list(
            "property",
            [
                SExp::atom(name),
                SExp::atom(val),
                SExp::list(
                    "at",
                    [num(position.0 + dx), num(position.1 + dy), SExp::atom(0i64)],
                ),
                SExp::list(
                    "effects",
                    [SExp::list(
                        "font",
                        [SExp::list("size", [num(1.27), num(1.27)])],
                    )],
                ),
            ],
        ));
    };
    add_property("Reference", reference, 0.0, -5.0);
    add_property(
        "Value",
        if value.is_empty() { &lib_id } else { value },
        0.0,
        -2.5,
    );
    add_property("Footprint", footprint, 0.0, 0.0);
    add_property("Datasheet", "~", 0.0, 0.0);
    for unit_sym in sym_def.find_all("symbol") {
        for pin in unit_sym.find_all("pin") {
            if let Some(n) = pin.find("number").filter(|n| !n.children.is_empty()) {
                if let Some(num) = gs(n, 0).filter(|s| !s.is_empty()) {
                    add_symbol_pin(&mut inst, &num);
                }
            }
        }
    }
    Ok(inst)
}
