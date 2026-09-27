//! Minimal KiCad s-expression tree used by the library view.
//!
//! Atoms keep their raw token text (including quotes and escapes) so a parsed
//! tree can be re-serialized into text KiCad will load again. Formatting is
//! compact (single spaces), which is fine for throwaway render inputs and for
//! drift comparisons.

use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sexp {
    Atom(String),
    List(Vec<Sexp>),
}

impl Sexp {
    pub fn atom(raw: impl Into<String>) -> Self {
        Sexp::Atom(raw.into())
    }

    pub fn string(value: &str) -> Self {
        Sexp::Atom(quote(value))
    }

    pub fn items(&self) -> &[Sexp] {
        match self {
            Sexp::List(items) => items,
            Sexp::Atom(_) => &[],
        }
    }

    pub fn items_mut(&mut self) -> Option<&mut Vec<Sexp>> {
        match self {
            Sexp::List(items) => Some(items),
            Sexp::Atom(_) => None,
        }
    }

    /// The unquoted head symbol of a list, e.g. `footprint` for `(footprint ...)`.
    pub fn head(&self) -> Option<&str> {
        match self.items().first() {
            Some(Sexp::Atom(raw)) => Some(raw.as_str()),
            _ => None,
        }
    }

    pub fn is(&self, head: &str) -> bool {
        self.head() == Some(head)
    }

    /// Unquoted string value of the atom at `index` of this list.
    pub fn str_at(&self, index: usize) -> Option<String> {
        match self.items().get(index) {
            Some(Sexp::Atom(raw)) => Some(unquote(raw)),
            _ => None,
        }
    }

    pub fn child(&self, head: &str) -> Option<&Sexp> {
        self.items().iter().find(|item| item.is(head))
    }

    pub fn children<'a>(&'a self, head: &'a str) -> impl Iterator<Item = &'a Sexp> + 'a {
        self.items().iter().filter(move |item| item.is(head))
    }

    /// `(property "Name" "Value" ...)` lookup on a symbol or footprint node.
    pub fn property(&self, name: &str) -> Option<String> {
        self.children("property")
            .find(|prop| prop.str_at(1).as_deref() == Some(name))
            .and_then(|prop| prop.str_at(2))
    }

    /// All `(property "Name" "Value")` pairs in order.
    pub fn properties(&self) -> Vec<(String, String)> {
        self.children("property")
            .filter_map(|prop| Some((prop.str_at(1)?, prop.str_at(2)?)))
            .collect()
    }

    /// Visit every list node depth-first (pre-order), mutably.
    pub fn walk_mut(&mut self, visit: &mut dyn FnMut(&mut Sexp)) {
        visit(self);
        if let Sexp::List(items) = self {
            for item in items {
                item.walk_mut(visit);
            }
        }
    }

    /// Keep only the children (after the head atom) for which `keep` is true.
    pub fn retain_children(&mut self, mut keep: impl FnMut(&Sexp) -> bool) {
        if let Sexp::List(items) = self {
            let mut first = true;
            items.retain(|item| std::mem::take(&mut first) || keep(item));
        }
    }
}

impl std::fmt::Display for Sexp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Sexp::Atom(raw) => f.write_str(raw),
            Sexp::List(items) => {
                f.write_str("(")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str(")")
            }
        }
    }
}

pub fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

pub fn unquote(raw: &str) -> String {
    let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return raw.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse every top-level expression in `text`.
pub fn parse_all(text: &str) -> Result<Vec<Sexp>> {
    let bytes = text.as_bytes();
    let mut stack: Vec<Vec<Sexp>> = Vec::new();
    let mut top = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b'(' => {
                stack.push(Vec::new());
                index += 1;
            }
            b')' => {
                let items = stack
                    .pop()
                    .ok_or_else(|| anyhow!("unbalanced ')' at byte {index}"))?;
                let node = Sexp::List(items);
                match stack.last_mut() {
                    Some(parent) => parent.push(node),
                    None => top.push(node),
                }
                index += 1;
            }
            b'"' => {
                let start = index;
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => {
                            index += 1;
                            index += text[index..].chars().next().map_or(0, char::len_utf8);
                        }
                        b'"' => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                let end = index.min(bytes.len());
                push_atom(&mut stack, &mut top, &text[start..end]);
            }
            _ if byte.is_ascii_whitespace() => index += 1,
            _ => {
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'(' | b')' | b'"')
                {
                    index += 1;
                }
                push_atom(&mut stack, &mut top, &text[start..index]);
            }
        }
    }
    if !stack.is_empty() {
        return Err(anyhow!("unterminated s-expression"));
    }
    Ok(top)
}

fn push_atom(stack: &mut [Vec<Sexp>], top: &mut Vec<Sexp>, raw: &str) {
    let atom = Sexp::Atom(raw.to_string());
    match stack.last_mut() {
        Some(parent) => parent.push(atom),
        None => top.push(atom),
    }
}

/// Parse a single expression (the first list in `text`).
pub fn parse(text: &str) -> Result<Sexp> {
    parse_all(text)?
        .into_iter()
        .find(|node| matches!(node, Sexp::List(_)))
        .ok_or_else(|| anyhow!("no s-expression found"))
}

/// Canonicalize bare numeric atoms so `1.2700`, `1.27` and `1.27000001`
/// compare equal. Quoted strings are left untouched.
pub fn normalize_number(raw: &str) -> Option<String> {
    if raw.starts_with('"') {
        return None;
    }
    let first = raw.chars().next()?;
    if !(first.is_ascii_digit() || first == '-' || first == '+' || first == '.') {
        return None;
    }
    let value: f64 = raw.parse().ok()?;
    let rounded = (value * 10_000.0).round() / 10_000.0;
    let mut text = format!("{rounded:.4}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text == "-0" {
        text = "0".to_string();
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_nested_lists_and_strings() {
        let text = r#"(footprint "Lib:Name \"q\"" (at 1 2) (property "Reference" "R1"))"#;
        let node = parse(text).unwrap();
        assert_eq!(node.head(), Some("footprint"));
        assert_eq!(node.str_at(1).unwrap(), "Lib:Name \"q\"");
        assert_eq!(node.property("Reference").as_deref(), Some("R1"));
        assert_eq!(node.to_string(), text);
    }

    #[test]
    fn escaped_multibyte_characters_do_not_split() {
        let node = parse("(a \"x\\\u{00b5}y\" b)").unwrap();
        assert_eq!(node.str_at(1).unwrap(), "x\u{00b5}y");
    }

    #[test]
    fn rejects_unbalanced() {
        assert!(parse_all("(a (b)").is_err());
        assert!(parse_all("(a))").is_err());
    }

    #[test]
    fn normalizes_numbers() {
        assert_eq!(normalize_number("1.2700").as_deref(), Some("1.27"));
        assert_eq!(normalize_number("-0.00001").as_deref(), Some("0"));
        assert_eq!(
            normalize_number("30.480000000000004").as_deref(),
            Some("30.48")
        );
        assert_eq!(normalize_number("\"1.0\""), None);
        assert_eq!(normalize_number("F.Cu"), None);
    }
}
