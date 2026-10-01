//! Bill of Materials extraction (port of `kicad_tools.schema.bom`).

use std::cmp::Ordering;
use std::path::Path;

use anyhow::Result;

use crate::sexp::{self, SExp};

use super::hierarchy::build_hierarchy;
use super::schematic::Schematic;
use super::symbol::{get_string, OrderedMap};

/// A single component in the BOM.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BOMItem {
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub lib_id: String,
    pub datasheet: String,
    pub description: String,
    pub manufacturer: String,
    /// Manufacturer part number.
    pub mpn: String,
    /// LCSC part number.
    pub lcsc: String,
    pub dnp: bool,
    pub in_bom: bool,
    pub on_board: bool,
    /// Additional (unmapped) properties.
    pub properties: OrderedMap<String>,
}

/// JSON form of a [`BOMItem`] (upstream `to_dict`, key order preserved).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BOMItemDict {
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub description: String,
    pub manufacturer: String,
    pub mpn: String,
    pub lcsc: String,
    pub dnp: bool,
}

impl BOMItem {
    pub fn new(
        reference: impl Into<String>,
        value: impl Into<String>,
        footprint: impl Into<String>,
        lib_id: impl Into<String>,
    ) -> Self {
        BOMItem {
            reference: reference.into(),
            value: value.into(),
            footprint: footprint.into(),
            lib_id: lib_id.into(),
            datasheet: String::new(),
            description: String::new(),
            manufacturer: String::new(),
            mpn: String::new(),
            lcsc: String::new(),
            dnp: false,
            in_bom: true,
            on_board: true,
            properties: OrderedMap::new(),
        }
    }

    /// `power:` / `kicad_tools_pwr:` library, or a `#PWR` reference.
    pub fn is_power_symbol(&self) -> bool {
        self.lib_id.starts_with("power:")
            || self.lib_id.starts_with("kicad_tools_pwr:")
            || self.reference.starts_with("#PWR")
    }

    /// Not placed as a real part (excluded from BOM or a power symbol).
    pub fn is_virtual(&self) -> bool {
        !self.in_bom || self.is_power_symbol()
    }

    pub fn to_dict(&self) -> BOMItemDict {
        BOMItemDict {
            reference: self.reference.clone(),
            value: self.value.clone(),
            footprint: self.footprint.clone(),
            description: self.description.clone(),
            manufacturer: self.manufacturer.clone(),
            mpn: self.mpn.clone(),
            lcsc: self.lcsc.clone(),
            dnp: self.dnp,
        }
    }

    /// Map a property onto the standard fields (as upstream), else keep it
    /// in `properties`.
    fn apply_property(&mut self, name: &str, value: &str) {
        match name.to_lowercase().as_str() {
            "description" | "desc" => self.description = value.into(),
            "manufacturer" | "mfr" | "mfg" => self.manufacturer = value.into(),
            "mpn" | "mfr_pn" | "manufacturer_pn" | "pn" => self.mpn = value.into(),
            "lcsc" | "lcsc_pn" | "lcsc part" | "jlc" | "jlcpcb" => self.lcsc = value.into(),
            _ => {
                self.properties.insert(name, value.to_string());
            }
        }
    }
}

/// Upstream reference sort key: (first char, integer of all digits).
fn reference_key(reference: &str) -> (String, String) {
    let first = reference
        .chars()
        .next()
        .map(String::from)
        .unwrap_or_default();
    let digits: String = reference.chars().filter(char::is_ascii_digit).collect();
    let trimmed = digits.trim_start_matches('0');
    (first, trimmed.to_string())
}

fn cmp_reference_key(a: &(String, String), b: &(String, String)) -> Ordering {
    a.0.cmp(&b.0)
        .then_with(|| a.1.len().cmp(&b.1.len()))
        .then_with(|| a.1.cmp(&b.1))
}

/// A group of identical components.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BOMGroup {
    pub value: String,
    pub footprint: String,
    pub items: Vec<BOMItem>,
}

/// JSON form of a [`BOMGroup`] (upstream `to_dict`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BOMGroupDict {
    pub value: String,
    pub footprint: String,
    pub qty: usize,
    pub refs: String,
    pub description: String,
    pub mpn: String,
    pub lcsc: String,
    pub items: Vec<BOMItemDict>,
}

impl BOMGroup {
    pub fn new(
        value: impl Into<String>,
        footprint: impl Into<String>,
        items: Vec<BOMItem>,
    ) -> Self {
        BOMGroup {
            value: value.into(),
            footprint: footprint.into(),
            items,
        }
    }

    pub fn quantity(&self) -> usize {
        self.items.len()
    }

    /// Comma-separated references sorted by prefix letter then number.
    pub fn references(&self) -> String {
        let mut refs: Vec<&BOMItem> = self.items.iter().collect();
        refs.sort_by(|a, b| {
            cmp_reference_key(&reference_key(&a.reference), &reference_key(&b.reference))
        });
        refs.iter()
            .map(|i| i.reference.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn first_nonempty(&self, f: impl Fn(&BOMItem) -> &str) -> String {
        self.items
            .iter()
            .map(f)
            .find(|v| !v.is_empty())
            .unwrap_or("")
            .to_string()
    }

    pub fn lcsc(&self) -> String {
        self.first_nonempty(|i| &i.lcsc)
    }

    pub fn mpn(&self) -> String {
        self.first_nonempty(|i| &i.mpn)
    }

    pub fn description(&self) -> String {
        self.first_nonempty(|i| &i.description)
    }

    pub fn to_dict(&self) -> BOMGroupDict {
        BOMGroupDict {
            value: self.value.clone(),
            footprint: self.footprint.clone(),
            qty: self.quantity(),
            refs: self.references(),
            description: self.description(),
            mpn: self.mpn(),
            lcsc: self.lcsc(),
            items: self.items.iter().map(BOMItem::to_dict).collect(),
        }
    }
}

/// Complete Bill of Materials.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct BOM {
    pub items: Vec<BOMItem>,
    /// Source schematic path.
    pub source: String,
}

impl BOM {
    pub fn new(items: Vec<BOMItem>) -> Self {
        BOM {
            items,
            source: String::new(),
        }
    }

    /// Real, placed (non-DNP) components.
    pub fn total_components(&self) -> usize {
        self.items
            .iter()
            .filter(|i| !i.is_virtual() && !i.dnp)
            .count()
    }

    pub fn unique_parts(&self) -> usize {
        self.grouped("value+footprint").len()
    }

    pub fn dnp_count(&self) -> usize {
        self.items.iter().filter(|i| i.dnp).count()
    }

    /// Group non-virtual items by `value+footprint` (default), `value`,
    /// `footprint`, or `mpn` (falling back to value+footprint).
    pub fn grouped(&self, by: &str) -> Vec<BOMGroup> {
        let mut groups: OrderedMap<BOMGroup> = OrderedMap::new();
        for item in self.items.iter().filter(|i| !i.is_virtual()) {
            let vf = || format!("{}|{}", item.value, item.footprint);
            let key = match by {
                "value" => item.value.clone(),
                "footprint" => item.footprint.clone(),
                "mpn" if !item.mpn.is_empty() => item.mpn.clone(),
                _ => vf(),
            };
            if !groups.contains_key(&key) {
                groups.insert(
                    key.clone(),
                    BOMGroup::new(item.value.clone(), item.footprint.clone(), Vec::new()),
                );
            }
            groups
                .get_mut(&key)
                .expect("inserted")
                .items
                .push(item.clone());
        }
        let mut out: Vec<BOMGroup> = groups.values().cloned().collect();
        let sort_key = |g: &BOMGroup| {
            let first = g
                .items
                .first()
                .and_then(|i| i.reference.chars().next())
                .map(String::from)
                .unwrap_or_default();
            (first, g.value.clone())
        };
        out.sort_by_key(sort_key);
        out
    }

    /// Filter out virtual items (and DNP unless `include_dnp`), optionally by
    /// an fnmatch-style reference pattern (e.g. `R*`).
    pub fn filter(&self, include_dnp: bool, reference_pattern: Option<&str>) -> BOM {
        let items = self
            .items
            .iter()
            .filter(|i| include_dnp || !i.dnp)
            .filter(|i| !i.is_virtual())
            .filter(|i| {
                reference_pattern
                    .filter(|p| !p.is_empty())
                    .is_none_or(|p| fnmatch(&i.reference, p))
            })
            .cloned()
            .collect();
        BOM {
            items,
            source: self.source.clone(),
        }
    }
}

/// Python `fnmatch.fnmatch` (POSIX, case-sensitive): `*`, `?`, `[seq]`, `[!seq]`.
pub fn fnmatch(name: &str, pattern: &str) -> bool {
    fn class(p: &[char], c: char) -> Option<(bool, usize)> {
        // p[0] == '['; returns (matched, length consumed) or None if unclosed.
        let mut i = 1;
        let negate = p.get(i) == Some(&'!');
        if negate {
            i += 1;
        }
        let start = i;
        let mut matched = false;
        while i < p.len() {
            if p[i] == ']' && i > start {
                return Some((matched != negate, i + 1));
            }
            if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
                if p[i] <= c && c <= p[i + 2] {
                    matched = true;
                }
                i += 3;
            } else {
                if p[i] == c {
                    matched = true;
                }
                i += 1;
            }
        }
        None
    }
    fn go(n: &[char], p: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|k| go(&n[k..], &p[1..])),
            Some('?') => !n.is_empty() && go(&n[1..], &p[1..]),
            Some('[') => match (n.first(), class(p, n.first().copied().unwrap_or('\0'))) {
                (Some(_), Some((true, len))) => go(&n[1..], &p[len..]),
                (Some(_), Some((false, _))) => false,
                (None, Some(_)) => false,
                (_, None) => n.first() == Some(&'[') && go(&n[1..], &p[1..]),
            },
            Some(&c) => n.first() == Some(&c) && go(&n[1..], &p[1..]),
        }
    }
    let n: Vec<char> = name.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    go(&n, &p)
}

/// BOM items from one schematic (power symbols skipped).
pub fn extract_bom_from_schematic(schematic: &Schematic) -> Vec<BOMItem> {
    let mut items = Vec::new();
    for sym in schematic.symbols() {
        if sym.lib_id.starts_with("power:") {
            continue;
        }
        let mut item = BOMItem::new(
            sym.reference(),
            sym.value(),
            sym.footprint(),
            sym.lib_id.clone(),
        );
        item.datasheet = sym.datasheet().into();
        item.dnp = sym.dnp;
        item.in_bom = sym.in_bom;
        item.on_board = sym.on_board;
        for (name, prop) in sym.properties.iter() {
            item.apply_property(name, &prop.value);
        }
        items.push(item);
    }
    items
}

/// BOM over every sheet of a hierarchy (unloadable sheets are skipped).
pub fn extract_bom_hierarchical(root_schematic: &str) -> BOM {
    let hierarchy = build_hierarchy(root_schematic);
    let mut items = Vec::new();
    for node in hierarchy.all_nodes() {
        if let Ok(sch) = Schematic::load(&node.path) {
            items.extend(extract_bom_from_schematic(&sch));
        }
    }
    BOM {
        items,
        source: root_schematic.into(),
    }
}

/// BOM from a schematic, including sub-sheets when `hierarchical`.
pub fn extract_bom(schematic_path: &str, hierarchical: bool) -> Result<BOM> {
    if hierarchical {
        return Ok(extract_bom_hierarchical(schematic_path));
    }
    let sch = Schematic::load(schematic_path)?;
    Ok(BOM {
        items: extract_bom_from_schematic(&sch),
        source: schematic_path.into(),
    })
}

// ------------------------------------------------------------- PCB side
//
// Upstream reads these through `schema.pcb.PCB`; until that port lands the
// few footprint fields needed are read straight from the tree.

struct PcbFootprint {
    name: String,
    reference: String,
    value: String,
    properties: OrderedMap<String>,
    exclude_from_bom: bool,
    dnp: bool,
}

fn pcb_footprints(pcb_path: &Path) -> Result<Vec<PcbFootprint>> {
    let root = sexp::parse_file(pcb_path)?;
    let text_of = |fp: &SExp, kind: &str| -> Option<String> {
        fp.children_named("fp_text")
            .find(|t| get_string(t, 0).as_deref() == Some(kind))
            .and_then(|t| get_string(t, 1))
    };
    Ok(root
        .children_named("footprint")
        .map(|fp| {
            let mut properties = OrderedMap::new();
            for prop in fp.children_named("property") {
                if let Some(key) = get_string(prop, 0) {
                    properties.insert(key, get_string(prop, 1).unwrap_or_default());
                }
            }
            let attr = fp.get("attr");
            let attr_flag = |flag: &str| attr.is_some_and(|a| a.flag(flag));
            PcbFootprint {
                name: get_string(fp, 0).unwrap_or_default(),
                reference: properties
                    .get("Reference")
                    .cloned()
                    .or_else(|| text_of(fp, "reference"))
                    .unwrap_or_default(),
                value: properties
                    .get("Value")
                    .cloned()
                    .or_else(|| text_of(fp, "value"))
                    .unwrap_or_default(),
                exclude_from_bom: attr_flag("exclude_from_bom"),
                dnp: attr_flag("dnp"),
                properties,
            }
        })
        .collect())
}

/// BOM from PCB footprints (skipping `exclude_from_bom`).
pub fn extract_bom_from_pcb(pcb_path: &str) -> Result<BOM> {
    let mut items = Vec::new();
    for fp in pcb_footprints(Path::new(pcb_path))? {
        if fp.exclude_from_bom {
            continue;
        }
        let mut item = BOMItem::new(fp.reference, fp.value, fp.name, "");
        item.dnp = fp.dnp;
        for (name, value) in fp.properties.iter() {
            item.apply_property(name, value);
        }
        items.push(item);
    }
    Ok(BOM {
        items,
        source: pcb_path.into(),
    })
}

/// Fill blank item footprints from the PCB's footprint names by reference;
/// returns how many were filled.
pub fn backfill_footprints_from_pcb(
    items: &mut [BOMItem],
    pcb_path: impl AsRef<Path>,
) -> Result<usize> {
    let mut by_ref: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for fp in pcb_footprints(pcb_path.as_ref())? {
        by_ref.insert(fp.reference, fp.name);
    }
    let mut filled = 0;
    for item in items.iter_mut() {
        if !item.footprint.trim().is_empty() {
            continue;
        }
        if let Some(name) = by_ref.get(&item.reference).filter(|n| !n.is_empty()) {
            item.footprint = name.clone();
            filled += 1;
        }
    }
    Ok(filled)
}
