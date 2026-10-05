//! Symbol library models (port of `kicad_tools.schema.library`).
//!
//! `.kicad_sym` libraries, library symbols with pin geometry and body
//! graphics, `extends` resolution, and a [`LibraryManager`] resolving
//! `lib_id`s via loaded libraries, `sym-lib-table` entries (kct addition,
//! mirroring `kicad_tools.footprints.fp_lib_table`), and search paths.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};

use crate::sexp::{self, SExp};

use super::symbol::{find_at, get_float, get_string, num_list, OrderedMap};

/// `kicad_tools.core.version.KICAD_SYM_FORMAT_VERSION`.
pub const KICAD_SYM_FORMAT_VERSION: i64 = 20231120;
/// `kicad_tools.core.version.KICAD_GENERATOR_VERSION`.
pub const KICAD_GENERATOR_VERSION: &str = "10.0";

/// Valid KiCad pin electrical types.
pub const VALID_PIN_TYPES: &[&str] = &[
    "input",
    "output",
    "bidirectional",
    "power_in",
    "power_out",
    "passive",
    "unspecified",
    "tri_state",
    "open_collector",
    "open_emitter",
    "no_connect",
];

/// Valid fill types for graphical shapes.
pub const VALID_FILL_TYPES: &[&str] = &["none", "outline", "background"];

/// Valid stroke types for graphical shapes.
pub const VALID_STROKE_TYPES: &[&str] = &[
    "default",
    "dash",
    "dot",
    "dash_dot",
    "dash_dot_dot",
    "solid",
];

/// Standard KiCad schematic grid (mm).
pub const KICAD_GRID: f64 = 1.27;

fn sorted_list(values: &[&str]) -> String {
    let sorted: BTreeSet<&str> = values.iter().copied().collect();
    let quoted: Vec<String> = sorted.iter().map(|v| format!("'{v}'")).collect();
    format!("[{}]", quoted.join(", "))
}

fn validate_fill_type(fill_type: &str) -> Result<()> {
    if !VALID_FILL_TYPES.contains(&fill_type) {
        bail!(
            "Invalid fill_type '{fill_type}'. Must be one of: {}",
            sorted_list(VALID_FILL_TYPES)
        );
    }
    Ok(())
}

fn validate_stroke_type(stroke_type: &str) -> Result<()> {
    if !VALID_STROKE_TYPES.contains(&stroke_type) {
        bail!(
            "Invalid stroke_type '{stroke_type}'. Must be one of: {}",
            sorted_list(VALID_STROKE_TYPES)
        );
    }
    Ok(())
}

fn stroke_sexp(width: f64, stroke_type: &str) -> SExp {
    SExp::list(
        "stroke",
        [
            SExp::pair("width", width),
            SExp::list("type", [SExp::symbol(stroke_type)]),
        ],
    )
}

fn fill_sexp(fill_type: &str) -> SExp {
    SExp::list("fill", [SExp::list("type", [SExp::symbol(fill_type)])])
}

/// `(stroke (width W) (type T))` and `(fill (type F))` of a shape node.
fn parse_style(sexp: &SExp) -> (f64, String, String) {
    let mut width = 0.0;
    let mut stroke_type = "default".to_string();
    if let Some(stroke) = sexp.find("stroke") {
        if let Some(w) = stroke.find("width") {
            width = get_float(w, 0).unwrap_or(0.0);
        }
        if let Some(t) = stroke.find("type") {
            stroke_type = get_string(t, 0)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "default".into());
        }
    }
    let mut fill_type = "none".to_string();
    if let Some(t) = sexp.find("fill").and_then(|f| f.find("type")) {
        fill_type = get_string(t, 0)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "none".into());
    }
    (width, stroke_type, fill_type)
}

fn point_of(sexp: &SExp, name: &str) -> (f64, f64) {
    sexp.find(name)
        .map(|n| {
            (
                get_float(n, 0).unwrap_or(0.0),
                get_float(n, 1).unwrap_or(0.0),
            )
        })
        .unwrap_or((0.0, 0.0))
}

fn short_name(name: &str) -> &str {
    name.split_once(':').map_or(name, |(_, s)| s)
}

/// A polyline graphical element (closed polygons repeat the first point).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SymbolPolyline {
    pub points: Vec<(f64, f64)>,
    pub stroke_width: f64,
    pub stroke_type: String,
    pub fill_type: String,
}

impl SymbolPolyline {
    /// Validated constructor (upstream `__post_init__`).
    pub fn new(
        points: Vec<(f64, f64)>,
        stroke_width: f64,
        stroke_type: &str,
        fill_type: &str,
    ) -> Result<Self> {
        if points.len() < 2 {
            bail!("A polyline requires at least 2 points");
        }
        validate_fill_type(fill_type)?;
        validate_stroke_type(stroke_type)?;
        Ok(SymbolPolyline {
            points,
            stroke_width,
            stroke_type: stroke_type.into(),
            fill_type: fill_type.into(),
        })
    }

    pub fn to_sexp_node(&self) -> SExp {
        let pts = SExp::list(
            "pts",
            self.points.iter().map(|&(x, y)| num_list("xy", &[x, y])),
        );
        SExp::list(
            "polyline",
            [
                pts,
                stroke_sexp(self.stroke_width, &self.stroke_type),
                fill_sexp(&self.fill_type),
            ],
        )
    }

    /// Parse `(polyline ...)`. Unlike upstream, file contents are not
    /// validated (a degenerate shape in a library does not abort loading).
    pub fn from_sexp(sexp: &SExp) -> Self {
        let points = sexp
            .find("pts")
            .map(|pts| {
                pts.find_all("xy")
                    .map(|xy| {
                        (
                            get_float(xy, 0).unwrap_or(0.0),
                            get_float(xy, 1).unwrap_or(0.0),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (stroke_width, stroke_type, fill_type) = parse_style(sexp);
        SymbolPolyline {
            points,
            stroke_width,
            stroke_type,
            fill_type,
        }
    }
}

/// A circle graphical element.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SymbolCircle {
    pub center: (f64, f64),
    pub radius: f64,
    pub stroke_width: f64,
    pub stroke_type: String,
    pub fill_type: String,
}

impl SymbolCircle {
    pub fn new(
        center: (f64, f64),
        radius: f64,
        stroke_width: f64,
        stroke_type: &str,
        fill_type: &str,
    ) -> Result<Self> {
        if radius <= 0.0 {
            bail!("Circle radius must be positive");
        }
        validate_fill_type(fill_type)?;
        validate_stroke_type(stroke_type)?;
        Ok(SymbolCircle {
            center,
            radius,
            stroke_width,
            stroke_type: stroke_type.into(),
            fill_type: fill_type.into(),
        })
    }

    pub fn to_sexp_node(&self) -> SExp {
        SExp::list(
            "circle",
            [
                num_list("center", &[self.center.0, self.center.1]),
                SExp::pair("radius", self.radius),
                stroke_sexp(self.stroke_width, &self.stroke_type),
                fill_sexp(&self.fill_type),
            ],
        )
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (stroke_width, stroke_type, fill_type) = parse_style(sexp);
        SymbolCircle {
            center: point_of(sexp, "center"),
            radius: sexp
                .find("radius")
                .and_then(|r| get_float(r, 0))
                .unwrap_or(0.0),
            stroke_width,
            stroke_type,
            fill_type,
        }
    }
}

/// An arc defined by start, mid (on the arc), and end points.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SymbolArc {
    pub start: (f64, f64),
    pub mid: (f64, f64),
    pub end: (f64, f64),
    pub stroke_width: f64,
    pub stroke_type: String,
    pub fill_type: String,
}

impl SymbolArc {
    pub fn new(
        start: (f64, f64),
        mid: (f64, f64),
        end: (f64, f64),
        stroke_width: f64,
        stroke_type: &str,
        fill_type: &str,
    ) -> Result<Self> {
        validate_fill_type(fill_type)?;
        validate_stroke_type(stroke_type)?;
        Ok(SymbolArc {
            start,
            mid,
            end,
            stroke_width,
            stroke_type: stroke_type.into(),
            fill_type: fill_type.into(),
        })
    }

    pub fn to_sexp_node(&self) -> SExp {
        SExp::list(
            "arc",
            [
                num_list("start", &[self.start.0, self.start.1]),
                num_list("mid", &[self.mid.0, self.mid.1]),
                num_list("end", &[self.end.0, self.end.1]),
                stroke_sexp(self.stroke_width, &self.stroke_type),
                fill_sexp(&self.fill_type),
            ],
        )
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (stroke_width, stroke_type, fill_type) = parse_style(sexp);
        SymbolArc {
            start: point_of(sexp, "start"),
            mid: point_of(sexp, "mid"),
            end: point_of(sexp, "end"),
            stroke_width,
            stroke_type,
            fill_type,
        }
    }
}

/// A rectangle graphical element.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SymbolRectangle {
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub stroke_width: f64,
    pub stroke_type: String,
    pub fill_type: String,
}

impl SymbolRectangle {
    pub fn new(
        start: (f64, f64),
        end: (f64, f64),
        stroke_width: f64,
        stroke_type: &str,
        fill_type: &str,
    ) -> Result<Self> {
        validate_fill_type(fill_type)?;
        validate_stroke_type(stroke_type)?;
        Ok(SymbolRectangle {
            start,
            end,
            stroke_width,
            stroke_type: stroke_type.into(),
            fill_type: fill_type.into(),
        })
    }

    pub fn to_sexp_node(&self) -> SExp {
        SExp::list(
            "rectangle",
            [
                num_list("start", &[self.start.0, self.start.1]),
                num_list("end", &[self.end.0, self.end.1]),
                stroke_sexp(self.stroke_width, &self.stroke_type),
                fill_sexp(&self.fill_type),
            ],
        )
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let (stroke_width, stroke_type, fill_type) = parse_style(sexp);
        SymbolRectangle {
            start: point_of(sexp, "start"),
            end: point_of(sexp, "end"),
            stroke_width,
            stroke_type,
            fill_type,
        }
    }
}

/// Union of the graphical shape types (upstream `SymbolGraphic`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SymbolGraphic {
    Polyline(SymbolPolyline),
    Circle(SymbolCircle),
    Arc(SymbolArc),
    Rectangle(SymbolRectangle),
}

impl SymbolGraphic {
    pub fn to_sexp_node(&self) -> SExp {
        match self {
            SymbolGraphic::Polyline(g) => g.to_sexp_node(),
            SymbolGraphic::Circle(g) => g.to_sexp_node(),
            SymbolGraphic::Arc(g) => g.to_sexp_node(),
            SymbolGraphic::Rectangle(g) => g.to_sexp_node(),
        }
    }

    /// Upstream class name (`SymbolPolyline`, ...).
    pub fn type_name(&self) -> &'static str {
        match self {
            SymbolGraphic::Polyline(_) => "SymbolPolyline",
            SymbolGraphic::Circle(_) => "SymbolCircle",
            SymbolGraphic::Arc(_) => "SymbolArc",
            SymbolGraphic::Rectangle(_) => "SymbolRectangle",
        }
    }
}

/// A pin definition in a symbol library (library coordinates, Y-up).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LibraryPin {
    pub number: String,
    pub name: String,
    /// Electrical type: power_in, passive, input, output, ... (upstream `type`).
    #[serde(rename = "type")]
    pub pin_type: String,
    /// Position relative to symbol origin (the connection point).
    pub position: (f64, f64),
    /// Degrees: 0=right, 90=up, 180=left, 270=down.
    pub rotation: f64,
    pub length: f64,
    /// Unit number for multi-unit symbols (1-indexed).
    pub unit: i64,
    /// Pin shape: line, inverted, clock, ...
    pub shape: String,
}

impl LibraryPin {
    pub fn new(
        number: impl Into<String>,
        name: impl Into<String>,
        pin_type: impl Into<String>,
        position: (f64, f64),
        rotation: f64,
        length: f64,
    ) -> Self {
        LibraryPin {
            number: number.into(),
            name: name.into(),
            pin_type: pin_type.into(),
            position,
            rotation,
            length,
            unit: 1,
            shape: "line".into(),
        }
    }

    /// Offset from `(at)` to the wire connection point: always `(0, 0)`
    /// because the pin position already is the connection point.
    pub fn connection_offset(&self) -> (f64, f64) {
        (0.0, 0.0)
    }

    /// Parse `(pin <type> <shape> (at ...) (length ...) (name ...) (number ...))`.
    pub fn from_sexp(sexp: &SExp, unit: i64) -> Self {
        let (position, rotation) = find_at(sexp);
        let length = sexp
            .find("length")
            .and_then(|l| get_float(l, 0))
            .filter(|&l| l != 0.0)
            .unwrap_or(2.54);
        LibraryPin {
            number: sexp
                .find("number")
                .and_then(|n| get_string(n, 0))
                .unwrap_or_default(),
            name: sexp
                .find("name")
                .and_then(|n| get_string(n, 0))
                .unwrap_or_default(),
            pin_type: get_string(sexp, 0)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "passive".into()),
            position,
            rotation,
            length,
            unit,
            shape: get_string(sexp, 1)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "line".into()),
        }
    }

    /// Pin name and number are always quoted (KiCad strict string fields).
    pub fn to_sexp_node(&self) -> SExp {
        let font_effects = SExp::list(
            "effects",
            [SExp::list("font", [num_list("size", &[1.27, 1.27])])],
        );
        SExp::list(
            "pin",
            [
                SExp::symbol(self.pin_type.clone()),
                SExp::symbol(self.shape.clone()),
                num_list("at", &[self.position.0, self.position.1, self.rotation]),
                SExp::pair("length", self.length),
                SExp::list(
                    "name",
                    [SExp::quoted(self.name.clone()), font_effects.clone()],
                ),
                SExp::list("number", [SExp::quoted(self.number.clone()), font_effects]),
            ],
        )
    }
}

/// Python `round()` (half to even).
pub(crate) fn py_round(v: f64) -> f64 {
    v.round_ties_even()
}

/// Snap `value` to the nearest multiple of `grid` if within `tolerance`.
pub fn snap_to_kicad_grid(value: f64, grid: f64, tolerance: f64) -> f64 {
    let nearest = py_round(value / grid) * grid;
    if (value - nearest).abs() <= tolerance {
        nearest
    } else {
        value
    }
}

/// Parse the unit index from a `{short_name}_{unit}_{variant}` sub-symbol
/// name (0 for the graphics sub-symbol). `None` if the shape doesn't match.
pub fn parse_unit_index(unit_name: &str, parent_name: &str) -> Option<i64> {
    let mut parts = unit_name.rsplitn(3, '_');
    let variant = parts.next()?;
    let unit = parts.next()?;
    let prefix = parts.next()?;
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if !(digits(unit) && digits(variant)) || prefix != parent_name {
        return None;
    }
    unit.parse().ok()
}

/// Mirror, rotate (library Y-up), then negate Y: the library-to-sheet
/// offset transform shared with `field_geometry`.
pub(crate) fn transform_offset(mut x: f64, mut y: f64, rotation: f64, mirror: &str) -> (f64, f64) {
    if mirror == "x" {
        x = -x;
    } else if mirror == "y" {
        y = -y;
    }
    if rotation != 0.0 {
        let a = rotation.to_radians();
        let (sin_a, cos_a) = (a.sin(), a.cos());
        (x, y) = (x * cos_a - y * sin_a, x * sin_a + y * cos_a);
    }
    (x, -y)
}

/// A symbol definition from a KiCad symbol library.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LibrarySymbol {
    pub name: String,
    pub properties: OrderedMap<String>,
    pub pins: Vec<LibraryPin>,
    pub graphics: Vec<SymbolGraphic>,
    pub units: i64,
    pub extends: Option<String>,
}

impl LibrarySymbol {
    pub fn new(name: impl Into<String>) -> Self {
        LibrarySymbol {
            name: name.into(),
            properties: OrderedMap::new(),
            pins: Vec::new(),
            graphics: Vec::new(),
            units: 1,
            extends: None,
        }
    }

    pub fn pin_count(&self) -> usize {
        self.pins.len()
    }

    /// First pin with this number.
    pub fn get_pin(&self, number: &str) -> Option<&LibraryPin> {
        self.pins.iter().find(|p| p.number == number)
    }

    pub fn get_pins_by_name(&self, name: &str) -> Vec<&LibraryPin> {
        self.pins.iter().filter(|p| p.name == name).collect()
    }

    /// Schematic position of a pin (grid-snapped offsets), `None` if absent.
    pub fn get_pin_position(
        &self,
        pin_number: &str,
        instance_pos: (f64, f64),
        instance_rot: f64,
        mirror: &str,
    ) -> Option<(f64, f64)> {
        self.get_pin_position_with(pin_number, instance_pos, instance_rot, mirror, true)
    }

    /// [`get_pin_position`](Self::get_pin_position) with explicit `snap_to_grid`.
    ///
    /// Mirror and rotation are applied in library coordinates (Y-up), then Y
    /// is negated for the sheet (Y-down); with `snap_to_grid` each offset is
    /// snapped to 1.27 mm when within 0.2 mm (removing trig drift).
    pub fn get_pin_position_with(
        &self,
        pin_number: &str,
        instance_pos: (f64, f64),
        instance_rot: f64,
        mirror: &str,
        snap_to_grid: bool,
    ) -> Option<(f64, f64)> {
        let pin = self.get_pin(pin_number)?;
        let (mut x, mut y) = transform_offset(pin.position.0, pin.position.1, instance_rot, mirror);
        if snap_to_grid {
            x = snap_to_kicad_grid(x, KICAD_GRID, 0.2);
            y = snap_to_kicad_grid(y, KICAD_GRID, 0.2);
        }
        Some((instance_pos.0 + x, instance_pos.1 + y))
    }

    /// Pin number -> schematic position for every pin.
    pub fn get_all_pin_positions(
        &self,
        instance_pos: (f64, f64),
        instance_rot: f64,
        mirror: &str,
    ) -> OrderedMap<(f64, f64)> {
        self.get_all_pin_positions_with(instance_pos, instance_rot, mirror, true)
    }

    pub fn get_all_pin_positions_with(
        &self,
        instance_pos: (f64, f64),
        instance_rot: f64,
        mirror: &str,
        snap_to_grid: bool,
    ) -> OrderedMap<(f64, f64)> {
        let mut positions = OrderedMap::new();
        for pin in &self.pins {
            if let Some(pos) = self.get_pin_position_with(
                &pin.number,
                instance_pos,
                instance_rot,
                mirror,
                snap_to_grid,
            ) {
                positions.insert(pin.number.clone(), pos);
            }
        }
        positions
    }

    /// Parse a library `(symbol "Name" ...)` including unit sub-symbols.
    pub fn from_sexp(sexp: &SExp) -> Self {
        let name = get_string(sexp, 0).unwrap_or_default();
        let extends = sexp
            .find("extends")
            .and_then(|e| get_string(e, 0))
            .filter(|s| !s.is_empty());

        let mut properties = OrderedMap::new();
        for prop in sexp.find_all("property") {
            if let Some(key) = get_string(prop, 0).filter(|k| !k.is_empty()) {
                properties.insert(key, get_string(prop, 1).unwrap_or_default());
            }
        }

        let short = short_name(&name).to_string();
        let mut pins = Vec::new();
        let mut graphics = Vec::new();
        let mut max_unit = 1;
        for unit_sym in sexp.children_named("symbol") {
            let unit_name = get_string(unit_sym, 0).unwrap_or_default();
            let parsed = parse_unit_index(&unit_name, &short);
            let pin_unit = parsed.filter(|&u| u >= 1).unwrap_or(1);
            for pin in unit_sym.find_all("pin") {
                pins.push(LibraryPin::from_sexp(pin, pin_unit));
                max_unit = max_unit.max(pin_unit);
            }
            if let Some(u) = parsed.filter(|&u| u >= 1) {
                max_unit = max_unit.max(u);
            }
            graphics.extend(
                unit_sym
                    .find_all("polyline")
                    .map(|g| SymbolGraphic::Polyline(SymbolPolyline::from_sexp(g))),
            );
            graphics.extend(
                unit_sym
                    .find_all("circle")
                    .map(|g| SymbolGraphic::Circle(SymbolCircle::from_sexp(g))),
            );
            graphics.extend(
                unit_sym
                    .find_all("arc")
                    .map(|g| SymbolGraphic::Arc(SymbolArc::from_sexp(g))),
            );
            graphics.extend(
                unit_sym
                    .find_all("rectangle")
                    .map(|g| SymbolGraphic::Rectangle(SymbolRectangle::from_sexp(g))),
            );
        }

        LibrarySymbol {
            name,
            properties,
            pins,
            graphics,
            units: max_unit,
            extends,
        }
    }

    /// Add a pin; `pin_type` must be one of [`VALID_PIN_TYPES`].
    #[allow(clippy::too_many_arguments)]
    pub fn add_pin(
        &mut self,
        number: &str,
        name: &str,
        pin_type: &str,
        position: (f64, f64),
        rotation: f64,
        length: f64,
        unit: i64,
        shape: &str,
    ) -> Result<&mut LibraryPin> {
        if !VALID_PIN_TYPES.contains(&pin_type) {
            bail!(
                "Invalid pin type '{pin_type}'. Must be one of: {}",
                sorted_list(VALID_PIN_TYPES)
            );
        }
        self.pins.push(LibraryPin {
            number: number.into(),
            name: name.into(),
            pin_type: pin_type.into(),
            position,
            rotation,
            length,
            unit,
            shape: shape.into(),
        });
        Ok(self.pins.last_mut().expect("just pushed"))
    }

    /// [`add_pin`](Self::add_pin) with upstream defaults (length 2.54, unit 1, line).
    pub fn add_simple_pin(
        &mut self,
        number: &str,
        name: &str,
        pin_type: &str,
        position: (f64, f64),
        rotation: f64,
    ) -> Result<&mut LibraryPin> {
        self.add_pin(number, name, pin_type, position, rotation, 2.54, 1, "line")
    }

    pub fn add_property(&mut self, name: &str, value: &str) {
        self.properties.insert(name, value.to_string());
    }

    pub fn set_property(&mut self, name: &str, value: &str) {
        self.properties.insert(name, value.to_string());
    }

    fn push_graphic(&mut self, g: SymbolGraphic) -> &SymbolGraphic {
        self.graphics.push(g);
        self.graphics.last().expect("just pushed")
    }

    /// Add an open polyline (stroke width 0 = KiCad default).
    pub fn add_polyline(
        &mut self,
        points: Vec<(f64, f64)>,
        stroke_width: f64,
        stroke_type: &str,
        fill_type: &str,
    ) -> Result<&SymbolGraphic> {
        let shape = SymbolPolyline::new(points, stroke_width, stroke_type, fill_type)?;
        Ok(self.push_graphic(SymbolGraphic::Polyline(shape)))
    }

    /// Add a closed polygon, auto-closing it (upstream default fill `outline`).
    pub fn add_polygon(
        &mut self,
        points: Vec<(f64, f64)>,
        stroke_width: f64,
        stroke_type: &str,
        fill_type: &str,
    ) -> Result<&SymbolGraphic> {
        if points.len() < 3 {
            bail!("A polygon requires at least 3 points");
        }
        let mut pts = points;
        if pts.first() != pts.last() {
            pts.push(pts[0]);
        }
        self.add_polyline(pts, stroke_width, stroke_type, fill_type)
    }

    pub fn add_circle(
        &mut self,
        center: (f64, f64),
        radius: f64,
        stroke_width: f64,
        stroke_type: &str,
        fill_type: &str,
    ) -> Result<&SymbolGraphic> {
        let shape = SymbolCircle::new(center, radius, stroke_width, stroke_type, fill_type)?;
        Ok(self.push_graphic(SymbolGraphic::Circle(shape)))
    }

    pub fn add_arc(
        &mut self,
        start: (f64, f64),
        mid: (f64, f64),
        end: (f64, f64),
        stroke_width: f64,
        stroke_type: &str,
        fill_type: &str,
    ) -> Result<&SymbolGraphic> {
        let shape = SymbolArc::new(start, mid, end, stroke_width, stroke_type, fill_type)?;
        Ok(self.push_graphic(SymbolGraphic::Arc(shape)))
    }

    pub fn add_rectangle(
        &mut self,
        start: (f64, f64),
        end: (f64, f64),
        stroke_width: f64,
        stroke_type: &str,
        fill_type: &str,
    ) -> Result<&SymbolGraphic> {
        let shape = SymbolRectangle::new(start, end, stroke_width, stroke_type, fill_type)?;
        Ok(self.push_graphic(SymbolGraphic::Rectangle(shape)))
    }

    pub fn get_pins_for_unit(&self, unit: i64) -> Vec<&LibraryPin> {
        self.pins.iter().filter(|p| p.unit == unit).collect()
    }

    /// Serialize as a library symbol: properties, a `_0_1` graphics
    /// sub-symbol (if any), and `_N_1` pin sub-symbols per unit. Derived
    /// (`extends`) symbols emit no sub-symbols.
    pub fn to_sexp_node(&self) -> SExp {
        let mut sym = SExp::list("symbol", [SExp::quoted(self.name.clone())]);
        if let Some(base) = self.extends.as_deref().filter(|e| !e.is_empty()) {
            sym.push(SExp::list("extends", [SExp::quoted(base)]));
        }
        let short = short_name(&self.name);
        let mut y = 0.0;
        for (key, value) in self.properties.iter() {
            let mut effects = SExp::list(
                "effects",
                [SExp::list("font", [num_list("size", &[1.27, 1.27])])],
            );
            if key != "Reference" && key != "Value" {
                effects.push(SExp::list("hide", [SExp::symbol("yes")]));
            }
            sym.push(SExp::list(
                "property",
                [
                    SExp::quoted(key),
                    SExp::quoted(value.clone()),
                    num_list("at", &[0.0, y, 0.0]),
                    effects,
                ],
            ));
            y += 2.54;
        }
        if self.extends.as_deref().is_none_or(str::is_empty) {
            if !self.graphics.is_empty() {
                let mut gfx = SExp::list("symbol", [SExp::quoted(format!("{short}_0_1"))]);
                for g in &self.graphics {
                    gfx.push(g.to_sexp_node());
                }
                sym.push(gfx);
            }
            for unit in 1..=self.units {
                let mut u = SExp::list("symbol", [SExp::quoted(format!("{short}_{unit}_1"))]);
                for pin in self.pins.iter().filter(|p| p.unit == unit) {
                    u.push(pin.to_sexp_node());
                }
                sym.push(u);
            }
        }
        sym
    }
}

/// Resolve `extends` chains in place: derived symbols without own pins get
/// their base's pins (and graphics, if they have none). Base names may match
/// a key directly or the short part of a qualified `lib:name` key.
pub fn resolve_extends(symbols: &mut OrderedMap<LibrarySymbol>) -> Result<()> {
    resolve_extends_depth(symbols, 10)
}

pub fn resolve_extends_depth(
    symbols: &mut OrderedMap<LibrarySymbol>,
    max_depth: usize,
) -> Result<()> {
    fn find_base<'a>(
        symbols: &'a OrderedMap<LibrarySymbol>,
        name: &str,
    ) -> Option<&'a LibrarySymbol> {
        symbols.get(name).or_else(|| {
            symbols
                .iter()
                .find(|(k, _)| short_name(k) == name)
                .map(|(_, s)| s)
        })
    }

    for i in 0..symbols.len() {
        let (_, sym) = symbols.get_index(i).expect("in range");
        let Some(first) = sym.extends.clone() else {
            continue;
        };
        if !sym.pins.is_empty() {
            continue;
        }
        let mut visited: HashSet<String> = HashSet::from([sym.name.clone()]);
        let mut current = Some(first);
        let mut depth = 0;
        let mut resolved = None;
        while let Some(name) = current.take() {
            if depth >= max_depth {
                break;
            }
            if visited.contains(&name) {
                bail!(
                    "Circular extends chain detected: {} -> ... -> {name}",
                    sym.name
                );
            }
            visited.insert(name.clone());
            depth += 1;
            let Some(base) = find_base(symbols, &name) else {
                break;
            };
            if !base.pins.is_empty() {
                resolved = Some((base.pins.clone(), base.graphics.clone()));
                break;
            }
            current = base.extends.clone();
        }
        if let Some((pins, graphics)) = resolved {
            let (_, sym) = symbols.get_index_mut(i).expect("in range");
            sym.pins = pins;
            if sym.graphics.is_empty() {
                sym.graphics = graphics;
            }
        }
    }
    Ok(())
}

/// A KiCad symbol library (`.kicad_sym`).
#[derive(Debug, Clone)]
pub struct SymbolLibrary {
    pub path: String,
    pub symbols: OrderedMap<LibrarySymbol>,
    pub version: String,
    pub generator: String,
    /// Parsed source tree; when present, `save` writes it back verbatim
    /// (upstream `_sexp`). Set to `None` to regenerate from the model.
    pub sexp: Option<SExp>,
}

impl SymbolLibrary {
    /// An in-memory library (upstream dataclass constructor).
    pub fn new(path: impl Into<String>, symbols: OrderedMap<LibrarySymbol>) -> Self {
        SymbolLibrary {
            path: path.into(),
            symbols,
            version: String::new(),
            generator: "kicad_tools".into(),
            sexp: None,
        }
    }

    pub fn get_symbol(&self, name: &str) -> Option<&LibrarySymbol> {
        self.symbols.get(name)
    }

    pub fn get_symbol_mut(&mut self, name: &str) -> Option<&mut LibrarySymbol> {
        self.symbols.get_mut(name)
    }

    /// Walk the `extends` chain to the root base symbol.
    pub fn resolve_base<'a>(&'a self, symbol: &'a LibrarySymbol) -> Result<&'a LibrarySymbol> {
        let mut visited = HashSet::new();
        let mut current = symbol;
        while let Some(base_name) = current.extends.as_deref() {
            if !visited.insert(current.name.clone()) {
                bail!("Circular extends chain detected at '{}'", current.name);
            }
            current = self.symbols.get(base_name).ok_or_else(|| {
                anyhow!(
                    "Base symbol '{base_name}' (extended by '{}') not found in library '{}'",
                    current.name,
                    self.path
                )
            })?;
        }
        Ok(current)
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// Save to `path` (or the original path).
    pub fn save(&self, path: Option<&str>) -> Result<()> {
        let save_path = path.filter(|p| !p.is_empty()).unwrap_or(&self.path);
        if save_path.is_empty() {
            bail!("No path specified for save");
        }
        let mut text = self.to_sexp().to_kicad_string_preserving();
        text.push('\n');
        crate::fsutil::atomic_write(Path::new(save_path), text.as_bytes())
    }

    fn to_sexp(&self) -> SExp {
        match &self.sexp {
            Some(sexp) => sexp.clone(),
            None => self.to_sexp_node(),
        }
    }

    /// New empty library; `version` defaults to today's date (YYYYMMDD, UTC).
    pub fn create(path: impl Into<String>, version: Option<&str>) -> Self {
        SymbolLibrary {
            path: path.into(),
            symbols: OrderedMap::new(),
            version: version.map_or_else(today_yyyymmdd, str::to_string),
            generator: "kicad_tools".into(),
            sexp: None,
        }
    }

    fn from_root(root: SExp, path: String, resolve: bool, err: &str) -> Result<Self> {
        if !root.has_tag("kicad_symbol_lib") {
            bail!("{err}");
        }
        let version = root
            .find("version")
            .and_then(|v| get_string(v, 0))
            .unwrap_or_default();
        let generator = root
            .find("generator")
            .and_then(|v| get_string(v, 0))
            .unwrap_or_default();
        let mut symbols = OrderedMap::new();
        for sym in root.children_named("symbol") {
            let sym = LibrarySymbol::from_sexp(sym);
            symbols.insert(sym.name.clone(), sym);
        }
        if resolve {
            resolve_extends(&mut symbols)?;
        }
        Ok(SymbolLibrary {
            path,
            symbols,
            version,
            generator,
            sexp: Some(root),
        })
    }

    /// Load a `.kicad_sym` file (extends chains resolved).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let root = sexp::parse_file(path)?;
        let display = path.to_string_lossy().into_owned();
        Self::from_root(
            root,
            display.clone(),
            true,
            &format!("Not a KiCad symbol library: {display}"),
        )
    }

    /// Load from text (path `<string>`; extends not resolved, as upstream).
    pub fn load_from_string(text: &str) -> Result<Self> {
        let root = sexp::parse(text)?;
        Self::from_root(
            root,
            "<string>".into(),
            false,
            "Not a KiCad symbol library string",
        )
    }

    /// Create and register a new symbol.
    pub fn create_symbol(&mut self, name: &str, units: i64) -> Result<&mut LibrarySymbol> {
        if self.symbols.contains_key(name) {
            bail!("Symbol '{name}' already exists in library");
        }
        let mut sym = LibrarySymbol::new(name);
        sym.units = units;
        self.symbols.insert(name, sym);
        Ok(self.symbols.get_mut(name).expect("just inserted"))
    }

    /// `(kicad_symbol_lib (version ..) (generator "kicad_tools") (generator_version "10.0") symbols...)`.
    pub fn to_sexp_node(&self) -> SExp {
        let mut root = SExp::list(
            "kicad_symbol_lib",
            [
                SExp::pair("version", KICAD_SYM_FORMAT_VERSION),
                SExp::list("generator", [SExp::quoted("kicad_tools")]),
                SExp::list("generator_version", [SExp::quoted(KICAD_GENERATOR_VERSION)]),
            ],
        );
        for sym in self.symbols.values() {
            root.push(sym.to_sexp_node());
        }
        root
    }
}

fn today_yyyymmdd() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Civil-from-days (Howard Hinnant).
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}")
}

// ------------------------------------------------------------ sym-lib-table

/// One `(lib ...)` row of a `sym-lib-table`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SymLibEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub lib_type: String,
    pub uri: String,
    /// Expanded path for `type "KiCad"` rows (not checked for existence).
    pub resolved_path: Option<PathBuf>,
}

/// Environment variable lookup used by [`expand_kicad_vars`].
pub type EnvLookup = dyn Fn(&str) -> Option<String>;

/// Expand `${VAR}` references (`KIPRJMOD` = `kiprjmod`, others from `env`
/// or the process environment). `None` if any variable is unknown.
pub fn expand_kicad_vars(
    uri: &str,
    kiprjmod: Option<&Path>,
    env: Option<&EnvLookup>,
) -> Option<PathBuf> {
    let uri = uri.strip_prefix("file://").unwrap_or(uri);
    let mut out = String::new();
    let mut rest = uri;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            break;
        };
        let var = &after[..end];
        let valid = var
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        out.push_str(&rest[..start]);
        if !valid {
            out.push_str(&rest[start..start + 2 + end + 1]);
        } else if var == "KIPRJMOD" {
            out.push_str(&kiprjmod?.to_string_lossy());
        } else {
            let value = match env {
                Some(lookup) => lookup(var),
                None => std::env::var(var).ok(),
            };
            out.push_str(&value?);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(PathBuf::from(out))
}

/// Parse a `sym-lib-table` (missing/malformed -> empty). `KIPRJMOD` is the
/// table's directory.
pub fn parse_sym_lib_table(path: &Path) -> Vec<SymLibEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(root) = sexp::parse(&text) else {
        return Vec::new();
    };
    let kiprjmod = path
        .parent()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
    let mut entries = Vec::new();
    for lib in root.find_all("lib") {
        let (mut name, mut lib_type, mut uri) = (String::new(), String::new(), String::new());
        for field in &lib.children {
            let value = || get_string(field, 0).unwrap_or_default();
            match field.tag() {
                Some("name") => name = value(),
                Some("type") => lib_type = value(),
                Some("uri") => uri = value(),
                _ => {}
            }
        }
        if name.is_empty() || uri.is_empty() {
            continue;
        }
        let resolved_path = if lib_type == "KiCad" {
            expand_kicad_vars(&uri, kiprjmod.as_deref(), None)
        } else {
            None
        };
        entries.push(SymLibEntry {
            name,
            lib_type,
            uri,
            resolved_path,
        });
    }
    entries
}

/// Locate the project `sym-lib-table` for a schematic/project path, walking
/// up until a directory holding a `.kicad_pro` (the project root).
pub fn find_project_sym_lib_table(start: &Path) -> Option<PathBuf> {
    let start = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    let mut current = start.canonicalize().ok()?;
    loop {
        let table = current.join("sym-lib-table");
        let has_pro = std::fs::read_dir(&current).ok().is_some_and(|rd| {
            rd.flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "kicad_pro"))
        });
        if has_pro {
            return table.is_file().then_some(table);
        }
        if table.is_file() {
            return Some(table);
        }
        current = current.parent()?.to_path_buf();
    }
}

// ----------------------------------------------------------- LibraryManager

/// Resolves symbols by `lib_id` (e.g. `Device:R`) across libraries.
#[derive(Debug, Clone, Default)]
pub struct LibraryManager {
    pub libraries: OrderedMap<SymbolLibrary>,
    pub search_paths: Vec<String>,
    /// Nickname -> `.kicad_sym` path from `sym-lib-table` rows (kct addition).
    pub table_entries: OrderedMap<PathBuf>,
}

impl LibraryManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_library(&mut self, name: &str, library: SymbolLibrary) {
        self.libraries.insert(name, library);
    }

    /// Load a library file, registering it under `name` or the file stem.
    pub fn load_library(
        &mut self,
        path: impl AsRef<Path>,
        name: Option<&str>,
    ) -> Result<&SymbolLibrary> {
        let path = path.as_ref();
        let lib = SymbolLibrary::load(path)?;
        let lib_name = match name {
            Some(n) => n.to_string(),
            None => path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        self.libraries.insert(lib_name.clone(), lib);
        Ok(&self.libraries[lib_name.as_str()])
    }

    /// Register a schematic's embedded `lib_symbols` (does not overwrite
    /// symbols already loaded), then resolve `extends` across embedded libs.
    pub fn load_embedded(&mut self, schematic: &super::schematic::Schematic) -> Result<()> {
        self.load_embedded_sexp(schematic.lib_symbols())
    }

    /// [`load_embedded`](Self::load_embedded) from a `(lib_symbols ...)` node.
    pub fn load_embedded_sexp(&mut self, lib_symbols: Option<&SExp>) -> Result<()> {
        let Some(lib_symbols) = lib_symbols else {
            return Ok(());
        };
        for sym_sexp in lib_symbols.find_all("symbol") {
            let sym = LibrarySymbol::from_sexp(sym_sexp);
            let (lib_name, short) = match sym.name.split_once(':') {
                Some((l, s)) => (l.to_string(), s.to_string()),
                None => (sym.name.clone(), sym.name.clone()),
            };
            if !self.libraries.contains_key(&lib_name) {
                self.libraries.insert(
                    lib_name.clone(),
                    SymbolLibrary::new("<embedded>", OrderedMap::new()),
                );
            }
            let lib = self.libraries.get_mut(&lib_name).expect("inserted");
            if !lib.symbols.contains_key(&short) {
                lib.symbols.insert(short, sym);
            }
        }

        // Combined lookup across embedded libraries (later keys shadow).
        let mut origin: OrderedMap<(String, String)> = OrderedMap::new();
        let mut all: OrderedMap<LibrarySymbol> = OrderedMap::new();
        for (lib_name, lib) in self.libraries.iter() {
            if lib.path == "<embedded>" {
                for (key, sym) in lib.symbols.iter() {
                    all.insert(key, sym.clone());
                    origin.insert(key, (lib_name.to_string(), key.to_string()));
                }
            }
        }
        if all.is_empty() {
            return Ok(());
        }
        resolve_extends(&mut all)?;
        for (key, sym) in all.iter() {
            let (lib_name, sym_key) = &origin[key];
            if let Some(target) = self
                .libraries
                .get_mut(lib_name)
                .and_then(|l| l.symbols.get_mut(sym_key))
            {
                target.pins = sym.pins.clone();
                target.graphics = sym.graphics.clone();
            }
        }
        Ok(())
    }

    pub fn add_search_path(&mut self, path: &str) {
        self.search_paths.push(path.to_string());
    }

    /// Register every `type "KiCad"` row of a `sym-lib-table`; returns the
    /// number of rows added. Libraries load lazily in [`get_symbol`](Self::get_symbol).
    pub fn add_sym_lib_table(&mut self, table: &Path) -> usize {
        let mut added = 0;
        for entry in parse_sym_lib_table(table) {
            if let Some(path) = entry.resolved_path {
                if !self.table_entries.contains_key(&entry.name) {
                    self.table_entries.insert(entry.name, path);
                    added += 1;
                }
            }
        }
        added
    }

    /// Lookup without loading anything from disk.
    pub fn find_symbol(&self, lib_id: &str) -> Option<&LibrarySymbol> {
        match lib_id.split_once(':') {
            None => self.libraries.values().find_map(|l| l.get_symbol(lib_id)),
            Some((lib, sym)) => self.libraries.get(lib).and_then(|l| l.get_symbol(sym)),
        }
    }

    /// Resolve a `lib_id`, loading `<lib>.kicad_sym` from the sym-lib-table
    /// or search paths when the loaded libraries don't have it (a partial,
    /// embedded library is replaced by the full on-disk one).
    pub fn get_symbol(&mut self, lib_id: &str) -> Option<&LibrarySymbol> {
        let Some((lib_name, sym_name)) = lib_id.split_once(':') else {
            return self.libraries.values().find_map(|l| l.get_symbol(lib_id));
        };
        if self
            .libraries
            .get(lib_name)
            .is_some_and(|l| l.get_symbol(sym_name).is_some())
        {
            return self.libraries[lib_name].get_symbol(sym_name);
        }
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = self.table_entries.get(lib_name) {
            candidates.push(p.clone());
        }
        candidates.extend(
            self.search_paths
                .iter()
                .map(|sp| Path::new(sp).join(format!("{lib_name}.kicad_sym"))),
        );
        for path in candidates {
            if path.exists() {
                if self.load_library(&path, Some(lib_name)).is_err() {
                    continue;
                }
                return self.libraries[lib_name].get_symbol(sym_name);
            }
        }
        None
    }

    /// Pin number -> schematic position for a placed instance of `lib_id`.
    pub fn get_pin_positions(
        &mut self,
        lib_id: &str,
        instance_pos: (f64, f64),
        instance_rot: f64,
        mirror: &str,
    ) -> OrderedMap<(f64, f64)> {
        match self.get_symbol(lib_id) {
            Some(sym) => sym.get_all_pin_positions(instance_pos, instance_rot, mirror),
            None => OrderedMap::new(),
        }
    }
}
