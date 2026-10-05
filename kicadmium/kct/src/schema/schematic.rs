//! Schematic document model (port of `kicad_tools.schema.schematic`).
//!
//! [`Schematic`] wraps the lossless `SExp` tree of a `.kicad_sch`; typed views
//! (symbols, wires, labels, sheets, ...) are parsed lazily and cached, and the
//! editing API mutates the tree directly so untouched text saves unchanged.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::sexp::{Document, SExp, Value};

use super::label::{GlobalLabel, HierarchicalLabel, Label};
use super::library::{py_round, resolve_extends, LibrarySymbol};
use super::symbol::{
    find_mut, find_string, for_each_named_mut, get_float, get_int, get_string, new_uuid,
    OrderedMap, SymbolInstance, SymbolPin, SymbolProperty,
};
use super::wire::{Junction, NoConnect, Wire};

/// Schematic title block information.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct TitleBlock {
    pub title: String,
    pub date: String,
    pub rev: String,
    pub company: String,
    pub comments: BTreeMap<i64, String>,
}

impl TitleBlock {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut tb = TitleBlock {
            title: find_string(sexp, "title"),
            date: find_string(sexp, "date"),
            rev: find_string(sexp, "rev"),
            company: find_string(sexp, "company"),
            comments: BTreeMap::new(),
        };
        for child in sexp.children_named("comment") {
            if let (Some(num), Some(text)) = (get_int(child, 0), get_string(child, 1)) {
                tb.comments.insert(num, text);
            }
        }
        tb
    }
}

/// Reference to a hierarchical sheet (schematic-level view).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SheetInstance {
    pub name: String,
    pub filename: String,
    pub uuid: String,
    pub position: (f64, f64),
    pub size: (f64, f64),
}

impl SheetInstance {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut name = String::new();
        let mut filename = String::new();
        for prop in sexp.find_all("property") {
            match get_string(prop, 0).as_deref() {
                Some("Sheetname") => name = get_string(prop, 1).unwrap_or_default(),
                Some("Sheetfile") => filename = get_string(prop, 1).unwrap_or_default(),
                _ => {}
            }
        }
        let position = sexp
            .find("at")
            .map(|at| {
                (
                    get_float(at, 0).unwrap_or(0.0),
                    get_float(at, 1).unwrap_or(0.0),
                )
            })
            .unwrap_or((0.0, 0.0));
        let size = sexp
            .find("size")
            .map(|sz| {
                (
                    get_float(sz, 0).filter(|&v| v != 0.0).unwrap_or(50.0),
                    get_float(sz, 1).filter(|&v| v != 0.0).unwrap_or(25.0),
                )
            })
            .unwrap_or((50.0, 25.0));
        SheetInstance {
            name,
            filename,
            uuid: find_string(sexp, "uuid"),
            position,
            size,
        }
    }
}

/// Optional arguments of [`Schematic::add_symbol`] (upstream keyword args).
#[derive(Debug, Clone)]
pub struct AddSymbolOptions {
    pub rotation: f64,
    pub mirror: String,
    pub unit: i64,
    /// `None`: detect from the embedded `lib_symbols` entry.
    pub pin_numbers: Option<Vec<String>>,
    pub datasheet: String,
    pub project_name: String,
    pub instance_path: String,
}

impl Default for AddSymbolOptions {
    fn default() -> Self {
        AddSymbolOptions {
            rotation: 0.0,
            mirror: String::new(),
            unit: 1,
            pin_numbers: None,
            datasheet: String::new(),
            project_name: String::new(),
            instance_path: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Cache {
    symbols: OnceCell<Vec<SymbolInstance>>,
    wires: OnceCell<Vec<Wire>>,
    junctions: OnceCell<Vec<Junction>>,
    no_connects: OnceCell<Vec<NoConnect>>,
    labels: OnceCell<Vec<Label>>,
    hierarchical_labels: OnceCell<Vec<HierarchicalLabel>>,
    sheets: OnceCell<Vec<SheetInstance>>,
}

/// High-level interface to a KiCad schematic.
#[derive(Debug, Clone)]
pub struct Schematic {
    doc: Document,
    cache: Cache,
}

const LIB_SYMBOLS_ANCHORS: &[&str] = &[
    "uuid",
    "paper",
    "title_block",
    "generator",
    "generator_version",
    "version",
];

impl Schematic {
    /// Wrap a parsed tree; errors unless its tag is `kicad_sch`.
    pub fn new(sexp: SExp, path: Option<PathBuf>) -> Result<Self> {
        if !sexp.has_tag("kicad_sch") {
            bail!("Not a schematic: {}", sexp.tag().unwrap_or("None"));
        }
        Ok(Schematic {
            doc: Document { root: sexp, path },
            cache: Cache::default(),
        })
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            bail!("Schematic file not found: {}", path.display());
        }
        let doc = Document::load(path)?;
        if !doc.root.has_tag("kicad_sch") {
            bail!(
                "Not a KiCad schematic: {} (got {})",
                path.display(),
                doc.root.tag().unwrap_or("None")
            );
        }
        Schematic::new(doc.root, Some(path.to_path_buf()))
    }

    /// Save to `path` or the original path (atomic write).
    pub fn save(&self, path: Option<&Path>) -> Result<()> {
        if path.is_none() && self.doc.path.is_none() {
            bail!("No path specified and no original path available");
        }
        self.doc.save(path)
    }

    pub fn sexp(&self) -> &SExp {
        &self.doc.root
    }

    /// Mutable tree access; clears the parsed caches.
    pub fn sexp_mut(&mut self) -> &mut SExp {
        self.invalidate_cache();
        &mut self.doc.root
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn path(&self) -> Option<&Path> {
        self.doc.path.as_deref()
    }

    pub fn version(&self) -> Option<i64> {
        self.sexp().find("version").and_then(|v| get_int(v, 0))
    }

    pub fn generator(&self) -> Option<String> {
        self.sexp().find("generator").and_then(|g| get_string(g, 0))
    }

    pub fn title_block(&self) -> TitleBlock {
        self.sexp()
            .find("title_block")
            .map(TitleBlock::from_sexp)
            .unwrap_or_default()
    }

    pub fn paper(&self) -> Option<String> {
        self.sexp().find("paper").and_then(|p| get_string(p, 0))
    }

    pub fn uuid(&self) -> Option<String> {
        self.sexp().find("uuid").and_then(|u| get_string(u, 0))
    }

    // ------------------------------------------------------------- symbols

    /// Placed symbol instances (direct `symbol` children, not lib_symbols).
    pub fn symbols(&self) -> &[SymbolInstance] {
        self.cache.symbols.get_or_init(|| {
            self.sexp()
                .children_named("symbol")
                .map(SymbolInstance::from_sexp)
                .collect()
        })
    }

    pub fn get_symbol(&self, reference: &str) -> Option<&SymbolInstance> {
        self.symbols().iter().find(|s| s.reference() == reference)
    }

    pub fn find_symbols_by_lib(&self, lib_id: &str) -> Vec<&SymbolInstance> {
        self.symbols()
            .iter()
            .filter(|s| s.lib_id == lib_id)
            .collect()
    }

    pub fn iter_symbols(&self) -> std::slice::Iter<'_, SymbolInstance> {
        self.symbols().iter()
    }

    /// The `(symbol ...)` node of the placed instance with this UUID.
    pub fn symbol_sexp(&self, uuid: &str) -> Option<&SExp> {
        self.sexp()
            .children_named("symbol")
            .find(|s| find_string(s, "uuid") == uuid)
    }

    /// Mutable `(symbol ...)` node by UUID; clears the parsed caches.
    pub fn symbol_sexp_mut(&mut self, uuid: &str) -> Option<&mut SExp> {
        self.invalidate_cache();
        self.doc
            .root
            .children
            .iter_mut()
            .find(|s| s.has_tag("symbol") && find_string(s, "uuid") == uuid)
    }

    /// Set a property value on the symbol with this reference (kct helper).
    /// Returns whether the property existed.
    pub fn set_symbol_property(&mut self, reference: &str, name: &str, value: &str) -> bool {
        let Some(uuid) = self.get_symbol(reference).map(|s| s.uuid.clone()) else {
            return false;
        };
        self.symbol_sexp_mut(&uuid)
            .is_some_and(|s| s.set_property(name, value))
    }

    // ----------------------------------------------------- wires and labels

    pub fn wires(&self) -> &[Wire] {
        self.cache
            .wires
            .get_or_init(|| self.sexp().find_all("wire").map(Wire::from_sexp).collect())
    }

    pub fn junctions(&self) -> &[Junction] {
        self.cache.junctions.get_or_init(|| {
            self.sexp()
                .find_all("junction")
                .map(Junction::from_sexp)
                .collect()
        })
    }

    pub fn no_connects(&self) -> &[NoConnect] {
        self.cache.no_connects.get_or_init(|| {
            self.sexp()
                .find_all("no_connect")
                .map(NoConnect::from_sexp)
                .collect()
        })
    }

    pub fn labels(&self) -> &[Label] {
        self.cache.labels.get_or_init(|| {
            self.sexp()
                .find_all("label")
                .map(Label::from_sexp)
                .collect()
        })
    }

    pub fn hierarchical_labels(&self) -> &[HierarchicalLabel] {
        self.cache.hierarchical_labels.get_or_init(|| {
            self.sexp()
                .find_all("hierarchical_label")
                .map(HierarchicalLabel::from_sexp)
                .collect()
        })
    }

    pub fn global_labels(&self) -> Vec<GlobalLabel> {
        self.sexp()
            .find_all("global_label")
            .map(GlobalLabel::from_sexp)
            .collect()
    }

    pub fn sheets(&self) -> &[SheetInstance] {
        self.cache.sheets.get_or_init(|| {
            self.sexp()
                .find_all("sheet")
                .map(SheetInstance::from_sexp)
                .collect()
        })
    }

    pub fn is_hierarchical(&self) -> bool {
        !self.sheets().is_empty()
    }

    // ------------------------------------------------------ library symbols

    pub fn lib_symbols(&self) -> Option<&SExp> {
        self.sexp().find("lib_symbols")
    }

    /// Embedded library symbol definition named `lib_id`.
    pub fn get_lib_symbol(&self, lib_id: &str) -> Option<&SExp> {
        self.lib_symbols()?
            .find_all("symbol")
            .find(|s| get_string(s, 0).as_deref() == Some(lib_id))
    }

    /// Embedded library symbol with `extends` chains resolved.
    pub fn get_lib_symbol_resolved(&self, lib_id: &str) -> Result<Option<LibrarySymbol>> {
        let Some(sexp) = self.get_lib_symbol(lib_id) else {
            return Ok(None);
        };
        let sym = LibrarySymbol::from_sexp(sexp);
        if sym.extends.is_some() && sym.pins.is_empty() {
            let mut all: OrderedMap<LibrarySymbol> = OrderedMap::new();
            if let Some(lib_syms) = self.lib_symbols() {
                for s in lib_syms.find_all("symbol") {
                    let parsed = LibrarySymbol::from_sexp(s);
                    all.insert(parsed.name.clone(), parsed);
                }
            }
            resolve_extends(&mut all)?;
            if let Some(resolved) = all.remove(&sym.name) {
                return Ok(Some(resolved));
            }
        }
        Ok(Some(sym))
    }

    // ---------------------------------------------------------- editing API

    /// Index before `sheet_instances`/`symbol_instances`, else the end.
    fn find_insertion_index(&self) -> usize {
        self.sexp()
            .children
            .iter()
            .position(|c| c.has_tag("sheet_instances") || c.has_tag("symbol_instances"))
            .unwrap_or(self.sexp().children.len())
    }

    fn insert_element(&mut self, node: SExp) {
        let idx = self.find_insertion_index();
        self.doc.root.insert(idx, node);
        self.invalidate_cache();
    }

    fn pin_numbers_of(&self, lib_id: &str) -> Result<Option<Vec<String>>> {
        Ok(self
            .get_lib_symbol_resolved(lib_id)?
            .map(|s| s.pins.iter().map(|p| p.number.clone()).collect()))
    }

    /// Add a component symbol; its library definition must already be in
    /// `lib_symbols` unless `opts.pin_numbers` is given.
    pub fn add_symbol(
        &mut self,
        lib_id: &str,
        reference: &str,
        value: &str,
        footprint: &str,
        position: (f64, f64),
        opts: AddSymbolOptions,
    ) -> Result<SymbolInstance> {
        let mut props = OrderedMap::new();
        let below = (position.0, position.1 + 2.54);
        props.insert(
            "Reference",
            SymbolProperty::new("Reference", reference, position, 0.0, true),
        );
        props.insert(
            "Value",
            SymbolProperty::new("Value", value, below, 0.0, true),
        );
        props.insert(
            "Footprint",
            SymbolProperty::new("Footprint", footprint, position, 0.0, false),
        );
        props.insert(
            "Datasheet",
            SymbolProperty::new("Datasheet", opts.datasheet.clone(), position, 0.0, false),
        );

        let pin_numbers = match opts.pin_numbers {
            Some(p) => p,
            None => match self.pin_numbers_of(lib_id)? {
                Some(p) => p,
                None => bail!(
                    "lib_id '{lib_id}' not found in schematic lib_symbols. \
                     Use embed_lib_symbol() first, or supply pin_numbers explicitly."
                ),
            },
        };

        let instance = SymbolInstance {
            lib_id: lib_id.into(),
            uuid: new_uuid(),
            position,
            rotation: opts.rotation,
            mirror: opts.mirror,
            unit: opts.unit,
            in_bom: true,
            on_board: true,
            dnp: false,
            properties: props,
            pins: pin_numbers
                .into_iter()
                .map(|n| SymbolPin::new(n, new_uuid()))
                .collect(),
            project_name: opts.project_name,
            instance_path: opts.instance_path,
        };
        self.insert_element(instance.to_sexp());
        Ok(instance)
    }

    /// Add a power symbol (`power:<name>`, not in BOM or on board). The
    /// reference is `#FLG` for `PWR_FLAG`, else `#PWR`, numbered after the
    /// highest existing one.
    pub fn add_power(
        &mut self,
        name: &str,
        position: (f64, f64),
        rotation: f64,
        project_name: &str,
        instance_path: &str,
    ) -> Result<SymbolInstance> {
        let lib_id = format!("power:{name}");
        let pin_numbers = self
            .pin_numbers_of(&lib_id)?
            .unwrap_or_else(|| vec!["1".into()]);
        let prefix = if name == "PWR_FLAG" { "#FLG" } else { "#PWR" };
        let max_num = self
            .symbols()
            .iter()
            .filter_map(|s| {
                let digits = s.reference().strip_prefix(prefix)?;
                (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
                    .then(|| digits.parse::<u64>().ok())
                    .flatten()
            })
            .max()
            .unwrap_or(0);
        let ref_value = format!("{prefix}{:02}", max_num + 1);

        let mut props = OrderedMap::new();
        props.insert(
            "Reference",
            SymbolProperty::new("Reference", ref_value, position, 0.0, false),
        );
        props.insert(
            "Value",
            SymbolProperty::new("Value", name, (position.0, position.1 + 2.54), 0.0, true),
        );
        props.insert(
            "Footprint",
            SymbolProperty::new("Footprint", "", position, 0.0, false),
        );
        props.insert(
            "Datasheet",
            SymbolProperty::new("Datasheet", "", position, 0.0, false),
        );

        let instance = SymbolInstance {
            lib_id,
            uuid: new_uuid(),
            position,
            rotation,
            mirror: String::new(),
            unit: 1,
            in_bom: false,
            on_board: false,
            dnp: false,
            properties: props,
            pins: pin_numbers
                .into_iter()
                .map(|n| SymbolPin::new(n, new_uuid()))
                .collect(),
            project_name: project_name.into(),
            instance_path: instance_path.into(),
        };
        self.insert_element(instance.to_sexp());
        Ok(instance)
    }

    pub fn add_wire(&mut self, start: (f64, f64), end: (f64, f64)) -> Wire {
        let mut wire = Wire::new(start, end);
        wire.uuid = new_uuid();
        self.insert_element(wire.to_sexp());
        wire
    }

    pub fn add_label(&mut self, text: &str, position: (f64, f64), rotation: f64) -> Label {
        let label = Label {
            text: text.into(),
            position,
            rotation,
            uuid: new_uuid(),
        };
        self.insert_element(label.to_sexp());
        label
    }

    pub fn add_global_label(
        &mut self,
        text: &str,
        position: (f64, f64),
        rotation: f64,
        shape: &str,
    ) -> GlobalLabel {
        let label = GlobalLabel {
            text: text.into(),
            position,
            rotation,
            shape: shape.into(),
            uuid: new_uuid(),
        };
        self.insert_element(label.to_sexp());
        label
    }

    pub fn add_hierarchical_label(
        &mut self,
        text: &str,
        position: (f64, f64),
        rotation: f64,
        shape: &str,
    ) -> HierarchicalLabel {
        let label = HierarchicalLabel {
            text: text.into(),
            position,
            rotation,
            shape: shape.into(),
            uuid: new_uuid(),
        };
        self.insert_element(label.to_sexp());
        label
    }

    pub fn add_junction(&mut self, position: (f64, f64)) -> Junction {
        let mut junc = Junction::new(position);
        junc.uuid = new_uuid();
        self.insert_element(junc.to_sexp());
        junc
    }

    /// The `lib_symbols` section, created after the header if missing.
    fn ensure_lib_symbols(&mut self) -> &mut SExp {
        if self.lib_symbols().is_none() {
            let insert_idx = self
                .sexp()
                .children
                .iter()
                .rposition(|c| c.tag().is_some_and(|t| LIB_SYMBOLS_ANCHORS.contains(&t)))
                .map_or(0, |i| i + 1);
            self.doc
                .root
                .insert(insert_idx, SExp::list("lib_symbols", []));
        }
        self.invalidate_cache();
        find_mut(&mut self.doc.root, "lib_symbols").expect("lib_symbols present")
    }

    /// Insert a library symbol into `lib_symbols` (no-op if present).
    pub fn embed_lib_symbol(&mut self, lib_sym: &LibrarySymbol) {
        let lib_syms = self.ensure_lib_symbols();
        let exists = lib_syms
            .find_all("symbol")
            .any(|s| get_string(s, 0).as_deref() == Some(lib_sym.name.as_str()));
        if !exists {
            lib_syms.push(lib_sym.to_sexp_node());
        }
    }

    /// Replace `old_name` in `lib_symbols` with `lib_sym` (appending it).
    /// Returns whether an existing entry was replaced.
    pub fn replace_lib_symbol(&mut self, old_name: &str, lib_sym: &LibrarySymbol) -> bool {
        let lib_syms = self.ensure_lib_symbols();
        let named =
            |s: &SExp, n: &str| s.has_tag("symbol") && get_string(s, 0).as_deref() == Some(n);
        let mut replaced = false;
        if let Some(i) = lib_syms.children.iter().position(|s| named(s, old_name)) {
            lib_syms.children.remove(i);
            replaced = true;
        }
        if old_name != lib_sym.name {
            if let Some(i) = lib_syms
                .children
                .iter()
                .position(|s| named(s, &lib_sym.name))
            {
                lib_syms.children.remove(i);
            }
        }
        lib_syms.push(lib_sym.to_sexp_node());
        replaced
    }

    /// Nearest wire endpoint, junction, or placed pin within `radius` mm
    /// (upstream default 2.0).
    pub fn find_nearest_connection_point(
        &self,
        position: (f64, f64),
        radius: f64,
    ) -> Result<Option<(f64, f64)>> {
        let (px, py) = position;
        let mut best = None;
        let mut best_dist = radius;
        let mut consider = |pt: (f64, f64)| {
            let d = ((pt.0 - px).powi(2) + (pt.1 - py).powi(2)).sqrt();
            if d < best_dist {
                best_dist = d;
                best = Some(pt);
            }
        };
        for wire in self.wires() {
            consider(wire.start);
            consider(wire.end);
        }
        for junc in self.junctions() {
            consider(junc.position);
        }
        for sym in self.symbols() {
            let Some(lib) = self.get_lib_symbol_resolved(&sym.lib_id)? else {
                continue;
            };
            for (_, pos) in lib
                .get_all_pin_positions(sym.position, sym.rotation, &sym.mirror)
                .iter()
            {
                consider(*pos);
            }
        }
        Ok(best)
    }

    /// Round to the nearest `grid` multiple (upstream default 1.27 mm).
    pub fn snap_to_grid(value: f64, grid: f64) -> f64 {
        py_round(value / grid) * grid
    }

    /// Snap symbol, wire, and junction coordinates in the tree to `grid`.
    pub fn snap_all_to_grid(&mut self, grid: f64) {
        let snap_xy = |node: &mut SExp| {
            let x = get_float(node, 0).unwrap_or(0.0);
            let y = get_float(node, 1).unwrap_or(0.0);
            node.set_value(0, Value::Float(Self::snap_to_grid(x, grid)));
            node.set_value(1, Value::Float(Self::snap_to_grid(y, grid)));
        };
        let root = &mut self.doc.root;
        for sym in root.children.iter_mut().filter(|c| c.has_tag("symbol")) {
            if let Some(at) = find_mut(sym, "at") {
                snap_xy(at);
            }
        }
        for_each_named_mut(root, "wire", &mut |wire| {
            if let Some(pts) = find_mut(wire, "pts") {
                for_each_named_mut(pts, "xy", &mut |xy| snap_xy(xy));
            }
        });
        for_each_named_mut(root, "junction", &mut |junc| {
            if let Some(at) = find_mut(junc, "at") {
                snap_xy(at);
            }
        });
        self.invalidate_cache();
    }

    /// Clear cached parsed views after modifications.
    pub fn invalidate_cache(&mut self) {
        self.cache = Cache::default();
    }

    /// Whether the symbol cache is populated (test hook for upstream `_symbols`).
    pub fn is_symbols_cached(&self) -> bool {
        self.cache.symbols.get().is_some()
    }

    pub fn is_wires_cached(&self) -> bool {
        self.cache.wires.get().is_some()
    }
}

impl fmt::Display for Schematic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = self
            .path()
            .map_or_else(|| "unsaved".to_string(), |p| p.display().to_string());
        write!(
            f,
            "Schematic({path}, symbols={}, wires={})",
            self.symbols().len(),
            self.wires().len()
        )
    }
}
