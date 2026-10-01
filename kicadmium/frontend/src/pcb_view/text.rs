//! KiCad text drawn with Canvas2D `fillText` in a system monospace font.
//!
//! kicadmium ships no KiCad font data, so glyph shapes and advance widths
//! differ from KiCad's stroke font (see `docs/javascript-triage.md`). What
//! is honoured: anchor, rotation, mirror, horizontal/vertical justification,
//! multi-line spacing, the KiCad `(width, height)` size (cap height = text
//! height, glyphs stretched to the width/height ratio), stroke thickness,
//! italic, and the `~{overbar}` / `_{sub}` / `^{super}` markup. Line
//! placement follows KiCad's `FONT::getLinePositions` metrics.

use shared::pcb::PcbText;
use web_sys::CanvasRenderingContext2d;

/// Nominal font size the text is laid out at before scaling to board units
/// (tiny pixel fonts render badly).
const NOMINAL_PX: f64 = 100.0;
/// Cap height of the system monospace font as a fraction of the em.
const CAP_RATIO: f64 = 0.72;
/// Stem width the font already draws, as a fraction of the cap height.
const FONT_STEM: f64 = 0.12;
const FONT_FAMILY: &str = "ui-monospace, \"DejaVu Sans Mono\", Menlo, Consolas, monospace";
const INTERLINE_PITCH: f64 = 1.62;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Normal,
    Overbar,
    Sub,
    Sup,
}

/// Split one line of KiCad markup into styled runs (groups do not nest
/// style; inner groups take the innermost style).
fn runs(line: &str) -> Vec<(String, Kind)> {
    let mut out: Vec<(String, Kind)> = Vec::new();
    let mut stack = vec![Kind::Normal];
    let mut buf = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let flush = |buf: &mut String, out: &mut Vec<(String, Kind)>, kind: Kind| {
        if !buf.is_empty() {
            out.push((std::mem::take(buf), kind));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let top = *stack.last().unwrap();
        if matches!(c, '~' | '_' | '^') && chars.get(i + 1) == Some(&'{') {
            flush(&mut buf, &mut out, top);
            stack.push(match c {
                '~' => Kind::Overbar,
                '_' => Kind::Sub,
                _ => Kind::Sup,
            });
            i += 2;
            continue;
        }
        if c == '}' && stack.len() > 1 {
            flush(&mut buf, &mut out, top);
            stack.pop();
            i += 1;
            continue;
        }
        if c == '\t' {
            buf.push_str("    ");
        } else {
            buf.push(c);
        }
        i += 1;
    }
    let top = *stack.last().unwrap();
    flush(&mut buf, &mut out, top);
    out
}

fn font(px: f64, italic: bool) -> String {
    format!(
        "{}{px}px {FONT_FAMILY}",
        if italic { "italic " } else { "" }
    )
}

fn script_scale(kind: Kind) -> f64 {
    if matches!(kind, Kind::Sub | Kind::Sup) {
        0.7
    } else {
        1.0
    }
}

/// Vertical offset of a run's baseline, in nominal units (`h` = cap height).
fn script_shift(kind: Kind, h: f64) -> f64 {
    match kind {
        Kind::Sub => h * 0.7 * 0.3,
        Kind::Sup => -h * 0.7 * 0.5,
        _ => 0.0,
    }
}

/// Baseline (nominal units, relative to the anchor) of line `i` of `n`.
fn baseline(v_align: &str, h: f64, interline: f64, n: usize, i: usize) -> f64 {
    // KiCad: the block is `1.17 h + (n-1) interline` tall; glyph bases sit
    // 0.05 h above the nominal baseline.
    let block = h * 1.17 + n.saturating_sub(1) as f64 * interline;
    let shift = match v_align {
        "top" => 0.0,
        "bottom" => block,
        _ => block / 2.0,
    };
    h - shift - 0.05 * h + i as f64 * interline
}

fn line_width(ctx: &CanvasRenderingContext2d, runs: &[(String, Kind)], italic: bool) -> f64 {
    runs.iter()
        .map(|(text, kind)| {
            ctx.set_font(&font(NOMINAL_PX * script_scale(*kind), italic));
            ctx.measure_text(text).map(|m| m.width()).unwrap_or(0.0)
        })
        .sum()
}

/// Draw `texts` in the current (board) transform with `color`.
pub fn draw_texts(ctx: &CanvasRenderingContext2d, texts: &[PcbText], color: &str) {
    if texts.is_empty() {
        return;
    }
    ctx.set_fill_style_str(color);
    ctx.set_stroke_style_str(color);
    ctx.set_text_baseline("alphabetic");
    ctx.set_text_align("left");
    for t in texts {
        draw_text(ctx, t);
    }
}

fn draw_text(ctx: &CanvasRenderingContext2d, t: &PcbText) {
    let (w, h) = (t.size[0], t.size[1]);
    if h <= 0.0 || w <= 0.0 {
        return;
    }
    // Board mm per nominal unit, vertically and horizontally.
    let k = h / (CAP_RATIO * NOMINAL_PX);
    let kx = k * (w / h);
    let cap = CAP_RATIO * NOMINAL_PX;
    let interline = cap * INTERLINE_PITCH * t.line_spacing.max(0.1);
    ctx.save();
    let _ = ctx.translate(t.pos[0], t.pos[1]);
    if t.angle != 0.0 {
        let _ = ctx.rotate(-t.angle.to_radians());
    }
    if t.mirrored {
        let _ = ctx.scale(-1.0, 1.0);
    }
    let _ = ctx.scale(kx, k);
    // Thicken the font's own stems up to the KiCad stroke width.
    let extra = (t.thickness - FONT_STEM * h).max(0.0) / k;
    ctx.set_line_width(extra.max(0.0));
    ctx.set_line_join("round");
    let lines: Vec<Vec<(String, Kind)>> = t.text.split('\n').map(runs).collect();
    let n = lines.len();
    for (i, line) in lines.iter().enumerate() {
        let width = line_width(ctx, line, t.italic);
        let mut x = match t.h_align.as_str() {
            "left" => 0.0,
            "right" => -width,
            _ => -width / 2.0,
        };
        let y = baseline(&t.v_align, cap, interline, n, i);
        for (text, kind) in line {
            let s = script_scale(*kind);
            ctx.set_font(&font(NOMINAL_PX * s, t.italic));
            let run_w = ctx.measure_text(text).map(|m| m.width()).unwrap_or(0.0);
            let ry = y + script_shift(*kind, cap);
            let _ = ctx.fill_text(text, x, ry);
            if extra > 0.0 {
                let _ = ctx.stroke_text(text, x, ry);
            }
            if *kind == Kind::Overbar {
                let bar = ry - cap * 1.25;
                let inset = NOMINAL_PX * 0.05;
                ctx.begin_path();
                ctx.move_to(x + inset, bar);
                ctx.line_to(x + run_w - inset, bar);
                ctx.set_line_width((t.thickness / k).max(1.0));
                ctx.stroke();
                ctx.set_line_width(extra.max(0.0));
            }
            x += run_w;
        }
    }
    ctx.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_runs() {
        assert_eq!(runs("AB"), vec![("AB".into(), Kind::Normal)]);
        assert_eq!(
            runs("~{RST} V_{CC}"),
            vec![
                ("RST".into(), Kind::Overbar),
                (" V".into(), Kind::Normal),
                ("CC".into(), Kind::Sub)
            ]
        );
        // Unbalanced or literal braces stay text.
        assert_eq!(runs("a}b"), vec![("a}b".into(), Kind::Normal)]);
    }

    #[test]
    fn baselines_follow_kicad_alignment() {
        let (h, il) = (1.0, 1.62);
        // Single centred line: glyphs roughly centred on the anchor.
        let b = baseline("center", h, il, 1, 0);
        assert!((b - (h - 0.585 - 0.05)).abs() < 1e-9);
        assert!(baseline("top", h, il, 1, 0) > b);
        assert!(baseline("bottom", h, il, 1, 0) < b);
        assert!((baseline("top", h, il, 2, 1) - baseline("top", h, il, 2, 0) - il).abs() < 1e-9);
    }
}
