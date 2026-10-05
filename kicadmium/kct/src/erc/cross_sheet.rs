//! Cross-sheet ERC helpers (port of `kicad_tools.erc.cross_sheet`).
//!
//! Complements KiCad's per-sheet ERC with hierarchy-wide checks: duplicate
//! references across sheets, and suppression of per-sheet false positives
//! for global labels, power drivers, and wire positions.
//!
//! The hierarchy walk mirrors `schema.hierarchy.build_hierarchy` +
//! `schema.schematic.Schematic` over raw s-expressions.
// TODO(foundation): switch to schema::hierarchy / schema::schematic once merged

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::violation::{ERCSeverity, ERCViolation, ERCViolationType};
use crate::pyjson::Json;
use crate::sexp::SExp;

/// One node of the sheet hierarchy.
#[derive(Debug, Clone)]
pub struct HierarchyNode {
    pub name: String,
    pub path: PathBuf,
    pub path_string: String,
    pub is_root: bool,
    pub hierarchical_labels: Vec<String>,
    pub children: Vec<HierarchyNode>,
}

impl HierarchyNode {
    /// Pre-order list of nodes (including self).
    pub fn all_nodes(&self) -> Vec<&HierarchyNode> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.all_nodes());
        }
        out
    }
}

fn child_path_string(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn text0(node: &SExp) -> Option<String> {
    node.text_at(0)
}

fn load_sexp(path: &Path) -> Option<SExp> {
    let text = std::fs::read_to_string(path).ok()?;
    crate::sexp::parse(&text).ok()
}

struct Builder {
    base: PathBuf,
    loaded: HashMap<PathBuf, Vec<String>>,
}

impl Builder {
    fn load(&mut self, path: &Path, name: &str, parent: Option<&str>) -> HierarchyNode {
        let mut full = path.to_path_buf();
        if !full.exists() && !full.is_absolute() {
            full = self.base.join(path);
        }
        let path_string = match parent {
            None => "/".to_string(),
            Some(p) => child_path_string(p, name),
        };
        let mut node = HierarchyNode {
            name: name.to_string(),
            path: full.clone(),
            path_string,
            is_root: parent.is_none(),
            hierarchical_labels: Vec::new(),
            children: Vec::new(),
        };
        if let Some(labels) = self.loaded.get(&full) {
            node.hierarchical_labels = labels.clone();
            return node;
        }
        let Some(root) = load_sexp(&full) else {
            return node;
        };
        node.hierarchical_labels = root
            .find_all("hierarchical_label")
            .filter_map(|l| l.string_at(0).filter(|s| !s.is_empty()).map(str::to_string))
            .collect();
        self.loaded
            .insert(full.clone(), node.hierarchical_labels.clone());
        let sheets: Vec<(String, String)> = root
            .find_all("sheet")
            .map(|s| {
                let mut name = String::new();
                let mut file = String::new();
                for p in s.find_all("property") {
                    let value = p.string_at(1).unwrap_or_default().to_string();
                    match p.string_at(0) {
                        Some("Sheetname") => name = value,
                        Some("Sheetfile") => file = value,
                        _ => {}
                    }
                }
                (name, file)
            })
            .collect();
        let own_path = node.path_string.clone();
        for (name, file) in sheets {
            let child = self.load(Path::new(&file), &name, Some(&own_path));
            node.children.push(child);
        }
        node
    }
}

/// Build the sheet hierarchy rooted at `root_schematic`.
pub fn build_hierarchy(root_schematic: &Path) -> HierarchyNode {
    let base = root_schematic
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut b = Builder {
        base,
        loaded: HashMap::new(),
    };
    b.load(root_schematic, "Root", None)
}

/// Minimal schematic view used by the cross-sheet checks.
struct Sheet {
    root: SExp,
}

struct Symbol {
    reference: String,
    value: String,
    lib_id: String,
    uuid: String,
}

impl Sheet {
    fn load(path: &Path) -> Option<Sheet> {
        let root = load_sexp(path)?;
        root.has_tag("kicad_sch").then_some(Sheet { root })
    }

    fn symbols(&self) -> Vec<Symbol> {
        self.root
            .children_named("symbol")
            .map(|s| {
                let mut props: HashMap<String, String> = HashMap::new();
                for p in s.find_all("property") {
                    let name = p.string_at(0).unwrap_or_default().to_string();
                    let value = p.string_at(1).unwrap_or_default().to_string();
                    props.insert(name, value);
                }
                Symbol {
                    reference: props.get("Reference").cloned().unwrap_or_default(),
                    value: props.get("Value").cloned().unwrap_or_default(),
                    lib_id: s.find("lib_id").and_then(text0).unwrap_or_default(),
                    uuid: s.find("uuid").and_then(text0).unwrap_or_default(),
                }
            })
            .collect()
    }

    fn labels_of(&self, tag: &str) -> Vec<(String, (f64, f64), String)> {
        self.root
            .find_all(tag)
            .map(|l| {
                let text = l.string_at(0).unwrap_or_default().to_string();
                let pos = l
                    .find("at")
                    .map(|a| (a.float_at(0).unwrap_or(0.0), a.float_at(1).unwrap_or(0.0)))
                    .unwrap_or((0.0, 0.0));
                let uuid = l.find("uuid").and_then(text0).unwrap_or_default();
                (text, pos, uuid)
            })
            .collect()
    }

    fn wires(&self) -> Vec<((f64, f64), (f64, f64))> {
        self.root
            .find_all("wire")
            .map(|w| {
                let xy: Vec<(f64, f64)> = w
                    .find("pts")
                    .map(|pts| {
                        pts.find_all("xy")
                            .map(|p| (p.float_at(0).unwrap_or(0.0), p.float_at(1).unwrap_or(0.0)))
                            .collect()
                    })
                    .unwrap_or_default();
                if xy.len() >= 2 {
                    (xy[0], xy[1])
                } else {
                    ((0.0, 0.0), (0.0, 0.0))
                }
            })
            .collect()
    }

    fn junctions(&self) -> Vec<(f64, f64)> {
        self.root
            .find_all("junction")
            .map(|j| {
                j.find("at")
                    .map(|a| (a.float_at(0).unwrap_or(0.0), a.float_at(1).unwrap_or(0.0)))
                    .unwrap_or((0.0, 0.0))
            })
            .collect()
    }

    fn lib_symbol(&self, lib_id: &str) -> Option<&SExp> {
        self.root
            .find("lib_symbols")?
            .find_all("symbol")
            .find(|s| s.string_at(0) == Some(lib_id))
    }
}

fn each_sheet(root_schematic: &Path, mut visit: impl FnMut(&HierarchyNode, &Sheet)) {
    let hierarchy = build_hierarchy(root_schematic);
    for node in hierarchy.all_nodes() {
        if let Some(sch) = Sheet::load(&node.path) {
            visit(node, &sch);
        }
    }
}

static REF_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z]+)(\d+)$").expect("regex"));

/// Duplicate reference designators across hierarchical sheets (power and
/// `#` symbols excluded; multi-unit instances on one sheet are not dups).
pub fn check_cross_sheet_duplicates(root_schematic: &Path) -> Vec<ERCViolation> {
    struct Entry {
        value: String,
        sheet_path: String,
        lib_id: String,
    }
    let mut by_ref: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    each_sheet(root_schematic, |node, sch| {
        for sym in sch.symbols() {
            if sym.reference.is_empty()
                || sym.reference.starts_with('#')
                || sym.lib_id.starts_with("power:")
            {
                continue;
            }
            by_ref
                .entry(sym.reference.clone())
                .or_default()
                .push(Entry {
                    value: sym.value,
                    sheet_path: node.path_string.clone(),
                    lib_id: sym.lib_id,
                });
        }
    });

    let mut used: HashMap<String, BTreeSet<u64>> = HashMap::new();
    for r in by_ref.keys() {
        if let Some(c) = REF_NUM.captures(r) {
            if let Ok(n) = c[2].parse::<u64>() {
                used.entry(c[1].to_string()).or_default().insert(n);
            }
        }
    }

    let mut out = Vec::new();
    for (reference, entries) in &by_ref {
        if entries.len() < 2 {
            continue;
        }
        let mut unique: Vec<&Entry> = Vec::new();
        for e in entries {
            if !unique
                .iter()
                .any(|u| u.sheet_path == e.sheet_path && u.lib_id == e.lib_id)
            {
                unique.push(e);
            }
        }
        if unique.len() < 2 {
            continue;
        }
        let details: Vec<String> = unique
            .iter()
            .map(|e| format!("{} (value={})", e.sheet_path, e.value))
            .collect();
        let suggestion = REF_NUM.captures(reference).map(|c| {
            let prefix = &c[1];
            let empty = BTreeSet::new();
            let nums = used.get(prefix).unwrap_or(&empty);
            let max = nums.iter().next_back().copied().unwrap_or(0);
            let next = (1..=max + 1).find(|n| !nums.contains(n)).unwrap_or(max + 1);
            format!("Consider renaming the duplicate to {prefix}{next}")
        });
        let mut v = ERCViolation::new(
            ERCViolationType::DUPLICATE_REFERENCE,
            ERCViolationType::DUPLICATE_REFERENCE.value(),
            ERCSeverity::Error,
            format!(
                "Reference '{reference}' is used on multiple sheets: {}",
                details.join("; ")
            ),
        );
        v.sheet = "/".into();
        v.suggestions = suggestion.into_iter().collect();
        out.push(v);
    }
    out
}

/// Global label name -> sheet paths it appears on.
pub fn build_global_label_inventory(root_schematic: &Path) -> HashMap<String, BTreeSet<String>> {
    let mut inv: HashMap<String, BTreeSet<String>> = HashMap::new();
    each_sheet(root_schematic, |node, sch| {
        for (text, _, _) in sch.labels_of("global_label") {
            inv.entry(text)
                .or_default()
                .insert(node.path_string.clone());
        }
    });
    inv
}

/// Sheet paths containing at least one local, global, or hierarchical label.
pub fn build_sheet_label_presence(root_schematic: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    each_sheet(root_schematic, |node, sch| {
        if ["label", "global_label", "hierarchical_label"]
            .iter()
            .any(|t| sch.root.find(t).is_some())
        {
            out.insert(node.path_string.clone());
        }
    });
    out
}

static LABEL_QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[Ll]abel\s+'([^']+)'").expect("regex"));
static DOUBLE_QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([^"]+)""#).expect("regex"));

/// Label name from an ERC description, falling back to item descriptions
/// (KiCad 10+ puts the name in the items).
pub fn extract_label_name(description: &str, items: &[String]) -> Option<String> {
    for text in std::iter::once(description).chain(items.iter().map(String::as_str)) {
        if let Some(c) = LABEL_QUOTED.captures(text) {
            return Some(c[1].to_string());
        }
        if let Some(c) = DOUBLE_QUOTED.captures(text) {
            return Some(c[1].to_string());
        }
    }
    None
}

const LABEL_TARGETS: [&str; 2] = ["single_global_label", "isolated_pin_label"];

fn jstr<'a>(v: &'a Json, key: &str) -> &'a str {
    v.get(key).and_then(Json::as_str).unwrap_or("")
}

fn item_descs(v: &Json) -> Vec<String> {
    v.get("items")
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .map(|i| jstr(i, "description").to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn keep_label_violation(
    label: Option<String>,
    sheet: &str,
    inventory: &HashMap<String, BTreeSet<String>>,
    presence: &HashSet<String>,
) -> bool {
    match label {
        None => sheet.is_empty() || presence.contains(sheet),
        Some(name) => inventory.get(&name).map_or(0, BTreeSet::len) < 2,
    }
}

/// Drop false-positive `single_global_label` / `isolated_pin_label` raw
/// violation dicts (labels spanning >= 2 sheets, or label-free sheets).
/// Each dict may carry `_sheet_path`.
pub fn filter_cross_sheet_global_labels(violations: Vec<Json>, root_schematic: &Path) -> Vec<Json> {
    if !violations
        .iter()
        .any(|v| LABEL_TARGETS.contains(&jstr(v, "type")))
    {
        return violations;
    }
    let inv = build_global_label_inventory(root_schematic);
    let presence = build_sheet_label_presence(root_schematic);
    violations
        .into_iter()
        .filter(|v| {
            !LABEL_TARGETS.contains(&jstr(v, "type"))
                || keep_label_violation(
                    extract_label_name(jstr(v, "description"), &item_descs(v)),
                    jstr(v, "_sheet_path"),
                    &inv,
                    &presence,
                )
        })
        .collect()
}

/// [`ERCViolation`] variant of [`filter_cross_sheet_global_labels`].
pub fn filter_cross_sheet_global_labels_objs(
    violations: Vec<ERCViolation>,
    root_schematic: &Path,
) -> Vec<ERCViolation> {
    if !violations
        .iter()
        .any(|v| LABEL_TARGETS.contains(&v.type_str.as_str()))
    {
        return violations;
    }
    let inv = build_global_label_inventory(root_schematic);
    let presence = build_sheet_label_presence(root_schematic);
    violations
        .into_iter()
        .filter(|v| {
            !LABEL_TARGETS.contains(&v.type_str.as_str())
                || keep_label_violation(
                    extract_label_name(&v.description, &v.items),
                    &v.sheet,
                    &inv,
                    &presence,
                )
        })
        .collect()
}

/// Net names with a `power_out` driver anywhere in the hierarchy.
pub fn build_power_driver_inventory(root_schematic: &Path) -> HashSet<String> {
    let mut driven = HashSet::new();
    each_sheet(root_schematic, |_, sch| {
        for sym in sch.symbols() {
            let Some(lib) = sch.lib_symbol(&sym.lib_id) else {
                continue;
            };
            let mut names = Vec::new();
            for sub in lib.find_all("symbol") {
                for pin in sub.find_all("pin") {
                    if pin.string_at(0).unwrap_or("passive") == "power_out" {
                        names.push(pin.find("name").and_then(text0).unwrap_or_default());
                    }
                }
            }
            if names.is_empty() {
                continue;
            }
            if sym.lib_id.starts_with("power:") {
                if !sym.value.is_empty() {
                    driven.insert(sym.value.clone());
                }
            } else {
                driven.extend(names.into_iter().filter(|n| !n.is_empty()));
            }
        }
    });
    driven
}

static PIN_POWER_IN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Pin\s+(\S+)\s+\(power_in\)").expect("regex"));
static PIN_OF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Pin\s+(\S+)\s+of\s+").expect("regex"));
static PIN_ANY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Pin\s+(\S+)").expect("regex"));

/// Pin/net name from a `power_pin_not_driven` violation.
pub fn extract_power_net_name(description: &str, items: &[String]) -> Option<String> {
    for desc in items {
        if let Some(c) = PIN_POWER_IN.captures(desc) {
            return Some(c[1].to_string());
        }
        if let Some(c) = PIN_OF.captures(desc) {
            return Some(c[1].to_string());
        }
    }
    PIN_ANY.captures(description).map(|c| c[1].to_string())
}

/// Drop `power_pin_not_driven` dicts whose net is driven on another sheet.
pub fn filter_cross_sheet_power_violations(
    violations: Vec<Json>,
    root_schematic: &Path,
) -> Vec<Json> {
    const TARGET: &str = "power_pin_not_driven";
    if !violations.iter().any(|v| jstr(v, "type") == TARGET) {
        return violations;
    }
    let driven = build_power_driver_inventory(root_schematic);
    violations
        .into_iter()
        .filter(|v| {
            jstr(v, "type") != TARGET
                || !extract_power_net_name(jstr(v, "description"), &item_descs(v))
                    .is_some_and(|n| driven.contains(&n))
        })
        .collect()
}

const WIRE_POSITION_TYPES: [&str; 5] = [
    "wire_dangling",
    "endpoint_off_grid",
    "no_connect_dangling",
    "label_dangling",
    "global_label_dangling",
];

const TOLERANCE: f64 = 0.1;

/// `round(v / 0.1)` bucket (banker's rounding, like Python `round`).
fn snap(v: f64) -> i64 {
    (v / TOLERANCE).round_ties_even() as i64
}

fn pos_xy(v: &Json) -> Option<(f64, f64)> {
    let pos = v.get("pos")?;
    let x = pos.get("x").filter(|j| !j.is_null())?;
    let y = pos.get("y").filter(|j| !j.is_null())?;
    Some((x.as_f64()?, y.as_f64()?))
}

fn wire_points(sch: &Sheet) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for ((sx, sy), (ex, ey)) in sch.wires() {
        out.push((sx, sy));
        out.push((ex, ey));
        out.push(((sx + ex) / 2.0, (sy + ey) / 2.0));
    }
    out
}

fn enrich_description_with_pos(v: &mut Json) {
    let Some((x, y)) = pos_xy(v) else {
        return;
    };
    let coord = format!("at ({x:.1}, {y:.1})");
    let desc = jstr(v, "description").to_string();
    if !desc.contains(&coord) {
        v.set("description", format!("{desc} {coord}"));
    }
}

/// Move wire-position violations KiCad attributed to `/` onto the child
/// sheet owning that wire point, and append `at (x, y)` to descriptions.
pub fn reattribute_wire_dangling_violations(
    mut violations: Vec<Json>,
    root_schematic: &Path,
) -> Vec<Json> {
    let is_target = |v: &Json| WIRE_POSITION_TYPES.contains(&jstr(v, "type"));
    let needs = violations
        .iter()
        .any(|v| is_target(v) && jstr(v, "_sheet_path") == "/");
    if !needs {
        for v in violations.iter_mut().filter(|v| is_target(v)) {
            enrich_description_with_pos(v);
        }
        return violations;
    }
    let mut map: HashMap<(i64, i64), String> = HashMap::new();
    each_sheet(root_schematic, |node, sch| {
        if node.is_root {
            return;
        }
        for (x, y) in wire_points(sch) {
            map.insert((snap(x), snap(y)), node.path_string.clone());
        }
    });
    for v in violations.iter_mut() {
        if !is_target(v) {
            continue;
        }
        enrich_description_with_pos(v);
        if jstr(v, "_sheet_path") != "/" {
            continue;
        }
        if let Some((x, y)) = pos_xy(v) {
            if let Some(sheet) = map.get(&(snap(x), snap(y))) {
                v.set("_sheet_path", sheet.clone());
            }
        }
    }
    violations
}

/// Drop phantom `wire_dangling` dicts whose position matches no wire point,
/// junction, or label anywhere in the hierarchy.
pub fn filter_phantom_wire_violations(violations: Vec<Json>, root_schematic: &Path) -> Vec<Json> {
    const TARGET: &str = "wire_dangling";
    if !violations.iter().any(|v| jstr(v, "type") == TARGET) {
        return violations;
    }
    let mut known: HashSet<(i64, i64)> = HashSet::new();
    each_sheet(root_schematic, |_, sch| {
        let mut pts = wire_points(sch);
        pts.extend(sch.junctions());
        for tag in ["label", "global_label", "hierarchical_label"] {
            pts.extend(sch.labels_of(tag).into_iter().map(|(_, p, _)| p));
        }
        known.extend(pts.into_iter().map(|(x, y)| (snap(x), snap(y))));
    });
    violations
        .into_iter()
        .filter(|v| {
            jstr(v, "type") != TARGET
                || pos_xy(v).is_none_or(|(x, y)| known.contains(&(snap(x), snap(y))))
        })
        .collect()
}

const SYMBOL_VIOLATION_TYPES: [&str; 8] = [
    "pin_not_connected",
    "pin_not_driven",
    "power_pin_not_driven",
    "different_unit_value",
    "different_unit_footprint",
    "unresolved_variable",
    "extra_units",
    "missing_units",
];

const LABEL_VIOLATION_TYPES: [&str; 4] = [
    "global_label_dangling",
    "label_dangling",
    "single_global_label",
    "isolated_pin_label",
];

static OF_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bof\s+([A-Za-z]+\d+)\b").expect("regex"));
static SYMBOL_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bSymbol\s+([A-Za-z]+\d+)\b").expect("regex"));

/// UUIDs and refs (`"Pin VCC of U3"`, `"Symbol U3"`) from ERC item dicts.
pub fn extract_identifiers_from_items(items: &[Json]) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        let uuid = jstr(item, "uuid");
        if !uuid.is_empty() {
            out.push(uuid.to_string());
        }
        let desc = jstr(item, "description");
        if !desc.is_empty() {
            if let Some(c) = OF_REF.captures(desc) {
                out.push(c[1].to_string());
            }
            if let Some(c) = SYMBOL_REF.captures(desc) {
                out.push(c[1].to_string());
            }
        }
    }
    out
}

/// Move symbol/label violations KiCad attributed to `/` onto the child sheet
/// that owns the referenced symbol or label.
pub fn reattribute_symbol_violations(
    mut violations: Vec<Json>,
    root_schematic: &Path,
) -> Vec<Json> {
    let is_target = |v: &Json| {
        let t = jstr(v, "type");
        SYMBOL_VIOLATION_TYPES.contains(&t) || LABEL_VIOLATION_TYPES.contains(&t)
    };
    if !violations
        .iter()
        .any(|v| is_target(v) && jstr(v, "_sheet_path") == "/")
    {
        return violations;
    }
    let mut map: HashMap<String, String> = HashMap::new();
    each_sheet(root_schematic, |node, sch| {
        for sym in sch.symbols() {
            if !sym.uuid.is_empty() {
                map.insert(sym.uuid.clone(), node.path_string.clone());
            }
            if !sym.reference.is_empty() && !map.contains_key(&sym.reference) {
                map.insert(sym.reference.clone(), node.path_string.clone());
            }
        }
        for tag in ["global_label", "label"] {
            for (_, _, uuid) in sch.labels_of(tag) {
                if !uuid.is_empty() {
                    map.insert(uuid, node.path_string.clone());
                }
            }
        }
    });
    for v in violations.iter_mut() {
        if !is_target(v) || jstr(v, "_sheet_path") != "/" {
            continue;
        }
        let Some(items) = v
            .get("items")
            .and_then(Json::as_array)
            .filter(|i| !i.is_empty())
        else {
            continue;
        };
        let ids = extract_identifiers_from_items(items);
        if let Some(sheet) = ids
            .iter()
            .filter_map(|id| map.get(id))
            .find(|s| !s.is_empty() && s.as_str() != "/")
            .cloned()
        {
            v.set("_sheet_path", sheet);
        }
    }
    violations
}

/// All hierarchical label names in the hierarchy (deduplicated, sorted for
/// determinism; upstream returns `list(set(...))`).
pub fn gather_hierarchical_labels(root_schematic: &Path) -> Vec<String> {
    let h = build_hierarchy(root_schematic);
    let set: BTreeSet<String> = h
        .all_nodes()
        .iter()
        .flat_map(|n| n.hierarchical_labels.iter().cloned())
        .collect();
    set.into_iter().collect()
}
