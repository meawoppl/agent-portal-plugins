//! KiCad s-expression tree: parse, query, edit, and serialize.
//!
//! Port of `kicad_tools.sexp.parser`. Nodes mirror the Python `SExp` shape
//! (`name` + `children` for lists, `value` for atoms) so command ports can be
//! translated directly. Parsed atoms keep their original token, so untouched
//! numbers (`0.1000`) and quoted-vs-bare strings round-trip byte-exact; edited
//! or constructed atoms fall back to KiCad's quoting conventions.

pub mod builders;
mod format;
mod parse;

use std::fmt;
use std::path::Path;

use anyhow::Context;

pub use parse::{parse, parse_all, ParseError};

/// Scalar payload of an atom.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Str(s) => s.parse().ok(),
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.fract() == 0.0 => Some(*f as i64),
            Value::Str(s) => s.parse().ok(),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Str(s) => f.write_str(s),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(v) => f.write_str(&format::float(*v)),
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int(v.into())
    }
}
impl From<usize> for Value {
    fn from(v: usize) -> Self {
        Value::Int(v as i64)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}

/// How an atom appeared in the source, used to reproduce it on save.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum Token {
    /// Constructed or edited: format from the value.
    #[default]
    None,
    /// Parsed as a quoted string.
    Quoted,
    /// Parsed bare; keep bare unless it structurally needs quotes.
    Bare,
    /// Parsed number; the exact original text.
    Number(String),
}

/// One s-expression node: a list (`name` and/or `children`) or an atom (`value`).
#[derive(Debug, Clone, Default)]
pub struct SExp {
    pub name: Option<String>,
    pub children: Vec<SExp>,
    pub value: Option<Value>,
    pub(crate) token: Token,
    /// 1-based source line/column when parsed with positions.
    pub line: u32,
    pub column: u32,
    /// Lexical source of copper arcs, replayed on save while unchanged.
    pub(crate) source_text: Option<(String, String)>,
}

impl PartialEq for SExp {
    /// Structural equality (ignores source formatting and positions).
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.value == other.value && self.children == other.children
    }
}

impl SExp {
    // ----------------------------------------------------------- construction

    /// Bare list `(name children...)`.
    pub fn list(name: impl Into<String>, children: impl IntoIterator<Item = SExp>) -> Self {
        SExp {
            name: Some(name.into()),
            children: children.into_iter().collect(),
            ..Default::default()
        }
    }

    /// Atom formatted by KiCad quoting rules.
    pub fn atom(value: impl Into<Value>) -> Self {
        SExp {
            value: Some(value.into()),
            ..Default::default()
        }
    }

    /// String atom that is always quoted.
    pub fn quoted(value: impl Into<String>) -> Self {
        SExp {
            value: Some(Value::Str(value.into())),
            token: Token::Quoted,
            ..Default::default()
        }
    }

    /// Bare symbol atom (e.g. `yes`, `smd`, `not_allowed`).
    pub fn symbol(value: impl Into<String>) -> Self {
        SExp {
            value: Some(Value::Str(value.into())),
            token: Token::Bare,
            ..Default::default()
        }
    }

    /// `(name value)` convenience.
    pub fn pair(name: &str, value: impl Into<Value>) -> Self {
        SExp::list(name, [SExp::atom(value)])
    }

    // -------------------------------------------------------------- identity

    pub fn is_atom(&self) -> bool {
        self.name.is_none() && self.children.is_empty()
    }

    pub fn is_list(&self) -> bool {
        !self.is_atom()
    }

    pub fn tag(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.name.as_deref() == Some(tag)
    }

    pub fn has_position(&self) -> bool {
        self.line > 0 && self.column > 0
    }

    // --------------------------------------------------------------- queries

    /// First direct child list named `name` (Python `node[name]` / `get`).
    pub fn get(&self, name: &str) -> Option<&SExp> {
        self.children.iter().find(|c| c.has_tag(name))
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut SExp> {
        self.children.iter_mut().find(|c| c.has_tag(name))
    }

    /// Direct children lists named `name`.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a SExp> + 'a {
        self.children.iter().filter(move |c| c.has_tag(name))
    }

    /// Python `find_child`.
    pub fn find_child(&self, tag: &str) -> Option<&SExp> {
        self.get(tag)
    }

    /// Python `find_children`.
    pub fn find_children(&self, tag: &str) -> Vec<&SExp> {
        self.children.iter().filter(|c| c.has_tag(tag)).collect()
    }

    /// Pre-order walk including `self`.
    pub fn iter_all(&self) -> impl Iterator<Item = &SExp> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let node = stack.pop()?;
            stack.extend(node.children.iter().rev());
            Some(node)
        })
    }

    /// First descendant (not self) named `name` (pre-order).
    pub fn find(&self, name: &str) -> Option<&SExp> {
        self.iter_all().skip(1).find(|n| n.has_tag(name))
    }

    /// All descendants (not self) named `name` (pre-order).
    pub fn find_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a SExp> + 'a {
        self.iter_all().skip(1).filter(move |n| n.has_tag(name))
    }

    /// Descendant named `name` whose attributes match, e.g.
    /// `find_where("symbol", &[("lib_id", "Device:R".into())])`. An attribute
    /// matches a child list's first atom, or any direct atom child.
    pub fn find_where(&self, name: &str, attrs: &[(&str, Value)]) -> Option<&SExp> {
        self.iter_all()
            .skip(1)
            .find(|n| n.has_tag(name) && attrs.iter().all(|(k, v)| n.match_attr(k, v)))
    }

    pub fn find_all_where<'a>(
        &'a self,
        name: &'a str,
        attrs: &'a [(&'a str, Value)],
    ) -> impl Iterator<Item = &'a SExp> + 'a {
        self.find_all(name)
            .filter(move |n| attrs.iter().all(|(k, v)| n.match_attr(k, v)))
    }

    fn match_attr(&self, attr: &str, value: &Value) -> bool {
        match self.get(attr) {
            Some(child) => child
                .children
                .first()
                .is_some_and(|c| c.is_atom() && c.value.as_ref() == Some(value)),
            None => self
                .children
                .iter()
                .any(|c| c.is_atom() && c.value.as_ref() == Some(value)),
        }
    }

    /// Atom values of the direct children.
    pub fn atoms(&self) -> impl Iterator<Item = &Value> {
        self.children.iter().filter_map(|c| c.value.as_ref())
    }

    pub fn first_atom(&self) -> Option<&Value> {
        self.atoms().next()
    }

    /// `index`-th child's atom value (Python `get_value`).
    pub fn value_at(&self, index: usize) -> Option<&Value> {
        self.children.get(index).and_then(|c| c.value.as_ref())
    }

    pub fn string_at(&self, index: usize) -> Option<&str> {
        self.value_at(index).and_then(Value::as_str)
    }

    /// String form of any atom (numbers rendered as written).
    pub fn text_at(&self, index: usize) -> Option<String> {
        let child = self.children.get(index)?;
        match (&child.token, &child.value) {
            (Token::Number(raw), _) => Some(raw.clone()),
            (_, Some(v)) => Some(v.to_string()),
            _ => None,
        }
    }

    pub fn float_at(&self, index: usize) -> Option<f64> {
        self.value_at(index).and_then(Value::as_f64)
    }

    pub fn int_at(&self, index: usize) -> Option<i64> {
        self.value_at(index).and_then(Value::as_i64)
    }

    /// First atom of child `name`, as string: `(uuid "x")` -> `x`.
    pub fn child_str(&self, name: &str) -> Option<&str> {
        self.get(name)?.string_at(0)
    }

    pub fn child_f64(&self, name: &str) -> Option<f64> {
        self.get(name)?.float_at(0)
    }

    /// `(property "Name" "Value" ...)` lookup on symbols/footprints.
    pub fn property(&self, key: &str) -> Option<&str> {
        self.children_named("property")
            .find(|p| p.string_at(0) == Some(key))
            .and_then(|p| p.string_at(1))
    }

    /// Whether a bare flag atom (e.g. `hide`, `locked`) or `(flag yes)` is set.
    pub fn flag(&self, name: &str) -> bool {
        self.children.iter().any(|c| {
            (c.is_atom() && c.value.as_ref().and_then(Value::as_str) == Some(name))
                || (c.has_tag(name) && c.string_at(0).is_none_or(|v| v == "yes" || v == "true"))
        })
    }

    /// `(at x y [rot])` of this node.
    pub fn at(&self) -> Option<(f64, f64, f64)> {
        let at = self.get("at")?;
        Some((
            at.float_at(0)?,
            at.float_at(1)?,
            at.float_at(2).unwrap_or(0.0),
        ))
    }

    /// `(xy x y)` points under `(pts ...)`.
    pub fn points(&self) -> Vec<(f64, f64)> {
        self.get("pts")
            .map(|pts| {
                pts.children_named("xy")
                    .filter_map(|xy| Some((xy.float_at(0)?, xy.float_at(1)?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    // --------------------------------------------------------------- editing

    pub fn push(&mut self, child: SExp) -> &mut Self {
        self.children.push(child);
        self
    }

    pub fn insert(&mut self, index: usize, child: SExp) {
        self.children.insert(index.min(self.children.len()), child);
    }

    /// Insert after the last direct child named `name` (append if none).
    pub fn insert_after(&mut self, name: &str, child: SExp) {
        match self.children.iter().rposition(|c| c.has_tag(name)) {
            Some(i) => self.children.insert(i + 1, child),
            None => self.children.push(child),
        }
    }

    /// Insert before the first direct child named `name` (append if none).
    pub fn insert_before(&mut self, name: &str, child: SExp) {
        match self.children.iter().position(|c| c.has_tag(name)) {
            Some(i) => self.children.insert(i, child),
            None => self.children.push(child),
        }
    }

    /// Remove the first direct child named `tag`; returns whether one existed.
    pub fn remove_child(&mut self, tag: &str) -> bool {
        match self.children.iter().position(|c| c.has_tag(tag)) {
            Some(i) => {
                self.children.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn retain(&mut self, keep: impl FnMut(&SExp) -> bool) {
        self.children.retain(keep);
    }

    /// Replace the `index`-th child's atom (Python `set_value`), keeping it
    /// quoted if it was quoted.
    pub fn set_value(&mut self, index: usize, value: impl Into<Value>) {
        let value = value.into();
        match self.children.get_mut(index) {
            Some(child) if child.is_atom() => {
                child.token = match child.token {
                    Token::Quoted if matches!(value, Value::Str(_)) => Token::Quoted,
                    Token::Bare if matches!(value, Value::Str(_)) => Token::Bare,
                    _ => Token::None,
                };
                child.value = Some(value);
            }
            Some(_) => {}
            None => {
                while self.children.len() < index {
                    self.children.push(SExp::atom(""));
                }
                self.children.push(SExp::atom(value));
            }
        }
    }

    /// Set `(name value)`, replacing an existing child or appending one.
    pub fn set_child_value(&mut self, name: &str, value: impl Into<Value>) {
        let value = value.into();
        match self.get_mut(name) {
            Some(child) => child.set_value(0, value),
            None => self.children.push(SExp::pair(name, value)),
        }
    }

    /// Set a `(property "key" "value")` value if present.
    pub fn set_property(&mut self, key: &str, value: &str) -> bool {
        for p in self.children.iter_mut().filter(|c| c.has_tag("property")) {
            if p.string_at(0) == Some(key) {
                p.set_value(1, Value::Str(value.to_string()));
                return true;
            }
        }
        false
    }

    /// Mutable pre-order visit including `self`.
    pub fn walk_mut(&mut self, visit: &mut dyn FnMut(&mut SExp)) {
        visit(self);
        for child in &mut self.children {
            child.walk_mut(visit);
        }
    }

    // ------------------------------------------------------------ serializing

    /// KiCad-style multi-line text (tabs), as `kicad_tools` writes files.
    pub fn to_kicad_string(&self) -> String {
        format::pretty(self, 0, false)
    }

    /// Same, replaying untouched copper-arc source text byte-for-byte.
    pub fn to_kicad_string_preserving(&self) -> String {
        format::pretty(self, 0, true)
    }

    /// Single-line canonical form.
    pub fn to_compact_string(&self) -> String {
        format::compact(self)
    }
}

impl fmt::Display for SExp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_kicad_string())
    }
}

/// A parsed file plus its path (Python `Document`).
#[derive(Debug, Clone)]
pub struct Document {
    pub root: SExp,
    pub path: Option<std::path::PathBuf>,
}

impl Document {
    pub fn load(path: impl AsRef<Path>) -> crate::Result<Self> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let root = parse(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(Document {
            root,
            path: Some(path.to_path_buf()),
        })
    }

    /// Write atomically (temp file + rename) next to the target.
    pub fn save(&self, path: Option<&Path>) -> crate::Result<()> {
        let path = path
            .or(self.path.as_deref())
            .ok_or_else(|| anyhow::anyhow!("document has no path"))?;
        let mut text = self.root.to_kicad_string_preserving();
        text.push('\n');
        crate::fsutil::atomic_write(path, text.as_bytes())
    }
}

/// Parse a file from disk.
pub fn parse_file(path: impl AsRef<Path>) -> crate::Result<SExp> {
    Ok(Document::load(path)?.root)
}

#[cfg(test)]
mod tests;
