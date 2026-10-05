//! Text layout: markup parsing, line metrics, justification and transforms.
//!
//! Every metric here was measured from KiCad's own plotted output
//! (`kicad-cli pcb export svg`, KiCad 10.0.6), not taken from KiCad source.
//! See the crate tests for the fixtures and achieved tolerance.

use crate::{decode_glyph, glyphs, Glyph, HJustify, TextSpec, TextStrokes, VJustify};
use vector_view::{BBox, Point};

/// Font units per cap height: a glyph unit is `size / 21`.
pub const UNITS_PER_EM: f64 = 21.0;
/// Font-unit y of the glyph origin line (one unit above the baseline, which
/// is at font y = 9; the cap line is at font y = -12).
const ORIGIN_FY: f64 = 8.0;
/// Baseline-to-baseline pitch as a multiple of the height (times
/// `line_spacing`).
pub const LINE_PITCH: f64 = 1.6099;
/// Origin line of the first line for top justification, times height.
const TOP_ORIGIN: f64 = 1.0;
/// Origin line of the last line for bottom justification, times height.
const BOTTOM_ORIGIN: f64 = -0.17;
/// KiCad shifts a left (right) justified line right (left) by this many pen
/// widths.
const PEN_PAD_X: f64 = 0.6578;
/// KiCad shifts all text up by this many pen widths.
const PEN_SHIFT_Y: f64 = 0.0521;
/// Overbar height above the glyph origin line, times height.
const OVERBAR_RISE: f64 = 1.23;
/// Overbar inset from each end of the overbarred run, times width.
const OVERBAR_INSET: f64 = 0.1;
/// Sub/superscript size factor.
pub const SCRIPT_SCALE: f64 = 0.8;
/// Subscript origin drop below the line origin, times the text height.
const SUB_DROP: f64 = 0.12;
/// Superscript origin rise above the line origin, times the text height.
const SUPER_RISE: f64 = 0.28;
/// Italic shear (x shift per unit of height above the origin line).
pub const ITALIC_TILT: f64 = 1.0 / 8.0;
/// Tab stops sit at `TAB_PITCH * k - TAB_BACK` font units of width; a tab
/// advances to the first stop at least `TAB_MIN_GAP` units ahead.
const TAB_PITCH: f64 = 84.0;
const TAB_BACK: f64 = 5.0;
const TAB_MIN_GAP: f64 = 9.0;
/// Default pen widths when the thickness is unset, times width.
const DEFAULT_PEN: f64 = 1.0 / 8.0;
const DEFAULT_BOLD_PEN: f64 = 1.0 / 5.0;
/// Largest pen KiCad plots (bold or not), times the smaller size.
const MAX_PEN: f64 = 0.25;

/// Index into [`glyphs::GLYPHS`] of the box drawn for unknown characters
/// (the 2015 table's own placeholder, U+2BFF).
const PLACEHOLDER: usize = 0x2BFF - glyphs::FIRST_CODE_POINT as usize;

/// Pen width KiCad uses for `spec`: the explicit thickness, else the
/// default for the size, clamped to `min(width, height) / 4`.
pub fn pen_width(spec: &TextSpec) -> f64 {
    let [w, h] = spec.size;
    let pen = if spec.thickness > 0.0 {
        spec.thickness
    } else if spec.bold {
        w.abs() * DEFAULT_BOLD_PEN
    } else {
        w.abs() * DEFAULT_PEN
    };
    pen.min(MAX_PEN * w.abs().min(h.abs()))
}

#[derive(Clone, Copy, Default)]
struct Style {
    scale: f64,
    /// Origin-line offset (local y-down, mm) relative to the line origin.
    dy: f64,
    overbar: bool,
}

enum Item {
    Glyph(char, Style),
    Tab,
}

/// Splits one line into styled items, honouring `~{}`, `_{}` and `^{}`.
/// Markup without a matching `}` is drawn literally.
fn parse_line(line: &str, h: f64) -> Vec<Item> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut stack: Vec<Style> = vec![Style {
        scale: 1.0,
        dy: 0.0,
        overbar: false,
    }];
    // Matching close brace of each opener, computed up front.
    let mut close_of = vec![None; chars.len()];
    {
        let mut opens = Vec::new();
        for (i, &c) in chars.iter().enumerate() {
            if c == '{' && i > 0 && matches!(chars[i - 1], '~' | '_' | '^') {
                opens.push(i);
            } else if c == '}' {
                if let Some(o) = opens.pop() {
                    close_of[o] = Some(i);
                }
            }
        }
    }
    let mut is_close = vec![false; chars.len()];
    for c in close_of.iter().flatten() {
        is_close[*c] = true;
    }
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let cur = *stack.last().unwrap();
        if matches!(c, '~' | '_' | '^') && i + 1 < chars.len() && close_of[i + 1].is_some() {
            // KiCad does not nest scripts: an inner `_{}`/`^{}` is placed
            // relative to the main line, at the same 0.8 size.
            let mut s = cur;
            match c {
                '~' => s.overbar = true,
                '_' => {
                    s.scale = SCRIPT_SCALE;
                    s.dy = SUB_DROP * h;
                }
                _ => {
                    s.scale = SCRIPT_SCALE;
                    s.dy = -SUPER_RISE * h;
                }
            }
            stack.push(s);
            i += 2;
            continue;
        }
        if is_close[i] && stack.len() > 1 {
            stack.pop();
            i += 1;
            continue;
        }
        match c {
            '\t' => out.push(Item::Tab),
            c if (c as u32) < 0x20 => {}
            c => out.push(Item::Glyph(c, cur)),
        }
        i += 1;
    }
    out
}

fn glyph_for(c: char) -> Glyph {
    match crate::glyph(c) {
        Some(g) => g,
        None => decode_glyph(glyphs::GLYPHS[PLACEHOLDER]),
    }
}

/// A laid-out line in line-local coordinates (x from the line start, y
/// relative to the line origin, y down).
struct Line {
    strokes: Vec<Vec<Point>>,
    width: f64,
}

fn layout_line(items: &[Item], spec: &TextSpec) -> Line {
    let [w, h] = spec.size;
    let ux = w / UNITS_PER_EM;
    let uy = h / UNITS_PER_EM;
    let tilt = if spec.italic { ITALIC_TILT } else { 0.0 };
    let mut x = 0.0;
    let mut strokes = Vec::new();
    let mut bar: Option<(f64, f64, Style)> = None; // (start x, y, style)
    let mut bars: Vec<Vec<Point>> = Vec::new();
    let finish_bar =
        |bar: &mut Option<(f64, f64, Style)>, x_end: f64, bars: &mut Vec<Vec<Point>>| {
            if let Some((x0, y, st)) = bar.take() {
                let inset = OVERBAR_INSET * w * st.scale;
                let (a, b) = (x0 + inset, x_end - inset);
                if b > a {
                    bars.push(vec![[a, y], [b, y]]);
                }
            }
        };
    for it in items {
        match it {
            Item::Tab => {
                finish_bar(&mut bar, x, &mut bars);
                let u = x / ux + TAB_MIN_GAP + TAB_BACK;
                x = (((u - 1e-6) / TAB_PITCH).ceil() * TAB_PITCH - TAB_BACK) * ux;
            }
            Item::Glyph(c, st) => {
                if !st.overbar {
                    finish_bar(&mut bar, x, &mut bars);
                }
                let g = glyph_for(*c);
                let sx = ux * st.scale;
                let sy = uy * st.scale;
                if st.overbar && bar.is_none() {
                    let y = st.dy - OVERBAR_RISE * h * st.scale;
                    bar = Some((x, y, *st));
                }
                for s in &g.strokes {
                    strokes.push(
                        s.iter()
                            .map(|&[fx, fy]| {
                                let px = x + (fx - g.left) as f64 * sx;
                                let ly = (fy as f64 - ORIGIN_FY) * sy;
                                [px - ly * tilt, st.dy + ly]
                            })
                            .collect(),
                    );
                }
                x += (g.right - g.left) as f64 * sx;
            }
        }
    }
    finish_bar(&mut bar, x, &mut bars);
    // Overbars sit at a fixed height, sheared like the glyphs.
    for b in &mut bars {
        for p in b.iter_mut() {
            p[0] -= p[1] * tilt;
        }
    }
    strokes.extend(bars);
    Line { strokes, width: x }
}

pub(crate) fn layout(spec: &TextSpec) -> TextStrokes {
    let width = pen_width(spec);
    let [_, h] = spec.size;
    let lines: Vec<Line> = spec
        .text
        .split('\n')
        .map(|l| layout_line(&parse_line(l.trim_end_matches('\r'), h), spec))
        .collect();

    let mut angle = spec.angle_deg.rem_euclid(360.0);
    let (jh, jv) = (spec.justify_h, spec.justify_v);
    // KiCad keeps the justification: the text is drawn as at `angle - 180`
    // (footprint fields and all schematic text behave this way).
    if spec.keep_upright && angle > 90.0 && angle <= 270.0 {
        angle -= 180.0;
    }

    let pitch = LINE_PITCH * h * spec.line_spacing;
    let n = lines.len() as f64;
    let top = TOP_ORIGIN * h;
    let bottom = BOTTOM_ORIGIN * h - (n - 1.0) * pitch;
    let first = match jv {
        VJustify::Top => top,
        VJustify::Bottom => bottom,
        VJustify::Center => (top + bottom) / 2.0,
    } - PEN_SHIFT_Y * width;
    let pad = PEN_PAD_X * width;

    let (sin, cos) = angle.to_radians().sin_cos();
    let mirror = if spec.mirror { -1.0 } else { 1.0 };
    let [px, py] = spec.pos;
    let py = if spec.y_down { py } else { -py };
    let flip = if spec.y_down { 1.0 } else { -1.0 };

    let mut out = TextStrokes {
        strokes: Vec::new(),
        width,
        bbox: BBox::EMPTY,
    };
    for (i, line) in lines.into_iter().enumerate() {
        let x0 = match jh {
            HJustify::Left => pad,
            HJustify::Center => -line.width / 2.0,
            HJustify::Right => -line.width - pad,
        };
        let y0 = first + i as f64 * pitch;
        for s in line.strokes {
            let pts: Vec<Point> = s
                .into_iter()
                .map(|[x, y]| {
                    let lx = (x + x0) * mirror;
                    let ly = y + y0;
                    // Counter-clockwise on screen in a y-down frame.
                    let rx = lx * cos + ly * sin;
                    let ry = -lx * sin + ly * cos;
                    [px + rx, (py + ry) * flip]
                })
                .collect();
            for p in &pts {
                out.bbox.include_padded(*p, width / 2.0);
            }
            out.strokes.push(pts);
        }
    }
    out
}
