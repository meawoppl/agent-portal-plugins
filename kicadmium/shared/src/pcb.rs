//! `/api/kicad/pcbview`: a render-ready board model for the Rust PCB view.
//!
//! The backend builds it from `kct::schema::pcb` (the one KiCad parser in the
//! workspace). Geometry is fully resolved: millimetres, KiCad's sheet frame
//! (y down), footprint transforms applied. Text is sent as KiCad text plus
//! its effects; the view lays it out with a system font (no KiCad font data
//! is shipped). Layers, nets and footprints are referenced by index.
//!
//! THIRD-PARTY PROVENANCE: the shape/pad/track/zone model is derived from
//! pastebom's `crates/viewer/src/pcbdata.rs` (github.com/meawoppl/pastebom.com,
//! same author). Its author explicitly authorized the listed derived parts
//! under kicadmium's MIT license. See `docs/third-party.md` for provenance.

use serde::{Deserialize, Serialize};

pub type Pt = [f64; 2];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PcbViewResponse {
    pub ok: bool,
    /// Source revision the board was read at (`/api/kicad/revision`).
    pub revision: String,
    /// Project-relative board path.
    pub filename: String,
    pub board: PcbBoard,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PcbBoard {
    pub title: String,
    /// `[minx, miny, maxx, maxy]` of `Edge.Cuts` (all geometry if no outline).
    pub bbox: [f64; 4],
    pub layers: Vec<PcbLayer>,
    /// Net names; index 0 is the unconnected net "".
    pub nets: Vec<String>,
    pub footprints: Vec<PcbFootprint>,
    pub tracks: Vec<PcbTrack>,
    pub vias: Vec<PcbVia>,
    pub zones: Vec<PcbZone>,
    /// Board-level graphics (`gr_*`), including `Edge.Cuts`.
    pub graphics: Vec<PcbGraphic>,
    pub texts: Vec<PcbText>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PcbLayer {
    /// Canonical KiCad name (`F.Cu`, `B.SilkS`, `User.1`, ...).
    pub name: String,
    /// `copper` for `*.Cu`, otherwise the board's layer type (`user`, ...).
    pub kind: String,
    /// User-facing alias from the `(layers ...)` table, when set.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alias: String,
    /// Not in the board's layer table (KiCad does not show it); items
    /// on it are still sent so the view can opt in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

/// Geometry in board coordinates (mm). Arc angles are radians in the y-down
/// frame; the arc sweeps from `start` to `end` with increasing angle (canvas
/// `arc(.., false)` order).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum PcbShape {
    Segment {
        a: Pt,
        b: Pt,
    },
    Arc {
        c: Pt,
        r: f64,
        start: f64,
        end: f64,
    },
    Circle {
        c: Pt,
        r: f64,
    },
    /// Closed outline.
    Polygon {
        pts: Vec<Pt>,
    },
    /// Open path (flattened Béziers).
    Polyline {
        pts: Vec<Pt>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PcbGraphic {
    pub layer: u16,
    pub shape: PcbShape,
    /// Stroke width; 0 draws no outline.
    pub width: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub filled: bool,
}

/// KiCad text item (`gr_text`, `fp_text`, or a footprint `property`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PcbText {
    pub layer: u16,
    /// Shown text (`${REFERENCE}`/`${VALUE}` resolved); KiCad markup such as
    /// `~{overbar}`, `_{sub}`, `^{super}` and `\n` line breaks kept as-is.
    pub text: String,
    /// `reference`, `value`, `user`, or `board`.
    pub kind: String,
    /// Anchor (board coordinates).
    pub pos: Pt,
    /// Draw angle in degrees, KiCad sense (counter-clockwise on screen),
    /// already normalised for footprint keep-upright.
    pub angle: f64,
    /// KiCad font `(width, height)` in mm.
    pub size: Pt,
    /// KiCad stroke thickness in mm.
    pub thickness: f64,
    /// `left`, `center` or `right`.
    pub h_align: String,
    /// `top`, `center` or `bottom`.
    pub v_align: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mirrored: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    /// KiCad line-spacing multiplier (1.0 default).
    pub line_spacing: f64,
    /// `false` for KiCad-hidden text (sent for completeness, not drawn).
    pub visible: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PcbFootprint {
    pub reference: String,
    pub value: String,
    /// Library id (`Resistor_SMD:R_0805_2012Metric`).
    pub footprint: String,
    /// `F.Cu` or `B.Cu` index.
    pub layer: u16,
    pub pos: Pt,
    /// Degrees, KiCad sense (counter-clockwise on screen).
    pub angle: f64,
    /// Oriented bounding box of pads and graphics (4 corners).
    pub bbox: Vec<Pt>,
    pub pads: Vec<PcbPad>,
    pub graphics: Vec<PcbGraphic>,
    pub texts: Vec<PcbText>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// `smd`, `through_hole` or "".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attr: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dnp: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PcbPad {
    pub number: String,
    /// `smd`, `thru_hole`, `np_thru_hole`, `connect`.
    pub kind: String,
    /// `rect`, `roundrect`, `circle`, `oval`, `trapezoid` or `custom`
    /// (chamfers ride on `rect`/`roundrect` via `chamfer`).
    pub shape: String,
    /// Hole position (board).
    pub pos: Pt,
    /// Absolute pad angle, degrees counter-clockwise.
    pub angle: f64,
    pub size: Pt,
    /// Expanded layer indices (`*.Cu` resolved against the stack).
    pub layers: Vec<u16>,
    pub net: u32,
    /// Shape centre relative to the hole, in the pad frame.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub offset: Pt,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rratio: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub chamfer_ratio: f64,
    /// Chamfered corners: 1 top-left, 2 top-right, 4 bottom-left, 8 bottom-right.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub chamfer: u8,
    /// Trapezoid `rect_delta`.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub delta: Pt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drill: Option<PcbDrill>,
    /// Custom-pad primitives in the pad frame (centre at the shape centre,
    /// unrotated); the anchor shape is `custom_anchor`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<PcbGraphic>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub custom_anchor: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pin_function: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PcbDrill {
    pub oval: bool,
    /// `[w, h]` (both the diameter for round holes), pad frame.
    pub size: Pt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PcbTrack {
    pub layer: u16,
    pub net: u32,
    pub width: f64,
    /// `Segment` or `Arc`.
    pub shape: PcbShape,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PcbVia {
    pub pos: Pt,
    pub size: f64,
    pub drill: f64,
    /// Every copper layer the via spans.
    pub layers: Vec<u16>,
    pub net: u32,
    /// `through`, `blind`, `buried` or `micro`.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PcbZone {
    pub net: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub layers: Vec<u16>,
    pub outline: Vec<Pt>,
    pub fills: Vec<PcbZoneFill>,
    /// Rule area (keepout).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keepout: bool,
    pub priority: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PcbZoneFill {
    pub layer: u16,
    pub pts: Vec<Pt>,
    /// Legacy stroked fills: outline width that completes the copper.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stroke: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn is_origin(v: &Pt) -> bool {
    *v == [0.0, 0.0]
}
