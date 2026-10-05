//! Net label models (port of `kicad_tools.schema.label`).

use std::fmt;

use crate::sexp::SExp;

use super::symbol::{find_at, find_string, get_string, new_uuid, num_list};

fn uuid_or_new(uuid: &str) -> String {
    if uuid.is_empty() {
        new_uuid()
    } else {
        uuid.to_string()
    }
}

fn effects(justify: &[&str]) -> SExp {
    SExp::list(
        "effects",
        [
            SExp::list("font", [num_list("size", &[1.27, 1.27])]),
            SExp::list("justify", justify.iter().map(|j| SExp::symbol(*j))),
        ],
    )
}

fn shape_of(sexp: &SExp) -> String {
    sexp.find("shape")
        .and_then(|s| get_string(s, 0))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "input".into())
}

fn shaped_label(
    tag: &str,
    text: &str,
    shape: &str,
    position: (f64, f64),
    rotation: f64,
    uuid: &str,
) -> SExp {
    let justify = if rotation == 180.0 { "right" } else { "left" };
    SExp::list(
        tag,
        [
            SExp::quoted(text),
            SExp::list("shape", [SExp::symbol(shape)]),
            num_list("at", &[position.0, position.1, rotation]),
            SExp::list("fields_autoplaced", [SExp::symbol("yes")]),
            effects(&[justify]),
            SExp::list("uuid", [SExp::quoted(uuid_or_new(uuid))]),
        ],
    )
}

/// A local net label.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Label {
    pub text: String,
    pub position: (f64, f64),
    pub rotation: f64,
    pub uuid: String,
}

impl Label {
    pub fn new(text: impl Into<String>, position: (f64, f64)) -> Self {
        Label {
            text: text.into(),
            position,
            rotation: 0.0,
            uuid: String::new(),
        }
    }

    /// `(label "NAME" (at X Y ROT) (fields_autoplaced yes) (effects ... (justify left bottom)) (uuid ...))`.
    pub fn to_sexp(&self) -> SExp {
        SExp::list(
            "label",
            [
                SExp::quoted(self.text.clone()),
                num_list("at", &[self.position.0, self.position.1, self.rotation]),
                SExp::list("fields_autoplaced", [SExp::symbol("yes")]),
                effects(&["left", "bottom"]),
                SExp::list("uuid", [SExp::quoted(uuid_or_new(&self.uuid))]),
            ],
        )
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (position, rotation) = find_at(sexp);
        Label {
            text: get_string(sexp, 0).unwrap_or_default(),
            position,
            rotation,
            uuid: find_string(sexp, "uuid"),
        }
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Label({:?}, {})",
            self.text,
            super::symbol::fmt_point(self.position)
        )
    }
}

/// A hierarchical label for connections between sheets.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct HierarchicalLabel {
    pub text: String,
    pub position: (f64, f64),
    pub rotation: f64,
    /// input, output, bidirectional, tri_state, passive
    pub shape: String,
    pub uuid: String,
}

impl HierarchicalLabel {
    pub fn new(text: impl Into<String>, position: (f64, f64)) -> Self {
        HierarchicalLabel {
            text: text.into(),
            position,
            rotation: 0.0,
            shape: "input".into(),
            uuid: String::new(),
        }
    }

    pub fn to_sexp(&self) -> SExp {
        shaped_label(
            "hierarchical_label",
            &self.text,
            &self.shape,
            self.position,
            self.rotation,
            &self.uuid,
        )
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (position, rotation) = find_at(sexp);
        HierarchicalLabel {
            text: get_string(sexp, 0).unwrap_or_default(),
            position,
            rotation,
            shape: shape_of(sexp),
            uuid: find_string(sexp, "uuid"),
        }
    }
}

impl fmt::Display for HierarchicalLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "HierarchicalLabel({:?}, shape={})",
            self.text, self.shape
        )
    }
}

/// A global label for project-wide connections.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct GlobalLabel {
    pub text: String,
    pub position: (f64, f64),
    pub rotation: f64,
    pub shape: String,
    pub uuid: String,
}

impl GlobalLabel {
    pub fn new(text: impl Into<String>, position: (f64, f64)) -> Self {
        GlobalLabel {
            text: text.into(),
            position,
            rotation: 0.0,
            shape: "input".into(),
            uuid: String::new(),
        }
    }

    pub fn to_sexp(&self) -> SExp {
        shaped_label(
            "global_label",
            &self.text,
            &self.shape,
            self.position,
            self.rotation,
            &self.uuid,
        )
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (position, rotation) = find_at(sexp);
        GlobalLabel {
            text: get_string(sexp, 0).unwrap_or_default(),
            position,
            rotation,
            shape: shape_of(sexp),
            uuid: find_string(sexp, "uuid"),
        }
    }
}

impl fmt::Display for GlobalLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GlobalLabel({:?})", self.text)
    }
}

/// A power symbol (GND, +5V, ...): a `power:` library symbol naming a net.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PowerSymbol {
    pub lib_id: String,
    pub position: (f64, f64),
    pub rotation: f64,
    pub uuid: String,
    /// The net name (GND, +5V, ...).
    pub value: String,
}

impl PowerSymbol {
    /// Parse a placed `(symbol ...)`; `None` unless its lib_id starts with `power:`.
    pub fn from_symbol_sexp(sexp: &SExp) -> Option<Self> {
        let lib_id = find_string(sexp, "lib_id");
        if !lib_id.starts_with("power:") {
            return None;
        }
        let (position, rotation) = find_at(sexp);
        let value = sexp
            .find_all("property")
            .find(|p| get_string(p, 0).as_deref() == Some("Value"))
            .and_then(|p| get_string(p, 1))
            .unwrap_or_default();
        Some(PowerSymbol {
            lib_id,
            position,
            rotation,
            uuid: find_string(sexp, "uuid"),
            value,
        })
    }
}
