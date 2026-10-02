//! Layout behaviour: justification, rotation, mirroring, lines, markup.
//!
//! Expected numbers are KiCad 10.0.6 plot measurements for a 1 mm "H" with a
//! 0.1 mm pen (see `kicad_compare.rs` for the full fixture comparison).

use kicad_strokes::{pen_width, to_strokes, BBox, HJustify, Prim, TextSpec, VJustify, LINE_PITCH};

const EPS: f64 = 1e-3;

fn h(text: &str, jh: HJustify, jv: VJustify) -> TextSpec {
    TextSpec {
        text: text.into(),
        pos: [10.0, 20.0],
        size: [1.0, 1.0],
        thickness: 0.1,
        justify_h: jh,
        justify_v: jv,
        ..TextSpec::default()
    }
}

/// Centre-line extents relative to the anchor: [min x, min y, max x, max y].
fn ext(s: &TextSpec) -> [f64; 4] {
    let r = to_strokes(s);
    let mut b = BBox::EMPTY;
    for p in r.strokes.iter().flatten() {
        b.include(*p);
    }
    [
        b.min[0] - s.pos[0],
        b.min[1] - s.pos[1],
        b.max[0] - s.pos[0],
        b.max[1] - s.pos[1],
    ]
}

fn close(a: [f64; 4], b: [f64; 4]) {
    for i in 0..4 {
        assert!((a[i] - b[i]).abs() < EPS, "{a:?} != {b:?}");
    }
}

#[test]
fn justification_matches_kicad() {
    use HJustify as H;
    use VJustify as V;
    // x ranges for left / center / right, y ranges for top / center / bottom.
    let xs = [(0.3039, 0.8753), (-0.2857, 0.2857), (-0.8753, -0.3039)];
    let ys = [(0.0424, 1.0424), (-0.5426, 0.4574), (-1.1276, -0.1276)];
    for (jh, x) in [H::Left, H::Center, H::Right].into_iter().zip(xs) {
        for (jv, y) in [V::Top, V::Center, V::Bottom].into_iter().zip(ys) {
            close(ext(&h("H", jh, jv)), [x.0, y.0, x.1, y.1]);
        }
    }
}

#[test]
fn rotation_is_counter_clockwise_on_screen() {
    let mut s = h("H", HJustify::Left, VJustify::Bottom);
    let e0 = ext(&s);
    for (a, f) in [
        (90.0, [e0[1], -e0[2], e0[3], -e0[0]]),
        (180.0, [-e0[2], -e0[3], -e0[0], -e0[1]]),
        (270.0, [-e0[3], e0[0], -e0[1], e0[2]]),
    ] {
        s.angle_deg = a;
        close(ext(&s), f);
    }
    // 90 degrees: the text runs up the screen (towards -y).
    s.angle_deg = 90.0;
    let e = ext(&s);
    assert!(e[1] < -0.8 && e[3] < 0.0);
}

#[test]
fn mirror_flips_about_the_anchor_before_rotation() {
    let mut s = h("Hg1", HJustify::Left, VJustify::Bottom);
    let e = ext(&s);
    s.mirror = true;
    close(ext(&s), [-e[2], e[1], -e[0], e[3]]);
    // Rotated: mirrored in the text frame, so the text still runs along the
    // rotated baseline but towards the other side of the anchor.
    s.angle_deg = 90.0;
    let m = ext(&s);
    s.mirror = false;
    let n = ext(&s);
    close(m, [n[0], -n[3], n[2], -n[1]]);
}

#[test]
fn multiline_pitch_and_per_line_justification() {
    let s = h("H\nHHH", HJustify::Right, VJustify::Top);
    let r = to_strokes(&s);
    // 3 strokes per H: line 0 is r.strokes[0..3], line 1 is [3..12].
    let first_base = r.strokes[0][0][1];
    let second_base = r.strokes[3][0][1];
    assert!((second_base - first_base - LINE_PITCH).abs() < EPS);
    // Both lines end at the same right edge.
    let right = |ss: &[Vec<[f64; 2]>]| {
        ss.iter()
            .flatten()
            .map(|p| p[0])
            .fold(f64::NEG_INFINITY, f64::max)
    };
    assert!((right(&r.strokes[0..3]) - right(&r.strokes[3..])).abs() < EPS);
    // Center: the block is centred on the anchor (cap top of line 0 to the
    // baseline of line 1, shifted by KiCad's pen offset).
    let c = ext(&h("H\nH", HJustify::Center, VJustify::Center));
    assert!(((c[1] + c[3]) / 2.0 + 0.0426).abs() < EPS, "{c:?}");
    // line_spacing scales the pitch.
    let mut s = h("H\nH", HJustify::Left, VJustify::Top);
    s.line_spacing = 1.5;
    let r = to_strokes(&s);
    assert!((r.strokes[3][0][1] - r.strokes[0][0][1] - 1.5 * LINE_PITCH).abs() < EPS);
}

#[test]
fn overbar_sits_above_the_cap_line() {
    let plain = ext(&h("RST", HJustify::Left, VJustify::Bottom));
    let s = h("~{RST}", HJustify::Left, VJustify::Bottom);
    let r = to_strokes(&s);
    assert_eq!(
        r.strokes.len(),
        to_strokes(&h("RST", HJustify::Left, VJustify::Bottom))
            .strokes
            .len()
            + 1
    );
    let bar = r.strokes.last().unwrap();
    let baseline = -0.1276;
    assert!((bar[0][1] - s.pos[1] - (baseline - 1.2776)).abs() < EPS);
    assert!(bar[0][1] - s.pos[1] < plain[1]);
    // Inset 0.1 mm (0.1 x width) from each end of the run.
    let start = 0.0658;
    assert!((bar[0][0] - s.pos[0] - (start + 0.1)).abs() < EPS);
}

#[test]
fn sub_and_superscript_scale_and_offset() {
    let s = h("H_{H}", HJustify::Left, VJustify::Bottom);
    let r = to_strokes(&s);
    let sub = &r.strokes[3]; // left stem of the subscript H
    let height = sub[0][1] - sub[1][1];
    assert!((height - 0.8).abs() < EPS);
    assert!((sub[0][1] - s.pos[1] - (-0.1276 + 0.1105)).abs() < EPS);
    let s = h("H^{H}", HJustify::Left, VJustify::Bottom);
    let sup = &to_strokes(&s).strokes[3];
    assert!((sup[0][1] - s.pos[1] - (-0.1276 - 0.2895)).abs() < EPS);
    // Not nested: z in x^{y_{z}} is an ordinary subscript.
    let a = to_strokes(&h("x^{y_{z}}", HJustify::Left, VJustify::Bottom));
    let b = to_strokes(&h("x^{y}_{z}", HJustify::Left, VJustify::Bottom));
    assert_eq!(a.strokes, b.strokes);
}

#[test]
fn unmatched_markup_is_literal() {
    let a = to_strokes(&h("_{A", HJustify::Left, VJustify::Bottom));
    let n = |t: &str| {
        to_strokes(&h(t, HJustify::Left, VJustify::Bottom))
            .strokes
            .len()
    };
    assert_eq!(a.strokes.len(), n("_") + n("{") + n("A"));
    assert_eq!(n("A{B}C"), n("A") + n("{") + n("B") + n("}") + n("C"));
}

#[test]
fn tabs_advance_to_kicad_stops() {
    // Stops at 4k - 5/21 widths; the glyph after "\t" starts at 79/21.
    let x = |t: &str| {
        let s = h(t, HJustify::Left, VJustify::Bottom);
        let r = to_strokes(&s);
        r.strokes[r.strokes.len() - 3][0][0] - s.pos[0] // left stem of the last H
    };
    let h_stem = 0.0658 + 5.0 / 21.0; // pen offset + H's stem inset
    assert!((x("\tH") - (79.0 / 21.0 + h_stem)).abs() < EPS);
    assert!((x("\t\tH") - (163.0 / 21.0 + h_stem)).abs() < EPS);
    // "HHH" (66 units) still fits before the first stop, "HHHI" (76) doesn't.
    assert!((x("HHH\tH") - (79.0 / 21.0 + h_stem)).abs() < EPS);
    assert!((x("HHHI\tH") - (163.0 / 21.0 + h_stem)).abs() < EPS);
}

#[test]
fn italic_shears_by_one_eighth() {
    let mut s = h("I", HJustify::Left, VJustify::Bottom);
    s.italic = true;
    let stem = &to_strokes(&s).strokes[0];
    let dx = stem[1][0] - stem[0][0];
    let dy = stem[0][1] - stem[1][1];
    assert!((dx / dy - 0.125).abs() < 1e-9);
}

#[test]
fn pen_width_defaults_and_clamp() {
    let mut s = h("H", HJustify::Left, VJustify::Bottom);
    s.size = [2.0, 1.0];
    s.thickness = 0.0;
    assert!((pen_width(&s) - 0.25).abs() < 1e-12); // width / 8
    s.bold = true;
    assert!((pen_width(&s) - 0.25).abs() < 1e-12); // width / 5, clamped
    s.size = [1.0, 2.0];
    assert!((pen_width(&s) - 0.2).abs() < 1e-12);
    s.bold = false;
    s.thickness = 0.4;
    assert!((pen_width(&s) - 0.25).abs() < 1e-12); // min(w, h) / 4
    assert_eq!(to_strokes(&s).width, 0.25);
}

#[test]
fn keep_upright_keeps_justification() {
    let mut s = h("Hg", HJustify::Left, VJustify::Bottom);
    let upright = ext(&s);
    s.keep_upright = true;
    s.angle_deg = 180.0;
    close(ext(&s), upright);
    s.angle_deg = 270.0;
    let mut v = s.clone();
    v.angle_deg = 90.0;
    v.keep_upright = false;
    close(ext(&s), ext(&v));
    // 90 is readable and stays.
    s.angle_deg = 90.0;
    close(ext(&s), ext(&v));
    // Without keep_upright, 180 really is upside down.
    s.keep_upright = false;
    s.angle_deg = 180.0;
    close(
        ext(&s),
        [-upright[2], -upright[3], -upright[0], -upright[1]],
    );
}

#[test]
fn y_up_frame_negates_y() {
    let s = h("Hg", HJustify::Left, VJustify::Bottom);
    let mut u = s.clone();
    u.y_down = false;
    u.pos = [s.pos[0], -s.pos[1]];
    let (a, b) = (to_strokes(&s), to_strokes(&u));
    for (p, q) in a.strokes.iter().flatten().zip(b.strokes.iter().flatten()) {
        assert!((p[0] - q[0]).abs() < 1e-12 && (p[1] + q[1]).abs() < 1e-12);
    }
    assert!((a.bbox.min[1] + b.bbox.max[1]).abs() < 1e-12);
}

#[test]
fn bbox_includes_half_the_pen() {
    let s = h("H", HJustify::Left, VJustify::Bottom);
    let r = to_strokes(&s);
    let e = ext(&s);
    assert!((r.bbox.min[0] - (s.pos[0] + e[0] - 0.05)).abs() < 1e-9);
    assert!((r.bbox.max[1] - (s.pos[1] + e[3] + 0.05)).abs() < 1e-9);
}

#[test]
fn empty_and_blank_text_have_no_ink() {
    for t in ["", " ", "  \t ", "\n"] {
        let r = to_strokes(&h(t, HJustify::Center, VJustify::Center));
        assert!(r.strokes.is_empty(), "{t:?}");
        assert!(r.bbox.is_empty());
    }
}

#[test]
fn unknown_characters_draw_the_placeholder_box() {
    let a = to_strokes(&h("\u{1F600}", HJustify::Left, VJustify::Bottom));
    let b = to_strokes(&h("\u{2BFF}", HJustify::Left, VJustify::Bottom));
    assert_eq!(a.strokes, b.strokes);
    assert_eq!(a.strokes.len(), 1);
    assert_eq!(a.strokes[0].len(), 5); // closed box
}

#[test]
fn into_prim_is_strokes() {
    let r = to_strokes(&h("R1", HJustify::Center, VJustify::Center));
    let (strokes, width) = (r.strokes.clone(), r.width);
    match r.into_prim() {
        Prim::Strokes {
            strokes: s,
            width: w,
        } => {
            assert_eq!(s, strokes);
            assert_eq!(w, width);
        }
        other => panic!("{other:?}"),
    }
}
