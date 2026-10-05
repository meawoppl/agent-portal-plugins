//! Insertion-ordered JSON that renders like Python's `json.dumps`.
//!
//! Upstream emits JSON from plain `dict`s, so key order is insertion order and
//! floats use `repr` (`1.0`, `1e-05`). `serde_json::Value` (without the
//! workspace-wide `preserve_order` feature) sorts keys, so report ports build
//! [`Json`] values instead and render them with [`dumps`] / [`dumps_indent`].
//! Parsing ([`loads`]) keeps object order and the int/float distinction so
//! pass-through data round-trips like upstream.

use std::fmt::Write as _;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Json {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// Build an ordered [`Json::Obj`]: `jobj! { "a" => 1, "b" => "x" }`.
#[macro_export]
macro_rules! jobj {
    () => { $crate::pyjson::Json::Obj(Vec::new()) };
    ($($k:expr => $v:expr),+ $(,)?) => {
        $crate::pyjson::Json::Obj(vec![$((($k).to_string(), $crate::pyjson::Json::from($v))),+])
    };
}

impl Json {
    pub fn obj() -> Self {
        Json::Obj(Vec::new())
    }

    /// Insert or replace `key` (replacement keeps the original position, like
    /// assigning to an existing Python dict key).
    pub fn set(&mut self, key: &str, value: impl Into<Json>) {
        if let Json::Obj(items) = self {
            let value = value.into();
            if let Some(slot) = items.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value;
            } else {
                items.push((key.to_string(), value));
            }
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Json> {
        if let Json::Obj(items) = self {
            let idx = items.iter().position(|(k, _)| k == key)?;
            return Some(items.remove(idx).1);
        }
        None
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Obj(items) => items.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Numeric value (bools count as 0/1 like Python).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Int(i) => Some(*i as f64),
            Json::Float(f) => Some(*f),
            Json::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(i) => Some(*i),
            Json::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    pub fn is_int(&self) -> bool {
        matches!(self, Json::Int(_))
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Json::Null => false,
            Json::Bool(b) => *b,
            Json::Int(i) => *i != 0,
            Json::Float(f) => *f != 0.0,
            Json::Str(s) => !s.is_empty(),
            Json::Arr(a) => !a.is_empty(),
            Json::Obj(o) => !o.is_empty(),
        }
    }

    /// Python `type(x).__name__`.
    pub fn py_type_name(&self) -> &'static str {
        match self {
            Json::Null => "NoneType",
            Json::Bool(_) => "bool",
            Json::Int(_) => "int",
            Json::Float(_) => "float",
            Json::Str(_) => "str",
            Json::Arr(_) => "list",
            Json::Obj(_) => "dict",
        }
    }

    /// Python `repr()` of the equivalent Python object.
    pub fn py_repr(&self) -> String {
        match self {
            Json::Null => "None".into(),
            Json::Bool(true) => "True".into(),
            Json::Bool(false) => "False".into(),
            Json::Int(i) => i.to_string(),
            Json::Float(f) => py_float_repr(*f),
            Json::Str(s) => py_repr_str(s),
            Json::Arr(a) => format!(
                "[{}]",
                a.iter().map(Json::py_repr).collect::<Vec<_>>().join(", ")
            ),
            Json::Obj(o) => format!(
                "{{{}}}",
                o.iter()
                    .map(|(k, v)| format!("{}: {}", py_repr_str(k), v.py_repr()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Float that serializes as an int when `is_int` (mirrors values that were
    /// Python ints upstream, e.g. `pos.get("x", 0)`).
    pub fn num(value: f64, is_int: bool) -> Json {
        if is_int && value.is_finite() && value.fract() == 0.0 {
            Json::Int(value as i64)
        } else {
            Json::Float(value)
        }
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Json {
            fn from(v: $t) -> Self { Json::Int(v as i64) }
        }
    )*};
}
from_int!(i32, i64, u8, u32, u64, usize);

impl From<f64> for Json {
    fn from(v: f64) -> Self {
        Json::Float(v)
    }
}
impl From<bool> for Json {
    fn from(v: bool) -> Self {
        Json::Bool(v)
    }
}
impl From<&str> for Json {
    fn from(v: &str) -> Self {
        Json::Str(v.to_string())
    }
}
impl From<String> for Json {
    fn from(v: String) -> Self {
        Json::Str(v)
    }
}
impl From<&String> for Json {
    fn from(v: &String) -> Self {
        Json::Str(v.clone())
    }
}
impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Self {
        v.map_or(Json::Null, Into::into)
    }
}
impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(v: Vec<T>) -> Self {
        Json::Arr(v.into_iter().map(Into::into).collect())
    }
}
impl<T: Clone + Into<Json>> From<&[T]> for Json {
    fn from(v: &[T]) -> Self {
        Json::Arr(v.iter().cloned().map(Into::into).collect())
    }
}
impl<T: Clone + Into<Json>> From<&Vec<T>> for Json {
    fn from(v: &Vec<T>) -> Self {
        Json::Arr(v.iter().cloned().map(Into::into).collect())
    }
}

/// Python `repr(float)`.
pub fn py_float_repr(v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    // Shortest round-trip digits via `{:e}`: "d.ddde[-]x".
    let sci = format!("{v:e}");
    let (mantissa, exp) = sci.split_once('e').expect("exponent");
    let exp: i32 = exp.parse().expect("exponent int");
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if negative { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let n = digits.len() as i32;
        let body = if exp >= 0 {
            let int_len = exp + 1;
            if n <= int_len {
                format!("{digits}{}.0", "0".repeat((int_len - n) as usize))
            } else {
                format!(
                    "{}.{}",
                    &digits[..int_len as usize],
                    &digits[int_len as usize..]
                )
            }
        } else {
            format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
        };
        format!("{sign}{body}")
    } else {
        let m = if digits.len() > 1 {
            format!("{}.{}", &digits[..1], &digits[1..])
        } else {
            digits
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{sign}{m}e{esign}{:02}", exp.abs())
    }
}

/// JSON float text as Python's `json.dumps` writes it.
fn json_float(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v.is_infinite() {
        if v > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        }
    } else {
        py_float_repr(v)
    }
}

/// Python `repr(str)`.
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr(list_of_str)`, e.g. `['C52', 'U10']`.
pub fn py_repr_str_list<S: AsRef<str>>(items: &[S]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|s| py_repr_str(s.as_ref()))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn write_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if c.is_ascii() && (c as u32) >= 0x20 => out.push(c),
            c => {
                // ensure_ascii=True: escape everything else as UTF-16 units.
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

fn write_json(out: &mut String, v: &Json, indent: Option<usize>, level: usize) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Json::Float(f) => out.push_str(&json_float(*f)),
        Json::Str(s) => write_json_str(out, s),
        Json::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                if let Some(n) = indent {
                    out.push('\n');
                    out.push_str(&" ".repeat(n * (level + 1)));
                }
                write_json(out, item, indent, level + 1);
            }
            if let Some(n) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(n * level));
            }
            out.push(']');
        }
        Json::Obj(items) => {
            if items.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, item)) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                if let Some(n) = indent {
                    out.push('\n');
                    out.push_str(&" ".repeat(n * (level + 1)));
                }
                write_json_str(out, k);
                out.push_str(": ");
                write_json(out, item, indent, level + 1);
            }
            if let Some(n) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(n * level));
            }
            out.push('}');
        }
    }
}

/// `json.dumps(v)` (compact, Python default separators).
pub fn dumps(v: &Json) -> String {
    let mut out = String::new();
    write_json(&mut out, v, None, 0);
    out
}

/// `json.dumps(v, indent=n)`.
pub fn dumps_indent(v: &Json, indent: usize) -> String {
    let mut out = String::new();
    write_json(&mut out, v, Some(indent), 0);
    out
}

/// Parse JSON text, preserving object key order and int/float distinction.
pub fn loads(text: &str) -> Result<Json, serde_json::Error> {
    serde_json::from_str(text)
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => s.serialize_unit(),
            Json::Bool(b) => s.serialize_bool(*b),
            Json::Int(i) => s.serialize_i64(*i),
            Json::Float(f) => s.serialize_f64(*f),
            Json::Str(v) => s.serialize_str(v),
            Json::Arr(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Json::Obj(items) => {
                let mut map = s.serialize_map(Some(items.len()))?;
                for (k, v) in items {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("any JSON value")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Json, E> {
        Ok(Json::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Json, E> {
        Ok(Json::Int(v))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Json, E> {
        Ok(i64::try_from(v).map_or(Json::Float(v as f64), Json::Int))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Json, E> {
        Ok(Json::Float(v))
    }
    fn visit_str<E>(self, v: &str) -> Result<Json, E> {
        Ok(Json::Str(v.to_string()))
    }
    fn visit_string<E>(self, v: String) -> Result<Json, E> {
        Ok(Json::Str(v))
    }
    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }
    fn visit_none<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Json, D::Error> {
        Json::deserialize(d)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Json::Arr(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut items: Vec<(String, Json)> = Vec::new();
        while let Some((k, v)) = map.next_entry::<String, Json>()? {
            // Python dicts keep the last value for duplicate keys, in the
            // first key's position.
            if let Some(slot) = items.iter_mut().find(|(key, _)| *key == k) {
                slot.1 = v;
            } else {
                items.push((k, v));
            }
        }
        Ok(Json::Obj(items))
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Json, D::Error> {
        d.deserialize_any(JsonVisitor)
    }
}

/// Python `str.splitlines()`.
pub fn py_splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let is_break = matches!(
            c,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if is_break {
            out.push(&s[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(j, '\n')) = chars.peek() {
                    chars.next();
                    next = j + 1;
                }
            }
            start = next;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Python `str.title()` (approximation: letters are the cased characters).
pub fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_cased = true;
        } else {
            out.push(c);
            prev_cased = false;
        }
    }
    out
}

/// Python `round(x, ndigits)` (correctly rounded, half-to-even on the exact
/// binary value, which is what `{:.N}` formatting does).
pub fn py_round(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}").parse().unwrap_or(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_repr_matches_python() {
        assert_eq!(py_float_repr(1.0), "1.0");
        assert_eq!(py_float_repr(0.1), "0.1");
        assert_eq!(py_float_repr(100.5), "100.5");
        assert_eq!(py_float_repr(1e-5), "1e-05");
        assert_eq!(py_float_repr(0.0001), "0.0001");
        assert_eq!(py_float_repr(1e16), "1e+16");
        assert_eq!(py_float_repr(123456789012345.0), "123456789012345.0");
        assert_eq!(py_float_repr(-0.05), "-0.05");
        assert_eq!(py_float_repr(1.5e-7), "1.5e-07");
    }

    #[test]
    fn dumps_indent_matches_python() {
        let v = jobj! {"a" => 1, "b" => Json::Arr(vec![]), "c" => jobj!{}, "d" => vec![1.0, 2.5]};
        assert_eq!(
            dumps_indent(&v, 2),
            "{\n  \"a\": 1,\n  \"b\": [],\n  \"c\": {},\n  \"d\": [\n    1.0,\n    2.5\n  ]\n}"
        );
        assert_eq!(dumps(&jobj! {"k" => "é"}), "{\"k\": \"\\u00e9\"}");
    }

    #[test]
    fn loads_preserves_order_and_ints() {
        let v = loads(r#"{"z": 1, "a": 2.0}"#).unwrap();
        assert_eq!(dumps(&v), r#"{"z": 1, "a": 2.0}"#);
    }

    #[test]
    fn repr_helpers() {
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        assert_eq!(py_repr_str_list(&["C52", "U10"]), "['C52', 'U10']");
        assert_eq!(py_title("foo_bar baz1qux"), "Foo_Bar Baz1Qux");
        assert_eq!(py_splitlines("a\r\nb\nc"), vec!["a", "b", "c"]);
        assert_eq!(py_round(0.1234567, 3), 0.123);
    }
}
