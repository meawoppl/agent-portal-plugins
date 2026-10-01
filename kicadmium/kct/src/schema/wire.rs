//! Wire, junction, no-connect and bus models (port of `kicad_tools.schema.wire`).

use std::fmt;

use crate::sexp::SExp;

use super::symbol::{find_at, find_string, fmt_point, get_float, get_string, new_uuid, num_list};

fn two_points(sexp: &SExp) -> ((f64, f64), (f64, f64)) {
    if let Some(pts) = sexp.find("pts") {
        let xy: Vec<&SExp> = pts.find_all("xy").collect();
        if xy.len() >= 2 {
            let p = |n: &SExp| {
                (
                    get_float(n, 0).unwrap_or(0.0),
                    get_float(n, 1).unwrap_or(0.0),
                )
            };
            return (p(xy[0]), p(xy[1]));
        }
    }
    ((0.0, 0.0), (0.0, 0.0))
}

fn uuid_or_new(uuid: &str) -> String {
    if uuid.is_empty() {
        new_uuid()
    } else {
        uuid.to_string()
    }
}

/// A wire segment connecting two points.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Wire {
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub uuid: String,
    pub stroke_width: f64,
    pub stroke_type: String,
}

impl Wire {
    pub fn new(start: (f64, f64), end: (f64, f64)) -> Self {
        Wire {
            start,
            end,
            uuid: String::new(),
            stroke_width: 0.0,
            stroke_type: "default".into(),
        }
    }

    /// `(wire (pts (xy X1 Y1) (xy X2 Y2)) (stroke (width 0) (type default)) (uuid "..."))`.
    pub fn to_sexp(&self) -> SExp {
        let pts = SExp::list(
            "pts",
            [
                num_list("xy", &[self.start.0, self.start.1]),
                num_list("xy", &[self.end.0, self.end.1]),
            ],
        );
        let stroke = SExp::list(
            "stroke",
            [
                SExp::pair("width", self.stroke_width),
                SExp::list("type", [SExp::symbol(self.stroke_type.clone())]),
            ],
        );
        SExp::list(
            "wire",
            [
                pts,
                stroke,
                SExp::list("uuid", [SExp::quoted(uuid_or_new(&self.uuid))]),
            ],
        )
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (start, end) = two_points(sexp);
        let mut stroke_width = 0.0;
        let mut stroke_type = "default".to_string();
        if let Some(stroke) = sexp.find("stroke") {
            if let Some(w) = stroke.find("width") {
                stroke_width = get_float(w, 0).unwrap_or(0.0);
            }
            if let Some(t) = stroke.find("type") {
                stroke_type = get_string(t, 0)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "default".into());
            }
        }
        Wire {
            start,
            end,
            uuid: find_string(sexp, "uuid"),
            stroke_width,
            stroke_type,
        }
    }

    pub fn length(&self) -> f64 {
        let dx = self.end.0 - self.start.0;
        let dy = self.end.1 - self.start.1;
        (dx * dx + dy * dy).sqrt()
    }

    /// Whether `point` lies on this segment (default upstream tolerance 0.1).
    pub fn contains_point(&self, point: (f64, f64), tolerance: f64) -> bool {
        let (x, y) = point;
        let (x1, y1) = self.start;
        let (x2, y2) = self.end;
        if !(x1.min(x2) - tolerance <= x && x <= x1.max(x2) + tolerance) {
            return false;
        }
        if !(y1.min(y2) - tolerance <= y && y <= y1.max(y2) + tolerance) {
            return false;
        }
        let length = self.length();
        if length < tolerance {
            return ((x - x1).powi(2) + (y - y1).powi(2)).sqrt() < tolerance;
        }
        let dist = ((y2 - y1) * x - (x2 - x1) * y + x2 * y1 - y2 * x1).abs() / length;
        dist < tolerance
    }
}

impl fmt::Display for Wire {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Wire({} -> {})",
            fmt_point(self.start),
            fmt_point(self.end)
        )
    }
}

/// A junction point where multiple wires connect.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Junction {
    pub position: (f64, f64),
    pub uuid: String,
    pub diameter: f64,
}

impl Junction {
    pub fn new(position: (f64, f64)) -> Self {
        Junction {
            position,
            uuid: String::new(),
            diameter: 0.0,
        }
    }

    /// `(junction (at X Y) (diameter D) (color 0 0 0 0) (uuid "..."))`.
    pub fn to_sexp(&self) -> SExp {
        SExp::list(
            "junction",
            [
                num_list("at", &[self.position.0, self.position.1]),
                SExp::pair("diameter", self.diameter),
                SExp::list("color", (0..4).map(|_| SExp::atom(0i64))),
                SExp::list("uuid", [SExp::quoted(uuid_or_new(&self.uuid))]),
            ],
        )
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (position, _) = find_at(sexp);
        Junction {
            position,
            uuid: find_string(sexp, "uuid"),
            diameter: sexp
                .find("diameter")
                .and_then(|d| get_float(d, 0))
                .unwrap_or(0.0),
        }
    }
}

impl fmt::Display for Junction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Junction({})", fmt_point(self.position))
    }
}

/// A no-connect marker: `(no_connect (at X Y) (uuid "..."))`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NoConnect {
    pub position: (f64, f64),
    pub uuid: String,
}

impl NoConnect {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let (position, _) = find_at(sexp);
        NoConnect {
            position,
            uuid: find_string(sexp, "uuid"),
        }
    }
}

impl fmt::Display for NoConnect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NoConnect({})", fmt_point(self.position))
    }
}

/// A bus segment (multiple signals grouped together).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Bus {
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub uuid: String,
}

impl Bus {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let (start, end) = two_points(sexp);
        Bus {
            start,
            end,
            uuid: find_string(sexp, "uuid"),
        }
    }
}
