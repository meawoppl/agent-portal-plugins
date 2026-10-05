//! KiCad text to stroke polylines.
//!
//! `kicad-strokes` lays out KiCad text (board and schematic text items, fields,
//! labels) with the NewStroke stroke font and returns plain polylines plus a
//! pen width, so 2D viewers never need a font engine. The glyph table is the
//! author's 2015 CC0 release (see [`glyphs`] and `docs/newstroke-provenance.md`).
//!
//! ```
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
mod layout;

pub use layout::{pen_width, ITALIC_TILT, LINE_PITCH, SCRIPT_SCALE, UNITS_PER_EM};

/// Eeschema's default text pen width (mm), used when a schematic text item
/// has no explicit thickness.
pub const SCH_DEFAULT_PEN: f64 = 0.1524;

/// Eeschema plots `(text ...)` items this far (mm) above their anchor, in
/// screen -y at every angle. Labels and fields have their own offsets.
pub const SCH_TEXT_OFFSET: f64 = 0.25;

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
///
/// # Metrics
///
/// Measured from `kicad-cli` 10.0.6 plots (see `tests/kicad_compare.rs`):
///
/// - One font unit is `size / 21` ([`UNITS_PER_EM`]); the cap height equals
///   the text height and glyph advances come from the NewStroke bearings.
/// - Baseline-to-baseline pitch is [`LINE_PITCH`]` × height × line_spacing`.
///   Each line is justified on its own.
/// - Overbars run 1.2776 × height above the baseline, inset 0.1 × width
///   from each end of the overbarred run.
/// - Sub/superscripts are drawn at [`SCRIPT_SCALE`] (0.8) size, their
///   baseline 0.1105 × height below / 0.2895 × height above the main
///   baseline. They don't nest: `^{a_{b}}` places `b` as a plain subscript.
/// - Italic shears by [`ITALIC_TILT`] (1/8). The pen width shifts left- and
///   right-justified lines inward by 0.658 × pen, and all text up by
///   0.052 × pen.
///
/// # Schematic text
///
/// The layout is the same in Eeschema, with three differences callers handle:
/// the default pen is [`SCH_DEFAULT_PEN`] (pass it as `thickness`), text is
/// always kept upright (set [`keep_upright`](Self::keep_upright)), and
/// `(text ...)` items are plotted [`SCH_TEXT_OFFSET`] mm above their anchor
/// (screen -y at every angle; subtract it from `pos[1]`).
#[derive(Clone, Debug, PartialEq)]
pub struct TextSpec {
    /// Text, possibly multi-line (`\n`), with KiCad markup: `~{overbar}`,
    /// `_{subscript}`, `^{superscript}`. A marker without a matching `}` is
    /// drawn literally. Tabs advance to the next tab stop (stops every
    /// 4 × width, at `4k − 5/21` widths, at least 9/21 width ahead).
    /// Characters outside the NewStroke table (U+0020..=U+2BFF) are drawn as
    /// the table's placeholder box.
    pub text: String,
    /// Anchor position (KiCad `(at x y)`).
    pub pos: [f64; 2],
    /// Glyph size `[width, height]` in mm. KiCad files write `(size h w)`,
    /// height first. Height is the cap height.
    pub size: [f64; 2],
    /// Stroke (pen) width. `<= 0` selects KiCad's PCB default
    /// (`width / 8`, or `width / 5` when [`bold`](Self::bold)). All
    /// pens, bold or not, are clamped to `min(width, height) / 4`, as KiCad
    /// plots them.
    pub thickness: f64,
    /// Rotation in degrees, counter-clockwise as seen on screen (KiCad
    /// convention, also in the y-down frame). Rotation is about `pos`.
    pub angle_deg: f64,
    /// Horizontal justification of each line about `pos`.
    pub justify_h: HJustify,
    /// Vertical justification of the whole block about `pos`.
    pub justify_v: VJustify,
    /// Mirror horizontally about the anchor, in the text's own frame, before
    /// rotation (KiCad `(justify mirror)`, back-layer text).
    pub mirror: bool,
    /// Italic: glyphs are sheared by [`ITALIC_TILT`].
    pub italic: bool,
    /// Bold. An explicit thickness is used as given (KiCad stores the bold pen
    /// width in the file); with `thickness <= 0` the pen is `width / 5`.
    pub bold: bool,
    /// Line spacing factor (KiCad `line_spacing`, default 1.0). Note that
    /// `kicad-cli` 10.0.6 plots stroke-font text with a factor of 1.0 even
    /// when the file sets another value; pass 1.0 to match its plots.
    pub line_spacing: f64,
    /// Keep the text readable: when the angle (mod 360) is in (90, 270], draw
    /// it at `angle - 180` with the same justification, as KiCad does for
    /// footprint fields and all schematic text.
    pub keep_upright: bool,
    /// Frame of `pos` and of the output. `true` (default): y grows down, as in
    /// KiCad files. `false`: y grows up; the result is the same picture with
    /// y negated.
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
    layout::layout(spec)
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

/// Code points whose 2015 NewStroke slot is the placeholder box but which
/// have a visually identical, CC0-drawn glyph elsewhere in the same table.
/// U+2126 OHM SIGN is canonically equivalent to U+03A9 GREEK CAPITAL OMEGA.
const ALIASES: &[(char, char)] = &[('\u{2126}', '\u{03A9}')];

/// The NewStroke glyph for `c` (after [`ALIASES`]), or `None` outside
/// U+0020..=U+2BFF.
pub fn glyph(c: char) -> Option<Glyph> {
    let c = ALIASES
        .iter()
        .find(|(from, _)| *from == c)
        .map_or(c, |(_, to)| *to);
    let i = (c as u32).checked_sub(glyphs::FIRST_CODE_POINT)? as usize;
    glyphs::GLYPHS.get(i).map(|s| decode_glyph(s))
}
