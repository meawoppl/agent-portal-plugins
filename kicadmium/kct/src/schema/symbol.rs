//! Symbol instance model (port of `kicad_tools.schema.symbol`).
//!
//! Represents a component instance placed in a schematic. Also hosts the
//! small helpers shared by the `schema` modules: an insertion-ordered map
//! (Python `dict` semantics), UUID v4 generation, and `SExp` accessors that
//! mirror the Python `get_string`/`get_float` conventions.

use std::collections::HashMap;
use std::fmt;

use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::sexp::{SExp, Value};

// ---------------------------------------------------------------- helpers

/// Insertion-ordered string-keyed map with Python `dict` semantics:
/// re-inserting an existing key replaces the value in place.
#[derive(Debug, Clone)]
pub struct OrderedMap<V> {
    entries: Vec<(String, V)>,
    index: HashMap<String, usize>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        OrderedMap {
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl<V: PartialEq> PartialEq for OrderedMap<V> {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl<V> OrderedMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.index.get(key).map(|&i| &self.entries[i].1)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        match self.index.get(key) {
            Some(&i) => Some(&mut self.entries[i].1),
            None => None,
        }
    }

    /// Insert or replace (keeping the original position); returns the old value.
    pub fn insert(&mut self, key: impl Into<String>, value: V) -> Option<V> {
        let key = key.into();
        match self.index.get(&key) {
            Some(&i) => Some(std::mem::replace(&mut self.entries[i].1, value)),
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, value));
                None
            }
        }
    }

    /// Remove a key, preserving the order of the remaining entries.
    pub fn remove(&mut self, key: &str) -> Option<V> {
        let i = self.index.remove(key)?;
        let (_, v) = self.entries.remove(i);
        for slot in self.index.values_mut() {
            if *slot > i {
                *slot -= 1;
            }
        }
        Some(v)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.iter_mut().map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut V)> {
        self.entries.iter_mut().map(|(k, v)| (k.as_str(), v))
    }

    /// Entry at insertion position `i`.
    pub fn get_index(&self, i: usize) -> Option<(&str, &V)> {
        self.entries.get(i).map(|(k, v)| (k.as_str(), v))
    }

    pub fn get_index_mut(&mut self, i: usize) -> Option<(&str, &mut V)> {
        self.entries.get_mut(i).map(|(k, v)| (k.as_str(), v))
    }
}

impl<K: Into<String>, V> FromIterator<(K, V)> for OrderedMap<V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = OrderedMap::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl<V> std::ops::Index<&str> for OrderedMap<V> {
    type Output = V;
    fn index(&self, key: &str) -> &V {
        self.get(key)
            .unwrap_or_else(|| panic!("key not found: {key:?}"))
    }
}

impl<V: Serialize> Serialize for OrderedMap<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (k, v) in self.iter() {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// Random RFC 4122 version-4 UUID string (Python `str(uuid.uuid4())`).
pub fn new_uuid() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let word = |salt: u64| {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u64(count);
        h.write_u64(salt);
        h.finish()
    };
    let hi = word(0x9e37_79b9_7f4a_7c15);
    let lo = word(0xc2b2_ae3d_27d4_eb4f);
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&hi.to_be_bytes());
    b[8..].copy_from_slice(&lo.to_be_bytes());
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Python `SExp.get_string(index)`: string form of the `index`-th atom child.
pub(crate) fn get_string(node: &SExp, index: usize) -> Option<String> {
    let child = node.children.get(index)?;
    if child.is_atom() {
        node.text_at(index)
    } else {
        None
    }
}

/// Python `SExp.get_float(index)`.
pub(crate) fn get_float(node: &SExp, index: usize) -> Option<f64> {
    node.float_at(index)
}

/// Python `SExp.get_int(index)` (floats are not integers).
pub(crate) fn get_int(node: &SExp, index: usize) -> Option<i64> {
    match node.value_at(index)? {
        Value::Int(i) => Some(*i),
        Value::Str(s) => s.parse().ok(),
        Value::Float(_) => None,
    }
}

/// `get_string(0)` of the first descendant named `name` (Python
/// `node.find(name).get_string(0)`), defaulting to `""`.
pub(crate) fn find_string(node: &SExp, name: &str) -> String {
    node.find(name)
        .and_then(|n| get_string(n, 0))
        .unwrap_or_default()
}

/// `(at x y [rot])` of the first descendant `at`, as upstream parses it.
pub(crate) fn find_at(node: &SExp) -> ((f64, f64), f64) {
    match node.find("at") {
        Some(at) => (
            (
                get_float(at, 0).unwrap_or(0.0),
                get_float(at, 1).unwrap_or(0.0),
            ),
            get_float(at, 2).unwrap_or(0.0),
        ),
        None => ((0.0, 0.0), 0.0),
    }
}

/// First descendant (pre-order, excluding `node`) named `name`, mutably.
pub(crate) fn find_mut<'a>(node: &'a mut SExp, name: &str) -> Option<&'a mut SExp> {
    let pos = node
        .children
        .iter()
        .position(|c| c.has_tag(name) || c.iter_all().skip(1).any(|d| d.has_tag(name)))?;
    let child = &mut node.children[pos];
    if child.has_tag(name) {
        Some(child)
    } else {
        find_mut(child, name)
    }
}

/// Visit every descendant (excluding `node`) named `name`, mutably.
pub(crate) fn for_each_named_mut(node: &mut SExp, name: &str, f: &mut dyn FnMut(&mut SExp)) {
    for child in &mut node.children {
        if child.has_tag(name) {
            f(child);
        }
        for_each_named_mut(child, name, f);
    }
}

/// `(name v1 v2 ...)` of floats.
pub(crate) fn num_list(name: &str, values: &[f64]) -> SExp {
    SExp::list(name, values.iter().map(|&v| SExp::atom(v)))
}

/// Python `repr` of a float tuple, e.g. `(100, 97.46)`.
pub(crate) fn fmt_point(p: (f64, f64)) -> String {
    format!("({}, {})", p.0, p.1)
}

fn font_effects(hide: bool) -> SExp {
    let mut effects = SExp::list(
        "effects",
        [SExp::list("font", [num_list("size", &[1.27, 1.27])])],
    );
    if hide {
        effects.push(SExp::list("hide", [SExp::symbol("yes")]));
    }
    effects
}

fn yes_no(flag: bool) -> SExp {
    SExp::symbol(if flag { "yes" } else { "no" })
}

// ------------------------------------------------------------------ model

/// A pin on a symbol instance.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SymbolPin {
    pub number: String,
    pub uuid: String,
    pub name: Option<String>,
}

impl SymbolPin {
    pub fn new(number: impl Into<String>, uuid: impl Into<String>) -> Self {
        SymbolPin {
            number: number.into(),
            uuid: uuid.into(),
            name: None,
        }
    }

    /// `(pin "1" (uuid "..."))`; the pin number is always quoted.
    pub fn to_sexp(&self) -> SExp {
        let uuid = if self.uuid.is_empty() {
            new_uuid()
        } else {
            self.uuid.clone()
        };
        SExp::list(
            "pin",
            [
                SExp::quoted(self.number.clone()),
                SExp::list("uuid", [SExp::quoted(uuid)]),
            ],
        )
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        SymbolPin {
            number: get_string(sexp, 0).unwrap_or_default(),
            uuid: find_string(sexp, "uuid"),
            name: None,
        }
    }
}

/// A property on a symbol (Reference, Value, Footprint, etc.).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SymbolProperty {
    pub name: String,
    pub value: String,
    pub position: (f64, f64),
    pub rotation: f64,
    pub visible: bool,
}

impl SymbolProperty {
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
        position: (f64, f64),
        rotation: f64,
        visible: bool,
    ) -> Self {
        SymbolProperty {
            name: name.into(),
            value: value.into(),
            position,
            rotation,
            visible,
        }
    }

    /// `(property "Name" "Value" (at X Y R) (effects (font (size 1.27 1.27)) [(hide yes)]))`.
    pub fn to_sexp(&self) -> SExp {
        SExp::list(
            "property",
            [
                SExp::quoted(self.name.clone()),
                SExp::quoted(self.value.clone()),
                num_list("at", &[self.position.0, self.position.1, self.rotation]),
                font_effects(!self.visible),
            ],
        )
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (position, rotation) = find_at(sexp);
        // KiCad <= 8 writes `(effects ... (hide yes))` (or a bare `hide`
        // atom); KiCad 9 moved the flag to the property itself:
        // `(property "Footprint" "..." (at ...) (effects ...) (hide yes))`.
        // Only a `(hide ...)` *list* is honoured at property level so a
        // property whose value is literally `hide` is not mistaken for one.
        let hidden_here = sexp
            .children_named("hide")
            .any(|h| h.string_at(0).is_none_or(|v| v == "yes" || v == "true"));
        let visible = !(hidden_here || sexp.find("effects").is_some_and(|e| e.flag("hide")));
        SymbolProperty {
            name: get_string(sexp, 0).unwrap_or_default(),
            value: get_string(sexp, 1).unwrap_or_default(),
            position,
            rotation,
            visible,
        }
    }
}

/// A symbol instance placed in a schematic.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SymbolInstance {
    pub lib_id: String,
    pub uuid: String,
    pub position: (f64, f64),
    pub rotation: f64,
    /// `""`, `"x"`, or `"y"`.
    pub mirror: String,
    pub unit: i64,
    pub in_bom: bool,
    pub on_board: bool,
    pub dnp: bool,
    pub properties: OrderedMap<SymbolProperty>,
    pub pins: Vec<SymbolPin>,
    pub project_name: String,
    pub instance_path: String,
}

impl Default for SymbolInstance {
    fn default() -> Self {
        SymbolInstance {
            lib_id: String::new(),
            uuid: String::new(),
            position: (0.0, 0.0),
            rotation: 0.0,
            mirror: String::new(),
            unit: 1,
            in_bom: true,
            on_board: true,
            dnp: false,
            properties: OrderedMap::new(),
            pins: Vec::new(),
            project_name: String::new(),
            instance_path: String::new(),
        }
    }
}

impl SymbolInstance {
    pub fn new(lib_id: impl Into<String>, uuid: impl Into<String>) -> Self {
        SymbolInstance {
            lib_id: lib_id.into(),
            uuid: uuid.into(),
            ..Default::default()
        }
    }

    fn prop(&self, name: &str) -> &str {
        self.properties
            .get(name)
            .map(|p| p.value.as_str())
            .unwrap_or("")
    }

    /// Reference designator (e.g. `R1`), `""` if absent.
    pub fn reference(&self) -> &str {
        self.prop("Reference")
    }

    pub fn value(&self) -> &str {
        self.prop("Value")
    }

    pub fn footprint(&self) -> &str {
        self.prop("Footprint")
    }

    pub fn datasheet(&self) -> &str {
        self.prop("Datasheet")
    }

    pub fn get_property(&self, name: &str) -> Option<&str> {
        self.properties.get(name).map(|p| p.value.as_str())
    }

    /// KiCad 8 symbol instance form, including an `(instances ...)` block
    /// when both `project_name` and `instance_path` are set.
    pub fn to_sexp(&self) -> SExp {
        let mut sym = SExp::list("symbol", []);
        sym.push(SExp::list("lib_id", [SExp::quoted(self.lib_id.clone())]));
        sym.push(num_list(
            "at",
            &[self.position.0, self.position.1, self.rotation],
        ));
        if !self.mirror.is_empty() {
            sym.push(SExp::list("mirror", [SExp::symbol(self.mirror.clone())]));
        }
        sym.push(SExp::pair("unit", self.unit));
        sym.push(SExp::list("in_bom", [yes_no(self.in_bom)]));
        sym.push(SExp::list("on_board", [yes_no(self.on_board)]));
        sym.push(SExp::list("dnp", [yes_no(self.dnp)]));
        sym.push(SExp::list("uuid", [SExp::quoted(self.uuid.clone())]));
        for prop in self.properties.values() {
            sym.push(prop.to_sexp());
        }
        for pin in &self.pins {
            sym.push(pin.to_sexp());
        }
        if !self.project_name.is_empty() && !self.instance_path.is_empty() {
            let path = SExp::list(
                "path",
                [
                    SExp::quoted(self.instance_path.clone()),
                    SExp::list("reference", [SExp::quoted(self.reference().to_string())]),
                    SExp::pair("unit", self.unit),
                ],
            );
            let project = SExp::list("project", [SExp::quoted(self.project_name.clone()), path]);
            sym.push(SExp::list("instances", [project]));
        }
        sym
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (position, rotation) = find_at(sexp);
        let unit = sexp
            .find("unit")
            .and_then(|u| get_int(u, 0))
            .filter(|&u| u != 0)
            .unwrap_or(1);
        let in_bom = sexp
            .find("in_bom")
            .is_none_or(|n| get_string(n, 0).as_deref() != Some("no"));
        let on_board = sexp
            .find("on_board")
            .is_none_or(|n| get_string(n, 0).as_deref() != Some("no"));
        let dnp = sexp
            .find("dnp")
            .is_some_and(|n| get_string(n, 0).as_deref() == Some("yes"));

        let mut properties = OrderedMap::new();
        for prop in sexp.find_all("property") {
            let sp = SymbolProperty::from_sexp(prop);
            properties.insert(sp.name.clone(), sp);
        }
        let pins = sexp.find_all("pin").map(SymbolPin::from_sexp).collect();

        let mut project_name = String::new();
        let mut instance_path = String::new();
        if let Some(project) = sexp.find("instances").and_then(|i| i.find("project")) {
            project_name = get_string(project, 0).unwrap_or_default();
            if let Some(path) = project.find("path") {
                instance_path = get_string(path, 0).unwrap_or_default();
            }
        }

        SymbolInstance {
            lib_id: find_string(sexp, "lib_id"),
            uuid: find_string(sexp, "uuid"),
            position,
            rotation,
            mirror: find_string(sexp, "mirror"),
            unit,
            in_bom,
            on_board,
            dnp,
            properties,
            pins,
            project_name,
            instance_path,
        }
    }
}

impl fmt::Display for SymbolInstance {
    /// Python `repr`: `SymbolInstance('R1', lib='Device:R', pos=(100, 100))`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SymbolInstance({:?}, lib={:?}, pos={})",
            self.reference(),
            self.lib_id,
            fmt_point(self.position)
        )
    }
}
