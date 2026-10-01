//! Element dataclasses of the PCB model (`Layer`, `Net`, `Pad`, `Footprint`,
//! `Segment`, `Arc`, `Via`, `Zone`, graphics, setup/stackup, net classes).
//!
//! Each type parses from its S-expression node with the exact lookup rules
//! of the upstream `from_sexp` (recursive `find` vs direct `find_child`,
//! Python `x or default` fallbacks). Coordinates are in mm; see [`super::Pcb`]
//! for the board-relative coordinate convention.

use std::f64::consts::TAU;

use crate::core::board_outline::legacy_arc_points;
use crate::sexp::{SExp, Value};

use super::util::{
    atom, find_xy, gf, gf_or, gi, gs, gs_or, list, new_uuid, pair_xy, string_atoms, Untracked,
};

pub type Point = (f64, f64);

/// PCB layer definition from the `(layers ...)` table.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub number: i64,
    pub name: String,
    /// `signal`, `power`, `mixed`, `jumper` or `user` (Python `type`).
    pub layer_type: String,
}

impl Layer {
    /// Copper per the upstream `copper_layers` rule (`signal`/`power`).
    pub fn is_copper(&self) -> bool {
        matches!(self.layer_type.as_str(), "signal" | "power")
    }
}

/// PCB net declaration `(net N "name")`.
#[derive(Debug, Clone, PartialEq)]
pub struct Net {
    pub number: i64,
    pub name: String,
}

/// Inline `(net ...)` reference: numeric `(net N ["name"])` or KiCad 10
/// name-only `(net "name")`. Returns `(number, name, name_only)`.
pub(crate) fn parse_net_ref(net: &SExp) -> (i64, String, bool) {
    match gi(net, 0) {
        Some(n) => (n, gs(net, 1).unwrap_or_default(), false),
        None => (0, gs(net, 0).unwrap_or_default(), true),
    }
}

/// Python `_sync_at_angle`: write `angle` into `(at x y [angle])` so a node
/// that arrived as 2 tokens returns to 2 tokens when its angle returns to 0
/// (only a token this write-through appended is ever removed).
pub(crate) fn sync_at_angle(synthetic: &mut bool, at: &mut SExp, angle: f64) {
    if at.children.len() >= 3 {
        if angle == 0.0 && *synthetic && at.children.len() == 3 {
            at.children.remove(2);
            *synthetic = false;
        } else {
            at.set_value(2, angle);
        }
    } else if angle != 0.0 {
        at.push(atom(angle));
        *synthetic = true;
    }
}

/// Component pad.
#[derive(Debug, Clone, PartialEq)]
pub struct Pad {
    pub number: String,
    /// `smd`, `thru_hole`, `np_thru_hole`, `connect` (Python `type`).
    pub pad_type: String,
    /// `roundrect`, `rect`, `circle`, `oval`, `trapezoid`, `custom`.
    pub shape: String,
    /// Footprint-local offset (rotate by the footprint rotation for board
    /// coordinates; see [`crate::core::geometry::rotate_pad_offset`]).
    pub position: Point,
    pub size: Point,
    pub layers: Vec<String>,
    pub net_number: i64,
    pub net_name: String,
    pub drill: f64,
    pub solder_mask_margin: Option<f64>,
    pub uuid: String,
    /// Absolute board-frame angle: KiCad stores pad angles with the parent
    /// footprint rotation already folded in (do not add it again).
    pub rotation: f64,
    /// `roundrect` corner ratio (KiCad default 0.25).
    pub roundrect_rratio: f64,
    /// Both dimensions of an oval `(drill oval W H)`.
    pub drill_size: Option<Point>,
    pub drill_offset: Point,
    pub(crate) at_angle_synthetic: Untracked<bool>,
}

impl Pad {
    /// Net id (alias of `net_number`).
    pub fn net(&self) -> i64 {
        self.net_number
    }

    pub fn from_sexp(sexp: &SExp) -> Pad {
        let mut pad = Pad {
            number: gs(sexp, 0).unwrap_or_default(),
            pad_type: gs(sexp, 1).unwrap_or_default(),
            shape: gs(sexp, 2).unwrap_or_default(),
            position: (0.0, 0.0),
            size: (0.0, 0.0),
            layers: Vec::new(),
            net_number: 0,
            net_name: String::new(),
            drill: 0.0,
            solder_mask_margin: None,
            uuid: String::new(),
            rotation: 0.0,
            roundrect_rratio: 0.25,
            drill_size: None,
            drill_offset: (0.0, 0.0),
            at_angle_synthetic: Untracked(false),
        };
        if let Some(at) = sexp.find("at") {
            pad.position = (gf(at, 0).unwrap_or(0.0), gf(at, 1).unwrap_or(0.0));
            pad.rotation = gf(at, 2).unwrap_or(0.0);
        }
        if let Some(size) = sexp.find("size") {
            let w = gf(size, 0).unwrap_or(0.0);
            pad.size = (w, gf_or(size, 1, w));
        }
        if let Some(r) = sexp.find("roundrect_rratio").and_then(|n| gf(n, 0)) {
            pad.roundrect_rratio = r;
        }
        if let Some(layers) = sexp.find("layers") {
            pad.layers = string_atoms(layers);
        }
        if let Some(net) = sexp.find("net") {
            let (n, name, _) = parse_net_ref(net);
            pad.net_number = n;
            pad.net_name = name;
        }
        if let Some(drill) = sexp.find("drill") {
            pad.drill = gf(drill, 0).unwrap_or(0.0);
            if gs(drill, 0).as_deref() == Some("oval") {
                pad.drill_size = Some((gf(drill, 1).unwrap_or(0.0), gf(drill, 2).unwrap_or(0.0)));
            }
            if let Some(offset) = drill.find("offset") {
                pad.drill_offset = (
                    gf(offset, 0).unwrap_or(f64::NAN),
                    gf(offset, 1).unwrap_or(f64::NAN),
                );
            }
        }
        if let Some(m) = sexp.find("solder_mask_margin") {
            pad.solder_mask_margin = gf(m, 0);
        }
        if let Some(u) = sexp.find("uuid") {
            pad.uuid = gs(u, 0).unwrap_or_default();
        }
        pad
    }
}

fn parse_effects(sexp: &SExp, hidden: &mut bool, size: &mut Point, thickness: &mut f64) {
    if let Some(effects) = sexp.find("effects") {
        if effects.find("hide").is_some() {
            *hidden = true;
        }
        if let Some(font) = effects.find("font") {
            if let Some(s) = font.find("size") {
                let w = gf_or(s, 0, 1.0);
                *size = (w, gf_or(s, 1, w));
            }
            if let Some(t) = font.find("thickness") {
                *thickness = gf_or(t, 0, 0.15);
            }
        }
    }
}

/// Footprint text (`fp_text`, or a KiCad 8+ `property` shown as text).
#[derive(Debug, Clone, PartialEq)]
pub struct FootprintText {
    /// `reference`, `value` or `user`.
    pub text_type: String,
    pub text: String,
    /// Footprint-local position.
    pub position: Point,
    pub layer: String,
    /// `(width, height)` in mm.
    pub font_size: Point,
    pub font_thickness: f64,
    pub uuid: String,
    pub hidden: bool,
    /// Serialized board-frame angle.
    pub rotation: f64,
}

impl FootprintText {
    fn blank(text_type: String, text: String) -> Self {
        FootprintText {
            text_type,
            text,
            position: (0.0, 0.0),
            layer: String::new(),
            font_size: (1.0, 1.0),
            font_thickness: 0.15,
            uuid: String::new(),
            hidden: false,
            rotation: 0.0,
        }
    }

    fn common(&mut self, sexp: &SExp) {
        if let Some(at) = sexp.find("at") {
            self.position = (gf(at, 0).unwrap_or(0.0), gf(at, 1).unwrap_or(0.0));
            self.rotation = gf(at, 2).unwrap_or(0.0);
        }
        if let Some(l) = sexp.find("layer") {
            self.layer = gs(l, 0).unwrap_or_default();
        }
        if let Some(u) = sexp.find("uuid") {
            self.uuid = gs(u, 0).unwrap_or_default();
        }
    }

    /// Parse `(fp_text <type> "<text>" ...)`.
    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut t = Self::blank(
            gs(sexp, 0).unwrap_or_default(),
            gs(sexp, 1).unwrap_or_default(),
        );
        t.common(sexp);
        parse_effects(sexp, &mut t.hidden, &mut t.font_size, &mut t.font_thickness);
        t
    }

    /// Parse a KiCad 8+ `(property "Reference" "U1" ...)` as text.
    pub fn from_property_sexp(sexp: &SExp, text_type: &str) -> Self {
        let mut t = Self::blank(text_type.to_string(), gs(sexp, 1).unwrap_or_default());
        t.common(sexp);
        if let Some(hide) = sexp.find("hide") {
            t.hidden = gs(hide, 0).as_deref() == Some("yes");
        }
        parse_effects(sexp, &mut t.hidden, &mut t.font_size, &mut t.font_thickness);
        t
    }

    pub fn font_height(&self) -> f64 {
        self.font_size.1
    }
}

/// Whether a raw `(fill ...)` token means the interior is printed
/// (`yes`/`solid`/`true`, case-insensitive).
pub fn fill_token_is_filled(token: &str) -> bool {
    matches!(
        token.trim().to_ascii_lowercase().as_str(),
        "yes" | "solid" | "true"
    )
}

/// Normalize modern three-point and legacy center/signed-angle arcs into
/// `(start, mid, end)` (input coordinate frame preserved).
pub fn arc_points_from_sexp(sexp: &SExp) -> (Point, Point, Point) {
    let mut start = find_xy(sexp, "start").unwrap_or((0.0, 0.0));
    let mid_node = sexp.find("mid");
    let mut mid = mid_node.map(super::util::xy).unwrap_or((0.0, 0.0));
    let mut end = find_xy(sexp, "end").unwrap_or((0.0, 0.0));
    if mid_node.is_none() {
        if let Some(angle) = sexp.find("angle") {
            let deg = gf(angle, 0).unwrap_or(0.0);
            let (center, on_arc) = (start, end);
            (start, mid, end) = legacy_arc_points(on_arc, center, deg);
        }
    }
    (start, mid, end)
}

/// Footprint graphic (`fp_line`, `fp_rect`, `fp_circle`, `fp_arc`,
/// `fp_poly`) in footprint-local coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct FootprintGraphic {
    /// `line`, `rect`, `circle`, `arc`, `poly`.
    pub graphic_type: String,
    pub layer: String,
    pub stroke_width: f64,
    pub start: Point,
    pub end: Point,
    pub center: Option<Point>,
    pub radius: Option<f64>,
    pub points: Vec<Point>,
    pub uuid: String,
    pub mid: Option<Point>,
    /// Raw `(fill ...)` token (`yes`/`no`, legacy `solid`/`none`, or empty).
    pub fill: String,
}

impl FootprintGraphic {
    pub fn is_filled(&self) -> bool {
        fill_token_is_filled(&self.fill)
    }

    pub fn from_sexp(sexp: &SExp, graphic_type: &str) -> Self {
        let mut g = FootprintGraphic {
            graphic_type: graphic_type.to_string(),
            layer: String::new(),
            stroke_width: 0.0,
            start: (0.0, 0.0),
            end: (0.0, 0.0),
            center: None,
            radius: None,
            points: Vec::new(),
            uuid: String::new(),
            mid: None,
            fill: String::new(),
        };
        parse_graphic_common(
            sexp,
            graphic_type,
            &mut g.layer,
            &mut g.stroke_width,
            &mut g.start,
            &mut g.mid,
            &mut g.end,
            &mut g.center,
            &mut g.points,
            &mut g.fill,
            &mut g.uuid,
        );
        g
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_graphic_common(
    sexp: &SExp,
    graphic_type: &str,
    layer: &mut String,
    stroke_width: &mut f64,
    start: &mut Point,
    mid: &mut Option<Point>,
    end: &mut Point,
    center: &mut Option<Point>,
    points: &mut Vec<Point>,
    fill: &mut String,
    uuid: &mut String,
) {
    if let Some(l) = sexp.find("layer") {
        *layer = gs(l, 0).unwrap_or_default();
    }
    if let Some(w) = sexp.find("stroke").and_then(|s| s.find("width")) {
        *stroke_width = gf(w, 0).unwrap_or(0.0);
    }
    if let Some(p) = find_xy(sexp, "start") {
        *start = p;
    }
    if let Some(p) = find_xy(sexp, "end") {
        *end = p;
    }
    if graphic_type == "arc" {
        let (s, m, e) = arc_points_from_sexp(sexp);
        *start = s;
        *mid = Some(m);
        *end = e;
    }
    if let Some(c) = find_xy(sexp, "center") {
        *center = Some(c);
    }
    if graphic_type == "poly" {
        if let Some(pts) = sexp.find("pts") {
            for xy in pts.find_all("xy") {
                points.push(super::util::xy(xy));
            }
        }
    }
    if let Some(f) = sexp.find("fill") {
        *fill = gs(f, 0).unwrap_or_default();
    }
    if let Some(u) = sexp.find("uuid") {
        *uuid = gs(u, 0).unwrap_or_default();
    }
}

/// Board-level text (`gr_text`).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphicText {
    pub text: String,
    pub position: Point,
    pub layer: String,
    pub font_size: Point,
    pub font_thickness: f64,
    pub uuid: String,
    pub hidden: bool,
    pub rotation: f64,
}

impl GraphicText {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut t = GraphicText {
            text: gs(sexp, 0).unwrap_or_default(),
            position: (0.0, 0.0),
            layer: String::new(),
            font_size: (1.0, 1.0),
            font_thickness: 0.15,
            uuid: String::new(),
            hidden: false,
            rotation: 0.0,
        };
        if let Some(at) = sexp.find("at") {
            t.position = (gf(at, 0).unwrap_or(0.0), gf(at, 1).unwrap_or(0.0));
            t.rotation = gf(at, 2).unwrap_or(0.0);
        }
        if let Some(l) = sexp.find("layer") {
            t.layer = gs(l, 0).unwrap_or_default();
        }
        if let Some(u) = sexp.find("uuid") {
            t.uuid = gs(u, 0).unwrap_or_default();
        }
        parse_effects(sexp, &mut t.hidden, &mut t.font_size, &mut t.font_thickness);
        t
    }

    pub fn font_height(&self) -> f64 {
        self.font_size.1
    }
}

/// Board-level graphic (`gr_line`, `gr_rect`, `gr_circle`, `gr_arc`,
/// `gr_poly`), sheet-absolute coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardGraphic {
    pub graphic_type: String,
    pub layer: String,
    pub stroke_width: f64,
    pub start: Point,
    pub end: Point,
    pub center: Option<Point>,
    pub uuid: String,
    pub mid: Option<Point>,
    pub points: Vec<Point>,
    pub fill: String,
}

impl BoardGraphic {
    pub fn is_filled(&self) -> bool {
        fill_token_is_filled(&self.fill)
    }

    pub fn from_sexp(sexp: &SExp, graphic_type: &str) -> Self {
        let mut g = BoardGraphic {
            graphic_type: graphic_type.to_string(),
            layer: String::new(),
            stroke_width: 0.0,
            start: (0.0, 0.0),
            end: (0.0, 0.0),
            center: None,
            uuid: String::new(),
            mid: None,
            points: Vec::new(),
            fill: String::new(),
        };
        parse_graphic_common(
            sexp,
            graphic_type,
            &mut g.layer,
            &mut g.stroke_width,
            &mut g.start,
            &mut g.mid,
            &mut g.end,
            &mut g.center,
            &mut g.points,
            &mut g.fill,
            &mut g.uuid,
        );
        // A gr_poly has no (start ...): report its first vertex as location.
        if graphic_type == "poly" {
            if let Some(first) = g.points.first() {
                g.start = *first;
            }
        }
        g
    }
}

/// `(attr ...)` tokens the parser models; anything else is preserved in
/// `attr_unknown_tokens`.
pub const ATTR_KNOWN_TOKENS: &[&str] = &[
    "smd",
    "through_hole",
    "exclude_from_pos_files",
    "exclude_from_bom",
    "locked",
    "dnp",
];

/// Placed component footprint.
///
/// `position` is board-relative once owned by a [`super::Pcb`]; mutate
/// through [`super::Pcb::footprint_mut`] so edits reach the S-expression
/// tree that `save` writes. Plain field assignment only changes this value.
#[derive(Debug, Clone, PartialEq)]
pub struct Footprint {
    pub name: String,
    pub layer: String,
    pub position: Point,
    pub rotation: f64,
    pub reference: String,
    pub value: String,
    pub pads: Vec<Pad>,
    pub texts: Vec<FootprintText>,
    pub graphics: Vec<FootprintGraphic>,
    pub uuid: String,
    pub description: String,
    pub tags: String,
    /// `smd`, `through_hole` or empty.
    pub attr: String,
    pub exclude_from_pos_files: bool,
    pub exclude_from_bom: bool,
    pub locked: bool,
    pub dnp: bool,
    /// Extra properties (LCSC, MPN, ...) in file order, excluding
    /// Reference/Value/Footprint.
    pub properties: Vec<(String, String)>,
    /// `(attr ...)` tokens not modeled above (`board_only`,
    /// `allow_missing_courtyard`, ...), re-emitted verbatim on rebuild.
    pub attr_unknown_tokens: Untracked<Vec<String>>,
    pub(crate) at_angle_synthetic: Untracked<bool>,
}

impl Footprint {
    /// Unlinked footprint with defaults (Python `Footprint(...)`).
    pub fn new(
        name: &str,
        layer: &str,
        position: Point,
        rotation: f64,
        reference: &str,
        value: &str,
    ) -> Self {
        Footprint {
            name: name.into(),
            layer: layer.into(),
            position,
            rotation,
            reference: reference.into(),
            value: value.into(),
            pads: Vec::new(),
            texts: Vec::new(),
            graphics: Vec::new(),
            uuid: String::new(),
            description: String::new(),
            tags: String::new(),
            attr: String::new(),
            exclude_from_pos_files: false,
            exclude_from_bom: false,
            locked: false,
            dnp: false,
            properties: Vec::new(),
            attr_unknown_tokens: Untracked(Vec::new()),
            at_angle_synthetic: Untracked(false),
        }
    }

    /// Extra property by name.
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Parse a `(footprint ...)` / legacy `(module ...)` node. Positions are
    /// the file's sheet-absolute values; `Pcb` converts them.
    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut fp = Footprint::new(
            &gs(sexp, 0).unwrap_or_default(),
            "F.Cu",
            (0.0, 0.0),
            0.0,
            "",
            "",
        );
        // Footprint-level scalars use direct children only: pads, texts and
        // properties carry their own (at)/(layer)/(uuid)/(locked).
        if let Some(l) = sexp.find_child("layer") {
            fp.layer = gs_or(l, 0, "F.Cu");
        }
        if let Some(at) = sexp.find_child("at") {
            fp.position = (gf(at, 0).unwrap_or(0.0), gf(at, 1).unwrap_or(0.0));
            fp.rotation = gf(at, 2).unwrap_or(0.0);
        }
        if let Some(u) = sexp.find_child("uuid") {
            fp.uuid = gs(u, 0).unwrap_or_default();
        }
        if let Some(d) = sexp.find_child("descr") {
            fp.description = gs(d, 0).unwrap_or_default();
        }
        if let Some(t) = sexp.find_child("tags") {
            fp.tags = gs(t, 0).unwrap_or_default();
        }
        if let Some(locked) = sexp.find_child("locked") {
            if matches!(gs_or(locked, 0, "yes").as_str(), "yes" | "true") {
                fp.locked = true;
            }
        }
        if let Some(attr) = sexp.find_child("attr") {
            let first = gs(attr, 0).unwrap_or_default();
            let start = if first == "smd" || first == "through_hole" {
                fp.attr = first;
                1
            } else {
                0
            };
            let mut unknown = Vec::new();
            for i in start..attr.children.len() {
                let Some(token) = gs(attr, i) else { continue };
                match token.as_str() {
                    "exclude_from_pos_files" => fp.exclude_from_pos_files = true,
                    "exclude_from_bom" => fp.exclude_from_bom = true,
                    "locked" => fp.locked = true,
                    "dnp" => fp.dnp = true,
                    _ => unknown.push(token),
                }
            }
            fp.attr_unknown_tokens = Untracked(unknown);
        }
        // KiCad 7: fp_text reference/value.
        for node in sexp.find_all("fp_text") {
            let t = FootprintText::from_sexp(node);
            match t.text_type.as_str() {
                "reference" => fp.reference = t.text.clone(),
                "value" => fp.value = t.text.clone(),
                _ => {}
            }
            fp.texts.push(t);
        }
        // KiCad 8+: properties.
        for prop in sexp.find_all("property") {
            let Some(name) = gs(prop, 0) else { continue };
            let value = gs(prop, 1).unwrap_or_default();
            match name.as_str() {
                "Reference" => {
                    fp.reference = value;
                    fp.texts
                        .push(FootprintText::from_property_sexp(prop, "reference"));
                }
                "Value" => {
                    fp.value = value;
                    fp.texts
                        .push(FootprintText::from_property_sexp(prop, "value"));
                }
                "Footprint" => {}
                _ => match fp.properties.iter_mut().find(|(k, _)| *k == name) {
                    Some(entry) => entry.1 = value,
                    None => fp.properties.push((name, value)),
                },
            }
        }
        for pad in sexp.find_all("pad") {
            fp.pads.push(Pad::from_sexp(pad));
        }
        for kind in ["line", "rect", "circle", "arc", "poly"] {
            let tag = format!("fp_{kind}");
            for g in sexp.find_all(&tag) {
                fp.graphics.push(FootprintGraphic::from_sexp(g, kind));
            }
        }
        fp
    }

    /// Build the `(attr ...)` node for the current flags, or `None` when no
    /// flag is set. `locked` is never emitted in-attr (KiCad 10 rejects it).
    pub(crate) fn attr_node(&self) -> Option<SExp> {
        let unknown: Vec<&String> = self
            .attr_unknown_tokens
            .0
            .iter()
            .filter(|t| *t != "locked")
            .collect();
        if self.attr.is_empty()
            && !self.dnp
            && !self.exclude_from_pos_files
            && !self.exclude_from_bom
            && unknown.is_empty()
        {
            return None;
        }
        let mut node = list("attr", vec![]);
        if !self.attr.is_empty() {
            node.push(atom(self.attr.as_str()));
        }
        if self.exclude_from_pos_files {
            node.push(atom("exclude_from_pos_files"));
        }
        if self.exclude_from_bom {
            node.push(atom("exclude_from_bom"));
        }
        if self.dnp {
            node.push(atom("dnp"));
        }
        for t in unknown {
            node.push(atom(t.as_str()));
        }
        Some(node)
    }
}

/// Straight copper track.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: Point,
    pub end: Point,
    pub width: f64,
    pub layer: String,
    pub net_number: i64,
    pub net_name: String,
    pub uuid: String,
    /// Parsed in KiCad 10 name-only `(net "SDA")` form; re-emitted that way.
    pub net_name_only: Untracked<bool>,
}

fn net_ref_sexp(name_only: bool, name: &str, number: i64) -> SExp {
    if name_only && !name.is_empty() {
        list("net", vec![SExp::quoted(name)])
    } else {
        list("net", vec![atom(number)])
    }
}

impl Segment {
    pub fn new(start: Point, end: Point, width: f64, layer: &str, net_number: i64) -> Self {
        Segment {
            start,
            end,
            width,
            layer: layer.into(),
            net_number,
            net_name: String::new(),
            uuid: String::new(),
            net_name_only: Untracked(false),
        }
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut seg = Segment::new((0.0, 0.0), (0.0, 0.0), 0.0, "", 0);
        if let Some(p) = find_xy(sexp, "start") {
            seg.start = p;
        }
        if let Some(p) = find_xy(sexp, "end") {
            seg.end = p;
        }
        if let Some(w) = sexp.find("width") {
            seg.width = gf(w, 0).unwrap_or(0.0);
        }
        if let Some(l) = sexp.find("layer") {
            seg.layer = gs(l, 0).unwrap_or_default();
        }
        if let Some(net) = sexp.find("net") {
            let (n, name, only) = parse_net_ref(net);
            seg.net_number = n;
            seg.net_name = name;
            seg.net_name_only = Untracked(only);
        }
        if let Some(u) = sexp.find("uuid") {
            seg.uuid = gs(u, 0).unwrap_or_default();
        }
        seg
    }

    /// Straight-line length in mm.
    pub fn length(&self) -> f64 {
        (self.end.0 - self.start.0).hypot(self.end.1 - self.start.1)
    }

    /// Serialize in KiCad's canonical field order (`uuid` before `net`),
    /// shifting coordinates by `offset` (board-relative -> sheet-absolute).
    /// Assigns a fresh UUID when empty.
    pub fn to_sexp(&mut self, offset: Point) -> SExp {
        if self.uuid.is_empty() {
            self.uuid = new_uuid();
        }
        let (ox, oy) = offset;
        list(
            "segment",
            vec![
                pair_xy("start", self.start.0 + ox, self.start.1 + oy),
                pair_xy("end", self.end.0 + ox, self.end.1 + oy),
                list("width", vec![atom(self.width)]),
                list("layer", vec![atom(self.layer.as_str())]),
                list("uuid", vec![atom(self.uuid.as_str())]),
                net_ref_sexp(self.net_name_only.0, &self.net_name, self.net_number),
            ],
        )
    }
}

/// Circular copper track through `start`, `mid`, `end` (board-relative).
#[derive(Debug, Clone, PartialEq)]
pub struct Arc {
    pub start: Point,
    pub mid: Point,
    pub end: Point,
    pub width: f64,
    pub layer: String,
    pub net_number: i64,
    pub net_name: String,
    pub uuid: String,
    pub net_name_only: Untracked<bool>,
}

impl Arc {
    /// Parse and validate a copper `(arc ...)`; invalid geometry is an error
    /// rather than silently becoming a chord.
    pub fn from_sexp(sexp: &SExp) -> Result<Self, String> {
        let seg = Segment::from_sexp(sexp);
        let mut pts = [(0.0, 0.0); 3];
        for (i, tag) in ["start", "mid", "end"].iter().enumerate() {
            let node = sexp.find(tag);
            match node.map(|n| (gf(n, 0), gf(n, 1))) {
                Some((Some(x), Some(y))) => pts[i] = (x, y),
                _ => return Err(format!("Copper arc is missing {tag} coordinates")),
            }
        }
        let arc = Arc {
            start: pts[0],
            mid: pts[1],
            end: pts[2],
            width: seg.width,
            layer: seg.layer,
            net_number: seg.net_number,
            net_name: seg.net_name,
            uuid: seg.uuid,
            net_name_only: seg.net_name_only,
        };
        arc.circular_geometry()?;
        if !arc.width.is_finite() || arc.width <= 0.0 {
            return Err("Copper arc width must be finite and positive".into());
        }
        if !arc.layer.ends_with(".Cu") {
            return Err("Copper arc must be on a copper layer".into());
        }
        Ok(arc)
    }

    /// `(center, radius, start_angle, signed_sweep)` through `mid`.
    pub fn circular_geometry(&self) -> Result<(Point, f64, f64, f64), String> {
        let all = [
            self.start.0,
            self.start.1,
            self.mid.0,
            self.mid.1,
            self.end.0,
            self.end.1,
        ];
        if !all.iter().all(|v| v.is_finite()) {
            return Err("Copper arc coordinates must be finite".into());
        }
        let (ax, ay) = self.start;
        let (bx, by) = (self.mid.0 - ax, self.mid.1 - ay);
        let (cx, cy) = (self.end.0 - ax, self.end.1 - ay);
        let det = 2.0 * (bx * cy - by * cx);
        let scale = (bx * bx + by * by).max(cx * cx + cy * cy);
        if scale == 0.0 || det.abs() <= 1e-14 * scale {
            return Err("Copper arc points must define a nondegenerate circle".into());
        }
        let ux = ((bx * bx + by * by) * cy - (cx * cx + cy * cy) * by) / det;
        let uy = (bx * (cx * cx + cy * cy) - cx * (bx * bx + by * by)) / det;
        let radius = ux.hypot(uy);
        let start = (-uy).atan2(-ux);
        let rem = |a: f64| a.rem_euclid(TAU);
        let mid = rem((by - uy).atan2(bx - ux) - start);
        let end = rem((cy - uy).atan2(cx - ux) - start);
        let sweep = if mid <= end { end } else { end - TAU };
        if ![ux, uy, radius, sweep].iter().all(|v| v.is_finite()) || radius <= 0.0 {
            return Err("Copper arc geometry is not finite".into());
        }
        if ![ax + ux, ay + uy, radius * sweep]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err("Copper arc geometry is not finite".into());
        }
        Ok(((ax + ux, ay + uy), radius, start, sweep))
    }

    /// Swept centerline length (never the chord); NaN for invalid geometry.
    pub fn length(&self) -> f64 {
        self.circular_geometry()
            .map(|(_, r, _, sweep)| r * sweep.abs())
            .unwrap_or(f64::NAN)
    }

    /// Polyline approximation with sagitta <= `max_error_mm` (upstream
    /// default 0.00001); endpoints exact.
    pub fn centerline_points(&self, max_error_mm: f64) -> Result<Vec<Point>, String> {
        if !max_error_mm.is_finite() || max_error_mm <= 0.0 {
            return Err("Arc approximation error must be finite and positive".into());
        }
        let ((cx, cy), radius, start, sweep) = self.circular_geometry()?;
        let step = 4.0 * (max_error_mm / (2.0 * radius)).min(0.5).sqrt().asin();
        if step == 0.0 {
            return Err("Copper arc approximation exceeds floating-point resolution".into());
        }
        let count = ((sweep.abs() / step).ceil() as usize).max(2);
        if count > 100_000 {
            return Err("Copper arc requires more than 100000 approximation edges".into());
        }
        let mut out = vec![self.start];
        for i in 1..count {
            let a = start + sweep * i as f64 / count as f64;
            out.push((cx + radius * a.cos(), cy + radius * a.sin()));
        }
        out.push(self.end);
        Ok(out)
    }

    pub fn to_sexp(&mut self, offset: Point) -> SExp {
        let mut seg = Segment {
            start: self.start,
            end: self.end,
            width: self.width,
            layer: self.layer.clone(),
            net_number: self.net_number,
            net_name: self.net_name.clone(),
            uuid: self.uuid.clone(),
            net_name_only: self.net_name_only,
        };
        let mut node = seg.to_sexp(offset);
        self.uuid = seg.uuid;
        node.name = Some("arc".into());
        node.insert(
            1,
            pair_xy("mid", self.mid.0 + offset.0, self.mid.1 + offset.1),
        );
        node
    }
}

/// Via.
#[derive(Debug, Clone, PartialEq)]
pub struct Via {
    pub position: Point,
    pub size: f64,
    pub drill: f64,
    pub layers: Vec<String>,
    pub net_number: i64,
    pub net_name: String,
    pub uuid: String,
    /// Leading `micro`/`blind`/`buried` token; `None` for through vias.
    pub via_type: Option<String>,
    pub net_name_only: Untracked<bool>,
    /// Per-via `(tenting (front ..) (back ..))` raw tokens (`yes`/`no`/`none`).
    pub tenting_front: Option<String>,
    pub tenting_back: Option<String>,
}

impl Via {
    pub fn new(position: Point, size: f64, drill: f64, layers: &[&str], net_number: i64) -> Self {
        Via {
            position,
            size,
            drill,
            layers: layers.iter().map(|s| s.to_string()).collect(),
            net_number,
            net_name: String::new(),
            uuid: String::new(),
            via_type: None,
            net_name_only: Untracked(false),
            tenting_front: None,
            tenting_back: None,
        }
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut via = Via::new((0.0, 0.0), 0.0, 0.0, &[], 0);
        if let Some(first) = sexp.children.first() {
            if let (true, Some(Value::Str(t))) = (first.is_atom(), &first.value) {
                if matches!(t.as_str(), "micro" | "blind" | "buried") {
                    via.via_type = Some(t.clone());
                }
            }
        }
        if let Some(p) = find_xy(sexp, "at") {
            via.position = p;
        }
        if let Some(s) = sexp.find("size") {
            via.size = gf(s, 0).unwrap_or(0.0);
        }
        if let Some(d) = sexp.find("drill") {
            via.drill = gf(d, 0).unwrap_or(0.0);
        }
        if let Some(l) = sexp.find("layers") {
            via.layers = string_atoms(l);
        }
        if let Some(t) = sexp.find("tenting") {
            if let Some(f) = t.find("front") {
                via.tenting_front = gs(f, 0);
            }
            if let Some(b) = t.find("back") {
                via.tenting_back = gs(b, 0);
            }
        }
        if let Some(net) = sexp.find("net") {
            let (n, name, only) = parse_net_ref(net);
            via.net_number = n;
            via.net_name = name;
            via.net_name_only = Untracked(only);
        }
        if let Some(u) = sexp.find("uuid") {
            via.uuid = gs(u, 0).unwrap_or_default();
        }
        via
    }

    /// Serialize (via-type token first, tenting after layers, uuid before
    /// net), shifting by `offset`. Assigns a fresh UUID when empty.
    pub fn to_sexp(&mut self, offset: Point) -> SExp {
        let mut node = list("via", vec![]);
        if let Some(t) = &self.via_type {
            node.push(atom(t.as_str()));
        }
        node.push(pair_xy(
            "at",
            self.position.0 + offset.0,
            self.position.1 + offset.1,
        ));
        node.push(list("size", vec![atom(self.size)]));
        node.push(list("drill", vec![atom(self.drill)]));
        node.push(list(
            "layers",
            self.layers.iter().map(|l| atom(l.as_str())).collect(),
        ));
        if self.tenting_front.is_some() || self.tenting_back.is_some() {
            let mut t = list("tenting", vec![]);
            if let Some(f) = &self.tenting_front {
                t.push(list("front", vec![atom(f.as_str())]));
            }
            if let Some(b) = &self.tenting_back {
                t.push(list("back", vec![atom(b.as_str())]));
            }
            node.push(t);
        }
        if self.uuid.is_empty() {
            self.uuid = new_uuid();
        }
        node.push(list("uuid", vec![atom(self.uuid.as_str())]));
        node.push(net_ref_sexp(
            self.net_name_only.0,
            &self.net_name,
            self.net_number,
        ));
        node
    }
}

/// Keepout flags of a rule area; a missing child means `allowed`.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneKeepout {
    pub tracks_allowed: bool,
    pub vias_allowed: bool,
    pub pads_allowed: bool,
    pub copperpour_allowed: bool,
    pub footprints_allowed: bool,
}

impl Default for ZoneKeepout {
    fn default() -> Self {
        ZoneKeepout {
            tracks_allowed: true,
            vias_allowed: true,
            pads_allowed: true,
            copperpour_allowed: true,
            footprints_allowed: true,
        }
    }
}

impl ZoneKeepout {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut k = ZoneKeepout::default();
        let allowed = |tag: &str| {
            sexp.find(tag)
                .map(|c| gs(c, 0).as_deref() != Some("not_allowed"))
        };
        if let Some(v) = allowed("tracks") {
            k.tracks_allowed = v;
        }
        if let Some(v) = allowed("vias") {
            k.vias_allowed = v;
        }
        if let Some(v) = allowed("pads") {
            k.pads_allowed = v;
        }
        if let Some(v) = allowed("copperpour") {
            k.copperpour_allowed = v;
        }
        if let Some(v) = allowed("footprints") {
            k.footprints_allowed = v;
        }
        k
    }
}

/// Last `(version N)` whose zones default to stroked fills when
/// `filled_areas_thickness` is absent (KiCad: `version < 20250210`).
pub const STROKED_FILL_LAST_VERSION: i64 = 20250209;

/// Copper pour zone or (with `keepout`) rule area.
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub net_number: i64,
    pub net_name: String,
    pub layer: String,
    pub uuid: String,
    pub name: String,
    /// Multi-layer spec (`(layers ...)`, may hold wildcards like `*.Cu`).
    pub layers: Vec<String>,
    pub keepout: Option<ZoneKeepout>,
    /// Boundary polygon (board-relative once owned by a `Pcb`).
    pub polygon: Vec<Point>,
    pub filled_polygons: Vec<Vec<Point>>,
    /// Layer of each filled polygon, parallel to `filled_polygons`.
    pub filled_polygon_layers: Vec<String>,
    pub priority: i64,
    pub min_thickness: f64,
    pub clearance: f64,
    pub thermal_gap: f64,
    pub thermal_bridge_width: f64,
    /// `thermal_reliefs`, `solid` or `none`.
    pub connect_pads: String,
    /// `solid` or `hatch`.
    pub fill_type: String,
    pub is_filled: bool,
    /// `(filled_areas_thickness yes|no)`; `None` when absent.
    pub filled_areas_thickness: Option<bool>,
    /// Board `(version N)`, 0 when unknown.
    pub file_version: i64,
}

impl Zone {
    pub fn new(net_number: i64, net_name: &str, layer: &str) -> Self {
        Zone {
            net_number,
            net_name: net_name.into(),
            layer: layer.into(),
            uuid: String::new(),
            name: String::new(),
            layers: Vec::new(),
            keepout: None,
            polygon: Vec::new(),
            filled_polygons: Vec::new(),
            filled_polygon_layers: Vec::new(),
            priority: 0,
            min_thickness: 0.2,
            clearance: 0.2,
            thermal_gap: 0.3,
            thermal_bridge_width: 0.3,
            connect_pads: "thermal_reliefs".into(),
            fill_type: "solid".into(),
            is_filled: false,
            filled_areas_thickness: None,
            file_version: 0,
        }
    }

    /// Whether `filled_polygons` are stroke centre-lines (legacy files)
    /// rather than final copper. An explicit `no` forces solid; `yes` is a
    /// no-op, exactly as KiCad's parser.
    pub fn is_stroked_fill(&self) -> bool {
        if self.filled_areas_thickness == Some(false) {
            return false;
        }
        0 < self.file_version && self.file_version <= STROKED_FILL_LAST_VERSION
    }

    /// Half-width the stored fill outlines grow by to be real copper.
    pub fn fill_inflation(&self) -> f64 {
        if !self.is_stroked_fill() {
            return 0.0;
        }
        self.min_thickness.max(0.0) / 2.0
    }

    /// Layer of `filled_polygons[index]` (falls back to the zone layer).
    pub fn filled_polygon_layer(&self, index: usize) -> &str {
        match self.filled_polygon_layers.get(index) {
            Some(l) if !l.is_empty() => l,
            _ => &self.layer,
        }
    }

    pub fn is_rule_area(&self) -> bool {
        self.keepout.is_some()
    }

    fn polygon_pts(node: &SExp) -> Vec<Point> {
        node.find("pts")
            .map(|pts| pts.find_all("xy").map(super::util::xy).collect())
            .unwrap_or_default()
    }

    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut zone = Zone::new(0, "", "");
        // First-descendant index, one walk (same order as repeated find()).
        let mut index: std::collections::HashMap<&str, &SExp> = std::collections::HashMap::new();
        for child in &sexp.children {
            for node in child.iter_all() {
                if let Some(name) = node.name.as_deref() {
                    index.entry(name).or_insert(node);
                }
            }
        }
        if let Some(net) = index.get("net") {
            match gi(net, 0) {
                Some(n) => zone.net_number = n,
                None => zone.net_name = gs(net, 0).unwrap_or_default(),
            }
        }
        if let Some(n) = index.get("net_name") {
            zone.net_name = gs(n, 0).unwrap_or_default();
        }
        if let Some(l) = index.get("layer") {
            zone.layer = gs(l, 0).unwrap_or_default();
        }
        if let Some(layers) = index.get("layers") {
            zone.layers = (0..layers.children.len())
                .filter_map(|i| gs(layers, i))
                .filter(|s| !s.is_empty())
                .collect();
            if zone.layer.is_empty() {
                if let Some(first) = zone.layers.first() {
                    zone.layer = first.clone();
                }
            }
        }
        if let Some(u) = index.get("uuid") {
            zone.uuid = gs(u, 0).unwrap_or_default();
        }
        if let Some(n) = index.get("name") {
            zone.name = gs(n, 0).unwrap_or_default();
        }
        if let Some(k) = index.get("keepout") {
            zone.keepout = Some(ZoneKeepout::from_sexp(k));
        }
        if let Some(p) = index.get("priority") {
            zone.priority = gi(p, 0).unwrap_or(0);
        }
        if let Some(m) = index.get("min_thickness") {
            zone.min_thickness = gf_or(m, 0, 0.2);
        }
        if let Some(f) = index.get("filled_areas_thickness") {
            zone.filled_areas_thickness = Some(gs(f, 0).as_deref() != Some("no"));
        }
        if let Some(cp) = index.get("connect_pads") {
            zone.connect_pads = match gs(cp, 0).as_deref() {
                Some("no") => "none",
                Some("yes") => "solid",
                _ => "thermal_reliefs",
            }
            .into();
            if let Some(c) = cp.find("clearance") {
                zone.clearance = gf_or(c, 0, 0.2);
            }
        }
        if let Some(fill) = index.get("fill") {
            zone.is_filled = gs(fill, 0).as_deref() == Some("yes");
            if let Some(g) = fill.find("thermal_gap") {
                zone.thermal_gap = gf_or(g, 0, 0.3);
            }
            if let Some(b) = fill.find("thermal_bridge_width") {
                zone.thermal_bridge_width = gf_or(b, 0, 0.3);
            }
            if let Some(mode) = fill.find("mode") {
                if gs(mode, 0).as_deref() == Some("hatch") {
                    zone.fill_type = "hatch".into();
                }
            }
        }
        if let Some(poly) = index.get("polygon") {
            zone.polygon = Self::polygon_pts(poly);
        }
        for fp in sexp.find_all("filled_polygon") {
            let pts = Self::polygon_pts(fp);
            if !pts.is_empty() {
                zone.filled_polygons.push(pts);
                let layer = fp
                    .find("layer")
                    .and_then(|l| gs(l, 0))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| zone.layer.clone());
                zone.filled_polygon_layers.push(layer);
            }
        }
        zone
    }
}

/// Board graphic line (`gr_line`).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphicLine {
    pub start: Point,
    pub end: Point,
    pub layer: String,
    pub width: f64,
    pub uuid: String,
}

impl GraphicLine {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let mut line = GraphicLine {
            start: find_xy(sexp, "start").unwrap_or((0.0, 0.0)),
            end: find_xy(sexp, "end").unwrap_or((0.0, 0.0)),
            layer: String::new(),
            width: 0.1,
            uuid: String::new(),
        };
        if let Some(l) = sexp.find("layer") {
            line.layer = gs(l, 0).unwrap_or_default();
        }
        if let Some(w) = sexp.find("width") {
            line.width = gf_or(w, 0, 0.1);
        }
        if let Some(w) = sexp.find("stroke").and_then(|s| s.find("width")) {
            line.width = gf_or(w, 0, 0.1);
        }
        if let Some(u) = sexp.find("uuid") {
            line.uuid = gs(u, 0).unwrap_or_default();
        }
        line
    }
}

/// Board graphic arc (`gr_arc`), legacy center/angle form normalized.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphicArc {
    pub start: Point,
    pub mid: Point,
    pub end: Point,
    pub layer: String,
    pub width: f64,
    pub uuid: String,
}

impl GraphicArc {
    pub fn from_sexp(sexp: &SExp) -> Self {
        let (start, mid, end) = arc_points_from_sexp(sexp);
        let mut arc = GraphicArc {
            start,
            mid,
            end,
            layer: String::new(),
            width: 0.1,
            uuid: String::new(),
        };
        if let Some(l) = sexp.find("layer") {
            arc.layer = gs(l, 0).unwrap_or_default();
        }
        if let Some(w) = sexp.find("width") {
            arc.width = gf_or(w, 0, 0.1);
        }
        if let Some(w) = sexp.find("stroke").and_then(|s| s.find("width")) {
            arc.width = gf_or(w, 0, 0.1);
        }
        if let Some(u) = sexp.find("uuid") {
            arc.uuid = gs(u, 0).unwrap_or_default();
        }
        arc
    }
}

/// Any board-level graphic item (Python `graphic_items` union).
#[derive(Debug, Clone, PartialEq)]
pub enum GraphicItem<'a> {
    Line(&'a GraphicLine),
    Arc(&'a GraphicArc),
    Other(&'a BoardGraphic),
}

/// Connected group of `Edge.Cuts` graphics (outline or small cutout).
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeContour {
    pub index: usize,
    pub element_count: usize,
    /// `(min_x, min_y, max_x, max_y)`, sheet-absolute.
    pub bbox: (f64, f64, f64, f64),
    pub is_mounting_hole: bool,
    /// Indices of the member nodes among the board root's children.
    pub node_indices: Vec<usize>,
}

impl EdgeContour {
    pub fn bbox_area(&self) -> f64 {
        self.bbox_width() * self.bbox_height()
    }
    pub fn bbox_width(&self) -> f64 {
        self.bbox.2 - self.bbox.0
    }
    pub fn bbox_height(&self) -> f64 {
        self.bbox.3 - self.bbox.1
    }
}

/// Physical stackup stratum.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StackupLayer {
    pub name: String,
    /// `copper`, `prepreg`, `core`, `Top Solder Mask`, ... (Python `type`).
    pub layer_type: String,
    pub thickness: f64,
    pub material: String,
    pub epsilon_r: f64,
    pub loss_tangent: f64,
}

/// Board setup / design rules.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Setup {
    pub stackup: Vec<StackupLayer>,
    pub pad_to_mask_clearance: f64,
    pub copper_finish: String,
    pub aux_axis_origin: Point,
    /// Board-default via tenting (`None` = token absent).
    pub tenting_front: Option<bool>,
    pub tenting_back: Option<bool>,
}

/// Legacy (pre-KiCad-6) board-declared `(net_class ...)` rules; dimensions
/// are `None` when not declared.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardNetClass {
    pub name: String,
    pub description: String,
    pub clearance: Option<f64>,
    pub trace_width: Option<f64>,
    pub via_dia: Option<f64>,
    pub via_drill: Option<f64>,
    pub uvia_dia: Option<f64>,
    pub uvia_drill: Option<f64>,
    pub diff_pair_width: Option<f64>,
    pub diff_pair_gap: Option<f64>,
    pub nets: Vec<String>,
}

/// Paper sizes `(width, height)` in mm, landscape.
pub const PAPER_SIZES: &[(&str, (f64, f64))] = &[
    ("A4", (297.0, 210.0)),
    ("A3", (420.0, 297.0)),
    ("A2", (594.0, 420.0)),
    ("A1", (841.0, 594.0)),
    ("A0", (1189.0, 841.0)),
    ("A", (279.4, 215.9)),
    ("B", (431.8, 279.4)),
    ("C", (558.8, 431.8)),
    ("D", (863.6, 558.8)),
    ("E", (1117.6, 863.6)),
];

/// Size of a named paper, if known.
pub fn paper_size(name: &str) -> Option<(f64, f64)> {
    PAPER_SIZES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
}
