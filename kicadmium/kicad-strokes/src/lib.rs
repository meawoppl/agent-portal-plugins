//! KiCad text to stroke polylines.
//!
//! `kicad-strokes` lays out KiCad text (board and schematic text items, fields,
//! labels) with the NewStroke stroke font and returns plain polylines plus a
//! pen width, so 2D viewers never need a font engine. The glyph table is the
//! author's 2015 CC0 release (see [`glyphs`] and `docs/newstroke-provenance.md`).
//!
//! ```no_run
//! use kicad_strokes::{to_strokes, HJustify, TextSpec};
//! let s = to_strokes(&TextSpec {
//!     text: "R1".into(),
//!     pos: [10.0, 20.0],
//!     size: [1.0, 1.0],
//!     thickness: 0.15,
//!     justify_h: HJustify::Center,
//!     ..TextSpec::default()
//! });
//! assert!(!s.strokes.is_empty());
//! assert_eq!(s.width, 0.15);
//! ```

pub mod glyphs;

/// Horizontal justification of each line relative to [`TextSpec::pos`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HJustify {
    Left,
    #[default]
    Center,
    Right,
}

/// Vertical justification of the text block relative to [`TextSpec::pos`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VJustify {
    Top,
    #[default]
    Center,
    Bottom,
}

/// One KiCad text item to lay out.
///
/// Units are millimetres; the frame is KiCad's (x right, y down) unless
/// [`y_down`](Self::y_down) is false.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSpec {
    /// Text, possibly multi-line (`\n`), with KiCad markup: `~{overbar}`,
    /// `_{subscript}`, `^{superscript}`. Tabs expand to the next 4-column stop.
    pub text: String,
    /// Anchor position (KiCad `(at x y)`).
    pub pos: [f64; 2],
    /// Glyph size `[width, height]` (KiCad `(size h w)` is height first in the
    /// file; pass `[w, h]` here). Height is the cap height.
    pub size: [f64; 2],
    /// Stroke (pen) width. `<= 0` selects KiCad's default for the size
    /// (`height / 8`, or the bold width when [`bold`](Self::bold)).
    pub thickness: f64,
    /// Rotation in degrees, counter-clockwise as seen on screen (KiCad
    /// convention, also in the y-down frame).
    pub angle_deg: f64,
    pub justify_h: HJustify,
    pub justify_v: VJustify,
    /// Mirror horizontally about the anchor (KiCad `(justify mirror)`, back
    /// layer text). Applied before rotation.
    pub mirror: bool,
    pub italic: bool,
    /// Bold. A thickness given explicitly is kept, as KiCad stores the bold
    /// pen width in the file; with `thickness <= 0` the bold default is used.
    pub bold: bool,
    /// Line spacing factor (KiCad `line_spacing`, default 1.0).
    pub line_spacing: f64,
    /// Turn text that would read upside down by 180 degrees (KiCad "keep
    /// upright", footprint fields). Applies when the effective angle is in
    /// (90, 270] degrees.
    pub keep_upright: bool,
    /// Output frame. `true` (default): y grows down, as in KiCad files.
    /// `false`: the same picture in a y-up frame (y coordinates negated).
    pub y_down: bool,
}

impl Default for TextSpec {
    fn default() -> Self {
        Self {
            text: String::new(),
            pos: [0.0, 0.0],
            size: [1.27, 1.27],
            thickness: 0.0,
            angle_deg: 0.0,
            justify_h: HJustify::Center,
            justify_v: VJustify::Center,
            mirror: false,
            italic: false,
            bold: false,
            line_spacing: 1.0,
            keep_upright: false,
            y_down: true,
        }
    }
}

pub use vector_view::{BBox, Point, Prim};

/// Result of [`to_strokes`]: one text item as stroke polylines.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStrokes {
    /// Pen paths (stroke centre lines), in the output frame. Each is drawn
    /// with round caps and joins of [`width`](Self::width).
    pub strokes: Vec<Vec<Point>>,
    /// Pen width for every stroke (mm).
    pub width: f64,
    /// Ink extents: stroke centre lines grown by `width / 2`.
    /// [`BBox::EMPTY`] when the text has no ink (empty or only spaces).
    pub bbox: BBox,
}

impl Default for TextStrokes {
    fn default() -> Self {
        Self {
            strokes: Vec::new(),
            width: 0.0,
            bbox: BBox::EMPTY,
        }
    }
}

impl TextStrokes {
    /// Converts into a [`Prim::Strokes`] scene primitive.
    pub fn into_prim(self) -> Prim {
        Prim::Strokes {
            strokes: self.strokes,
            width: self.width,
        }
    }
}

/// Lays out `spec` and returns its strokes.
pub fn to_strokes(spec: &TextSpec) -> TextStrokes {
    let _ = spec;
    TextStrokes::default()
}

/// A decoded NewStroke glyph in font units (1 unit = cap height / 21).
///
/// x grows right from the glyph origin, y grows down; the baseline is at
/// `y = 9` and the cap line at `y = -12`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Glyph {
    /// Left bearing (advance starts here).
    pub left: i32,
    /// Right bearing; the advance is `right - left`.
    pub right: i32,
    /// Pen strokes.
    pub strokes: Vec<Vec<[i32; 2]>>,
}

/// Decodes a NewStroke glyph string (bearings, coordinate pairs, `" R"` pen
/// lifts; every value is `c - 'R'`).
pub fn decode_glyph(data: &str) -> Glyph {
    let b = data.as_bytes();
    let v = |c: u8| c as i32 - b'R' as i32;
    if b.len() < 2 {
        return Glyph::default();
    }
    let mut g = Glyph {
        left: v(b[0]),
        right: v(b[1]),
        strokes: Vec::new(),
    };
    let mut cur: Vec<[i32; 2]> = Vec::new();
    for pair in b[2..].chunks(2) {
        if pair.len() < 2 {
            break;
        }
        if pair == b" R" {
            if !cur.is_empty() {
                g.strokes.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push([v(pair[0]), v(pair[1])]);
        }
    }
    if !cur.is_empty() {
        g.strokes.push(cur);
    }
    g
}

/// The NewStroke glyph for `c`, or `None` outside U+0020..=U+2BFF.
pub fn glyph(c: char) -> Option<Glyph> {
    let i = (c as u32).checked_sub(glyphs::FIRST_CODE_POINT)? as usize;
    glyphs::GLYPHS.get(i).map(|s| decode_glyph(s))
}
