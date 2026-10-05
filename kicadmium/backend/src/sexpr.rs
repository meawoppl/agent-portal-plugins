//! Minimal KiCad s-expression reader used by the layout-quality audits.
//!
//! Parses the whole file into a tree of lists and atoms. Quoted strings are
//! unescaped; everything else is kept as the raw token.

use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    Atom(String),
    List(Vec<Node>),
}

impl Node {
    pub(crate) fn items(&self) -> &[Node] {
        match self {
            Node::List(items) => items,
            Node::Atom(_) => &[],
        }
    }

    pub(crate) fn atom(&self) -> Option<&str> {
        match self {
            Node::Atom(value) => Some(value),
            Node::List(_) => None,
        }
    }

    /// First atom of a list, e.g. `segment` for `(segment ...)`.
    pub(crate) fn head(&self) -> Option<&str> {
        self.items().first().and_then(Node::atom)
    }

    pub(crate) fn is(&self, head: &str) -> bool {
        self.head() == Some(head)
    }

    /// Direct child lists with the given head.
    pub(crate) fn children<'a>(&'a self, head: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.items().iter().filter(move |node| node.is(head))
    }

    pub(crate) fn child(&self, head: &str) -> Option<&Node> {
        self.items().iter().find(|node| node.is(head))
    }

    /// Atom at position `index` (0 is the head).
    pub(crate) fn arg(&self, index: usize) -> Option<&str> {
        self.items().get(index).and_then(Node::atom)
    }

    pub(crate) fn num(&self, index: usize) -> Option<f64> {
        self.arg(index)?.parse().ok()
    }

    /// `(name value)` child atom.
    pub(crate) fn value(&self, head: &str) -> Option<&str> {
        self.child(head)?.arg(1)
    }

    /// `(name x y)` child as a point.
    pub(crate) fn point(&self, head: &str) -> Option<(f64, f64)> {
        let node = self.child(head)?;
        Some((node.num(1)?, node.num(2)?))
    }

    /// All atoms after the head, e.g. layer names in `(layers "F.Cu" "B.Cu")`.
    pub(crate) fn args(&self) -> impl Iterator<Item = &str> {
        self.items().iter().skip(1).filter_map(Node::atom)
    }

    /// `(hide yes)` child or a bare `hide` atom (older formats).
    pub(crate) fn hidden(&self) -> bool {
        self.items().iter().any(|node| match node {
            Node::Atom(value) => value == "hide",
            Node::List(_) => node.is("hide") && node.arg(1) != Some("no"),
        })
    }
}

pub(crate) fn parse(text: &str) -> Result<Node> {
    let bytes = text.as_bytes();
    let mut stack: Vec<Vec<Node>> = Vec::new();
    let mut root: Option<Node> = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                stack.push(Vec::new());
                i += 1;
            }
            b')' => {
                let list = stack
                    .pop()
                    .ok_or_else(|| anyhow!("unbalanced ')' at {i}"))?;
                let node = Node::List(list);
                match stack.last_mut() {
                    Some(parent) => parent.push(node),
                    None if root.is_none() => root = Some(node),
                    None => return Err(anyhow!("multiple top-level expressions")),
                }
                i += 1;
            }
            b'"' => {
                let mut value = Vec::new();
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 1;
                        value.push(match bytes[i] {
                            b'n' => b'\n',
                            b't' => b'\t',
                            other => other,
                        });
                    } else {
                        value.push(bytes[i]);
                    }
                    i += 1;
                }
                i += 1;
                let atom = Node::Atom(String::from_utf8_lossy(&value).into_owned());
                stack
                    .last_mut()
                    .ok_or_else(|| anyhow!("string outside expression"))?
                    .push(atom);
            }
            c if c.is_ascii_whitespace() => i += 1,
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'(' | b')' | b'"')
                {
                    i += 1;
                }
                let atom = Node::Atom(text[start..i].to_string());
                stack
                    .last_mut()
                    .ok_or_else(|| anyhow!("atom outside expression"))?
                    .push(atom);
            }
        }
    }
    if !stack.is_empty() {
        return Err(anyhow!("unterminated expression"));
    }
    root.ok_or_else(|| anyhow!("empty s-expression"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_lists_and_strings() {
        let node = parse(r#"(kicad_pcb (via (at 1.5 -2) (layers "F.Cu" "B.Cu") (net "a \"b\"")))"#)
            .unwrap();
        let via = node.child("via").unwrap();
        assert_eq!(via.point("at"), Some((1.5, -2.0)));
        assert_eq!(
            via.child("layers").unwrap().args().collect::<Vec<_>>(),
            ["F.Cu", "B.Cu"]
        );
        assert_eq!(via.value("net"), Some("a \"b\""));
    }

    #[test]
    fn hidden_handles_both_formats() {
        assert!(parse("(fp_text reference hide)").unwrap().hidden());
        assert!(parse("(property (hide yes))").unwrap().hidden());
        assert!(!parse("(property (hide no))").unwrap().hidden());
    }

    #[test]
    fn rejects_unbalanced() {
        assert!(parse("(a (b)").is_err());
        assert!(parse("(a))").is_err());
    }
}
