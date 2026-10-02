//! Compares `to_strokes` with text plotted by KiCad itself.
//!
//! The fixtures under `tests/fixtures/` were produced by `kicad-cli` 10.0.6
//! from boards/schematics generated out of [`pcb_cases`] / [`sch_cases`]:
//!
//! ```text
//! cargo test -p kicad-strokes --test kicad_compare -- --ignored regenerate
//! ```
//!
//! (needs `kicad-cli` on PATH). The normal tests only read the saved SVGs.
//!
//! Measured accuracy: every stroke point is within 0.0002 mm of KiCad's
//! (the SVG has 4 decimals), for all justifications, 0/90/180/270/30 degrees,
//! mirroring, multi-line, markup, tabs, italic, bold and non-square sizes.
//! The asserted tolerance is [`TOL`].

use kicad_strokes::{to_strokes, HJustify, TextSpec, VJustify, SCH_DEFAULT_PEN, SCH_TEXT_OFFSET};
use std::fmt::Write as _;
use std::path::PathBuf;

/// Largest allowed distance (mm) from any of our stroke points to KiCad's
/// strokes and back, and the largest allowed bbox edge difference.
const TOL: f64 = 0.001;

struct Case {
    spec: TextSpec,
    /// PCB layer, or schematic item kind.
    layer: &'static str,
}

fn spec(text: &str) -> TextSpec {
    TextSpec {
        text: text.into(),
        size: [1.0, 1.0],
        thickness: 0.15,
        ..TextSpec::default()
    }
}

fn grid(cases: Vec<(TextSpec, &'static str)>) -> Vec<Case> {
    cases
        .into_iter()
        .enumerate()
        .map(|(i, (mut s, layer))| {
            s.pos = [20.0 + (i % 8) as f64 * 25.0, 20.0 + (i / 8) as f64 * 15.0];
            Case { spec: s, layer }
        })
        .collect()
}

fn pcb_cases() -> Vec<Case> {
    use HJustify::{Left, Right};
    use VJustify::{Bottom, Top};
    let mut v = Vec::new();
    for h in [Left, HJustify::Center, Right] {
        for vj in [Top, VJustify::Center, Bottom] {
            let mut s = spec("Hg");
            s.justify_h = h;
            s.justify_v = vj;
            v.push((s, "F.SilkS"));
        }
    }
    for a in [0.0, 90.0, 180.0, 270.0, 30.0] {
        for mirror in [false, true] {
            let mut s = spec("Hg1");
            s.angle_deg = a;
            s.justify_h = Left;
            s.justify_v = Bottom;
            s.mirror = mirror;
            v.push((s, if mirror { "B.SilkS" } else { "F.SilkS" }));
        }
    }
    for (h, vj) in [
        (Left, Top),
        (HJustify::Center, VJustify::Center),
        (Right, Bottom),
    ] {
        let mut s = spec("AB\nCDE");
        s.justify_h = h;
        s.justify_v = vj;
        v.push((s, "F.SilkS"));
    }
    for t in [
        "~{RST}",
        "A~{B}C",
        "V_{CC}",
        "x^{2}",
        "A\tB",
        "ABCD\tE",
        "x^{y_{z}}",
        "_{A",
    ] {
        let mut s = spec(t);
        s.justify_h = Left;
        s.justify_v = Bottom;
        v.push((s, "F.SilkS"));
    }
    let mut s = spec("Hl");
    s.italic = true;
    v.push((s, "F.SilkS"));
    let mut s = spec("Hl");
    s.bold = true;
    s.thickness = 0.0;
    v.push((s, "F.SilkS"));
    let mut s = spec("H_{2}");
    s.size = [2.0, 1.0];
    s.italic = true;
    v.push((s, "F.SilkS"));
    let mut s = spec("H");
    s.size = [1.0, 2.0];
    s.thickness = 0.4; // clamped to 0.25
    v.push((s, "F.SilkS"));
    let mut s = spec("Hg");
    s.size = [2.0, 2.0];
    s.thickness = 0.3;
    s.angle_deg = 45.0;
    s.justify_h = Right;
    s.justify_v = Top;
    v.push((s, "F.SilkS"));
    grid(v)
}

fn sch_cases() -> Vec<Case> {
    use HJustify::{Left, Right};
    use VJustify::{Bottom, Top};
    let mut v = Vec::new();
    for a in [0.0, 90.0, 180.0, 270.0] {
        for (h, vj) in [(Left, Bottom), (Right, Top)] {
            let mut s = spec("Hg");
            s.size = [1.27, 1.27];
            s.thickness = 0.0;
            s.angle_deg = a;
            s.justify_h = h;
            s.justify_v = vj;
            v.push((s, "text"));
        }
    }
    grid(v)
}

/// The spec KiCad effectively draws for a schematic `(text ...)` item.
fn sch_effective(s: &TextSpec) -> TextSpec {
    let mut a = s.clone();
    if a.thickness <= 0.0 {
        a.thickness = SCH_DEFAULT_PEN;
    }
    a.keep_upright = true;
    a.pos[1] -= SCH_TEXT_OFFSET;
    a
}

fn justify(s: &TextSpec) -> String {
    let mut j = Vec::new();
    match s.justify_h {
        HJustify::Left => j.push("left"),
        HJustify::Right => j.push("right"),
        HJustify::Center => {}
    }
    match s.justify_v {
        VJustify::Top => j.push("top"),
        VJustify::Bottom => j.push("bottom"),
        VJustify::Center => {}
    }
    if s.mirror {
        j.push("mirror");
    }
    if j.is_empty() {
        String::new()
    } else {
        format!("(justify {})", j.join(" "))
    }
}

fn quote(t: &str) -> String {
    t.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

fn pcb_file(cases: &[Case]) -> String {
    let mut o = String::from(
        "(kicad_pcb (version 20260206) (generator \"pcbnew\") (generator_version \"10.0\")\n\
         (general (thickness 1.6)) (paper \"A4\")\n\
         (layers (0 \"F.Cu\" signal) (2 \"B.Cu\" signal) (5 \"F.SilkS\" user \"F.Silkscreen\") \
         (7 \"B.SilkS\" user \"B.Silkscreen\") (25 \"Edge.Cuts\" user))\n\
         (setup (pad_to_mask_clearance 0)) (net 0 \"\")\n",
    );
    for c in cases {
        let s = &c.spec;
        let mut font = format!(
            "(size {} {}) (thickness {})",
            s.size[1], s.size[0], s.thickness
        );
        if s.bold {
            font.push_str(" (bold yes)");
        }
        if s.italic {
            font.push_str(" (italic yes)");
        }
        let _ = writeln!(
            o,
            "(gr_text \"{}\" (at {} {} {}) (layer \"{}\") (effects (font {}) {}))",
            quote(&s.text),
            s.pos[0],
            s.pos[1],
            s.angle_deg,
            c.layer,
            font,
            justify(s)
        );
    }
    o.push_str(")\n");
    o
}

fn sch_file(cases: &[Case]) -> String {
    let mut o = String::from(
        "(kicad_sch (version 20230121) (generator \"kicad-strokes\") \
         (uuid \"504fb266-d91b-41f2-bf05-15a43de856e1\") (paper \"A4\") (lib_symbols)\n",
    );
    for (i, c) in cases.iter().enumerate() {
        let s = &c.spec;
        let mut font = format!("(size {} {})", s.size[1], s.size[0]);
        if s.thickness > 0.0 {
            let _ = write!(font, " (thickness {})", s.thickness);
        }
        let _ = writeln!(
            o,
            "({} \"{}\" (at {} {} {}) (effects (font {}) {}) (uuid \"00000000-0000-0000-0000-{:012}\"))",
            c.layer,
            quote(&s.text),
            s.pos[0],
            s.pos[1],
            s.angle_deg,
            font,
            justify(s),
            i + 1
        );
    }
    o.push_str("(sheet_instances (path \"/\" (page \"1\"))))\n");
    o
}

type Poly = Vec<[f64; 2]>;

/// Extracts KiCad's `<g class="stroked-text">` groups: polylines plus the
/// enclosing stroke width.
fn parse_svg(svg: &str) -> Vec<(Vec<Poly>, f64)> {
    let mut out = Vec::new();
    let mut width = 0.0;
    let mut rest = svg;
    while let Some(g) = rest.find("class=\"stroked-text\"") {
        if let Some(w) = rest[..g].rfind("stroke-width:") {
            let t = &rest[w + "stroke-width:".len()..];
            let end = t.find([';', '"']).unwrap();
            width = t[..end].trim().parse().unwrap();
        }
        let body = &rest[g..];
        let end = body.find("</g>").unwrap();
        let mut polys = Vec::new();
        let mut p = &body[..end];
        while let Some(i) = p.find("d=\"") {
            let q = &p[i + 3..];
            let j = q.find('"').unwrap();
            let toks: Vec<&str> = q[..j].split_whitespace().collect();
            let mut poly = Vec::new();
            for pair in toks.chunks(2) {
                if pair[0].starts_with('M') && !poly.is_empty() {
                    polys.push(std::mem::take(&mut poly));
                }
                let x: f64 = pair[0].trim_start_matches(['M', 'L']).parse().unwrap();
                let y: f64 = pair[1].parse().unwrap();
                poly.push([x, y]);
            }
            if !poly.is_empty() {
                polys.push(poly);
            }
            p = &q[j..];
        }
        out.push((polys, width));
        rest = &body[end..];
    }
    out
}

fn bbox(polys: &[Poly]) -> [f64; 4] {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in polys.iter().flatten() {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

/// Symmetric Hausdorff distance between two point sets.
fn hausdorff(a: &[[f64; 2]], b: &[[f64; 2]]) -> f64 {
    let one = |a: &[[f64; 2]], b: &[[f64; 2]]| {
        a.iter()
            .map(|p| {
                b.iter()
                    .map(|q| (p[0] - q[0]).hypot(p[1] - q[1]))
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max)
    };
    one(a, b).max(one(b, a))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Assigns each KiCad group (one per text line) to the nearest case anchor.
fn compare(cases: &[Case], svg: &str, effective: impl Fn(&TextSpec) -> TextSpec) -> (f64, f64) {
    let mut kicad: Vec<(Vec<Poly>, f64)> = vec![(Vec::new(), 0.0); cases.len()];
    for (polys, w) in parse_svg(svg) {
        let b = bbox(&polys);
        let c = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
        let near = (0..cases.len())
            .min_by(|&i, &j| {
                let d = |k: usize| {
                    let p = cases[k].spec.pos;
                    (p[0] - c[0]).hypot(p[1] - c[1])
                };
                d(i).total_cmp(&d(j))
            })
            .unwrap();
        kicad[near].0.extend(polys);
        kicad[near].1 = w;
    }
    let (mut worst_pt, mut worst_box) = (0.0f64, 0.0f64);
    for (case, (kp, kw)) in cases.iter().zip(&kicad) {
        let ours = to_strokes(&effective(&case.spec));
        assert!(
            !kp.is_empty(),
            "KiCad drew nothing for {:?}",
            case.spec.text
        );
        assert!(
            (ours.width - kw).abs() < 1e-4,
            "{:?}: pen {} vs KiCad {kw}",
            case.spec.text,
            ours.width
        );
        let kpts: Vec<[f64; 2]> = kp.iter().flatten().copied().collect();
        let opts: Vec<[f64; 2]> = ours.strokes.iter().flatten().copied().collect();
        let d = hausdorff(&kpts, &opts);
        let (kb, ob) = (bbox(kp), bbox(&ours.strokes));
        let db = (0..4).map(|i| (kb[i] - ob[i]).abs()).fold(0.0, f64::max);
        assert!(
            d <= TOL && db <= TOL,
            "{:?} at {} deg: point error {d:.5} mm, bbox error {db:.5} mm",
            case.spec.text,
            case.spec.angle_deg
        );
        worst_pt = worst_pt.max(d);
        worst_box = worst_box.max(db);
    }
    (worst_pt, worst_box)
}

#[test]
fn matches_kicad_pcb_plot() {
    let svg = std::fs::read_to_string(fixture("pcb_text.svg")).unwrap();
    let (pt, bx) = compare(&pcb_cases(), &svg, Clone::clone);
    eprintln!("pcb: worst point error {pt:.5} mm, worst bbox error {bx:.5} mm");
}

#[test]
fn matches_kicad_schematic_plot() {
    let svg = std::fs::read_to_string(fixture("sch_text.svg")).unwrap();
    let (pt, bx) = compare(&sch_cases(), &svg, sch_effective);
    eprintln!("sch: worst point error {pt:.5} mm, worst bbox error {bx:.5} mm");
}

#[test]
fn fixture_sources_are_current() {
    // The saved board/schematic must be what the case lists generate, so the
    // SVGs stay reproducible.
    let pcb = std::fs::read_to_string(fixture("pcb_text.kicad_pcb")).unwrap();
    assert_eq!(pcb, pcb_file(&pcb_cases()));
    let sch = std::fs::read_to_string(fixture("sch_text.kicad_sch")).unwrap();
    assert_eq!(sch, sch_file(&sch_cases()));
}

#[test]
#[ignore = "needs kicad-cli; rewrites tests/fixtures"]
fn regenerate() {
    let run = |args: &[&str]| {
        let out = std::process::Command::new("kicad-cli")
            .args(args)
            .output()
            .expect("kicad-cli");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let pcb = fixture("pcb_text.kicad_pcb");
    std::fs::write(&pcb, pcb_file(&pcb_cases())).unwrap();
    let svg = fixture("pcb_text.svg");
    run(&[
        "pcb",
        "export",
        "svg",
        "-l",
        "F.SilkS,B.SilkS",
        "--exclude-drawing-sheet",
        "--mode-single",
        "-o",
        svg.to_str().unwrap(),
        pcb.to_str().unwrap(),
    ]);
    let sch = fixture("sch_text.kicad_sch");
    std::fs::write(&sch, sch_file(&sch_cases())).unwrap();
    let dir = format!("{}/", fixture("").display());
    run(&[
        "sch",
        "export",
        "svg",
        "--exclude-drawing-sheet",
        "-o",
        &dir,
        sch.to_str().unwrap(),
    ]);
    // kicad-cli leaves a project-local settings file next to the board.
    let _ = std::fs::remove_file(fixture("pcb_text.kicad_prl"));
}
