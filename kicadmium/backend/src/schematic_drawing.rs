//! Sheet furniture and free graphics of a schematic page.
//!
//! [`frame`] draws KiCad's default drawing sheet natively: the double border
//! with its lettered/numbered reference grid and the title block, filled from
//! the page's `(title_block ...)`. Custom `.kicad_wks` sheets are not read.
//!
//! [`annotations`] converts the page's top-level graphics (`text`,
//! `text_box`, `polyline`, `rectangle`, `circle`, `arc`, `bezier`, `bus`,
//! `bus_entry`) into scene primitives. Images are not drawn.

use crate::symbol_graphics::{body_prims, Fill};
use kct::schema::library::{
    SymbolArc, SymbolCircle, SymbolGraphic, SymbolPolyline, SymbolRectangle,
};
use kct::SExp;
use kicad_strokes::{HJustify, TextSpec, VJustify};
use vector_view::Prim;

/// Page border margin on every side (KiCad default drawing sheet).
const MARGIN: f64 = 10.0;
/// Width of the reference-grid band between the outer and inner border.
const BAND: f64 = 2.0;
/// Reference-grid cell size.
const CELL: f64 = 50.0;
/// Drawing-sheet line width.
pub(crate) const FRAME_LINE: f64 = 0.15;
/// KiCad's default schematic graphic line width (6 mil).
const GRAPHIC_LINE: f64 = 0.1524;

/// Paper name and size in mm (landscape unless `portrait`), from `(paper ...)`.
pub(crate) fn paper(root: &SExp) -> (String, f64, f64) {
    let Some(p) = root.get("paper") else {
        return ("A4".into(), 297.0, 210.0);
    };
    let name = p.string_at(0).unwrap_or("A4").to_owned();
    let portrait = p
        .children
        .iter()
        .any(|c| c.value.as_ref().and_then(|v| v.as_str()) == Some("portrait"));
    let (w, h) = match name.as_str() {
        "User" => (
            p.float_at(1).unwrap_or(297.0),
            p.float_at(2).unwrap_or(210.0),
        ),
        "A5" => (210.0, 148.0),
        "A3" => (420.0, 297.0),
        "A2" => (594.0, 420.0),
        "A1" => (841.0, 594.0),
        "A0" => (1189.0, 841.0),
        "A" | "USLetter" => (279.4, 215.9),
        "B" | "USLedger" => (431.8, 279.4),
        "C" => (558.8, 431.8),
        "D" => (863.6, 558.8),
        "E" => (1117.6, 863.6),
        "USLegal" => (355.6, 215.9),
        "GERBER" => (812.8, 812.8),
        _ => (297.0, 210.0),
    };
    if portrait && name != "User" {
        (name, h, w)
    } else {
        (name, w, h)
    }
}

/// Values substituted into the default title block.
#[derive(Debug, Clone, Default)]
pub(crate) struct TitleInfo {
    pub title: String,
    pub date: String,
    pub rev: String,
    pub company: String,
    pub comments: [String; 4],
    pub paper: String,
    pub file: String,
    pub sheet_path: String,
    pub sheet: usize,
    pub sheets: usize,
}

/// Drawing-sheet geometry: line paths and text runs, in page mm (y down).
#[derive(Debug, Default)]
pub(crate) struct Furniture {
    pub lines: Vec<Vec<[f64; 2]>>,
    pub texts: Vec<TextSpec>,
}

fn text(s: String, pos: [f64; 2], size: f64, h: HJustify, bold: bool, italic: bool) -> TextSpec {
    TextSpec {
        text: s,
        pos,
        size: [size, size],
        thickness: if bold { 0.3 } else { FRAME_LINE },
        justify_h: h,
        justify_v: VJustify::Center,
        bold,
        italic,
        ..TextSpec::default()
    }
}

fn rect(a: [f64; 2], b: [f64; 2]) -> Vec<[f64; 2]> {
    vec![a, [b[0], a[1]], b, [a[0], b[1]], a]
}

/// KiCad's default drawing sheet for a `w` x `h` mm page.
pub(crate) fn frame(w: f64, h: f64, info: &TitleInfo) -> Furniture {
    let mut f = Furniture::default();
    let (l, t, r, b) = (MARGIN, MARGIN, w - MARGIN, h - MARGIN);
    if r - l < 4.0 * BAND || b - t < 4.0 * BAND {
        return f;
    }
    f.lines.push(rect([l, t], [r, b]));
    f.lines
        .push(rect([l + BAND, t + BAND], [r - BAND, b - BAND]));
    // Reference grid: numbered columns along top/bottom, lettered rows along
    // the sides, a tick every CELL mm and the label centred in each cell.
    let mut k = 0;
    while l + CELL * k as f64 + CELL / 2.0 < r {
        let x0 = l + CELL * k as f64;
        let label = (k + 1).to_string();
        let mid = (x0 + CELL / 2.0).min((x0 + r) / 2.0);
        for (y, edge) in [(t + BAND / 2.0, t), (b - BAND / 2.0, b)] {
            f.texts.push(text(
                label.clone(),
                [mid, y],
                1.3,
                HJustify::Center,
                false,
                false,
            ));
            let x = x0 + CELL;
            if x < r {
                let inner = if edge == t { t + BAND } else { b - BAND };
                f.lines.push(vec![[x, edge], [x, inner]]);
            }
        }
        k += 1;
    }
    let mut k = 0;
    while t + CELL * k as f64 + CELL / 2.0 < b {
        let y0 = t + CELL * k as f64;
        let label = char::from_u32('A' as u32 + k as u32 % 26)
            .unwrap_or('?')
            .to_string();
        let mid = (y0 + CELL / 2.0).min((y0 + b) / 2.0);
        for (x, edge) in [(l + BAND / 2.0, l), (r - BAND / 2.0, r)] {
            f.texts.push(text(
                label.clone(),
                [x, mid],
                1.3,
                HJustify::Center,
                false,
                false,
            ));
            let y = y0 + CELL;
            if y < b {
                let inner = if edge == l { l + BAND } else { r - BAND };
                f.lines.push(vec![[edge, y], [inner, y]]);
            }
        }
        k += 1;
    }
    // Title block, laid out from the bottom-right border corner (x left, y up).
    let at = |x: f64, y: f64| [r - x, b - y];
    if r - l > 112.0 && b - t > 36.0 {
        f.lines.push(rect(at(110.0, 34.0), at(2.0, 2.0)));
        for y in [5.5, 8.5, 12.5, 18.5] {
            f.lines.push(vec![at(110.0, y), at(2.0, y)]);
        }
        f.lines.push(vec![at(90.0, 8.5), at(90.0, 5.5)]);
        f.lines.push(vec![at(26.0, 8.5), at(26.0, 2.0)]);
        let left = HJustify::Left;
        let mut put = |s: String, x: f64, y: f64, size: f64, bold: bool, italic: bool| {
            f.texts.push(text(s, at(x, y), size, left, bold, italic))
        };
        put(format!("Date: {}", info.date), 87.0, 6.9, 1.0, false, false);
        put("kicadmium".into(), 109.0, 4.1, 1.5, false, false);
        put(format!("Rev: {}", info.rev), 24.0, 6.9, 1.5, true, false);
        put(
            format!("Size: {}", info.paper),
            109.0,
            6.9,
            1.5,
            false,
            false,
        );
        put(
            format!("Id: {}/{}", info.sheet, info.sheets),
            24.0,
            4.1,
            1.5,
            false,
            false,
        );
        put(
            format!("Title: {}", info.title),
            109.0,
            10.7,
            2.0,
            true,
            true,
        );
        put(
            format!("File: {}", info.file),
            109.0,
            14.3,
            1.5,
            false,
            false,
        );
        put(
            format!("Sheet: {}", info.sheet_path),
            109.0,
            17.0,
            1.5,
            false,
            false,
        );
        put(info.company.clone(), 109.0, 20.0, 1.5, true, false);
        for (i, c) in info.comments.iter().enumerate() {
            put(c.clone(), 109.0, 23.0 + 3.0 * i as f64, 1.5, false, false);
        }
    }
    f.texts.retain(|s| !s.text.trim().is_empty());
    f
}

/// One free graphic of a page.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Annotation {
    /// A drawn shape: outline (possibly dashed) plus an optional fill.
    Shape {
        uuid: String,
        outline: Prim,
        fill: Option<(Fill, Prim)>,
    },
    /// A text note (also the text of a text box).
    Text {
        uuid: String,
        spec: TextSpec,
    },
    Bus {
        uuid: String,
        prim: Prim,
    },
    BusEntry {
        uuid: String,
        prim: Prim,
    },
}

fn uuid(node: &SExp) -> String {
    node.get("uuid")
        .and_then(|u| u.string_at(0))
        .unwrap_or_default()
        .to_owned()
}

fn xy(node: Option<&SExp>) -> Option<[f64; 2]> {
    let n = node?;
    Some([n.float_at(0)?, n.float_at(1)?])
}

fn has_atom(node: &SExp, atom: &str) -> bool {
    node.children
        .iter()
        .any(|c| c.value.as_ref().and_then(|v| v.as_str()) == Some(atom))
}

/// `bold`/`italic`/`hide` appear as bare atoms (KiCad 6–7) or as
/// `(bold yes)` lists (KiCad 8+).
fn flag(node: &SExp, name: &str) -> bool {
    has_atom(node, name)
        || node
            .get(name)
            .is_some_and(|c| c.string_at(0).is_none_or(|v| v == "yes"))
}

/// Stroke width and dash style of a graphic.
fn stroke(node: &SExp) -> (f64, String) {
    let s = node.get("stroke");
    let w = s
        .and_then(|s| s.get("width"))
        .and_then(|w| w.float_at(0))
        .filter(|w| *w > 0.0)
        .unwrap_or(GRAPHIC_LINE);
    let kind = s
        .and_then(|s| s.get("type"))
        .and_then(|t| t.string_at(0))
        .unwrap_or("default")
        .to_owned();
    (w, kind)
}

/// Text placement and font of a `text`/`text_box` node; `None` if hidden.
pub(crate) fn text_spec(node: &SExp, text: &str) -> Option<TextSpec> {
    let at = node.get("at");
    let pos = xy(at)?;
    let angle = at.and_then(|a| a.float_at(2)).unwrap_or(0.0);
    let effects = node.get("effects");
    if effects.is_some_and(|e| flag(e, "hide")) || flag(node, "hide") {
        return None;
    }
    let font = effects.and_then(|e| e.get("font"));
    let size = font
        .and_then(|f| f.get("size"))
        .and_then(|s| Some([s.float_at(1)?, s.float_at(0)?]))
        .unwrap_or([1.27, 1.27]);
    let bold = font.is_some_and(|f| flag(f, "bold"));
    let italic = font.is_some_and(|f| flag(f, "italic"));
    let thickness = font
        .and_then(|f| f.get("thickness"))
        .and_then(|t| t.float_at(0))
        .filter(|t| *t > 0.0)
        .unwrap_or(if bold {
            size[0].min(size[1]) / 5.0
        } else {
            kicad_strokes::SCH_DEFAULT_PEN
        });
    let justify = effects.and_then(|e| e.get("justify"));
    let j = |a: &str| justify.is_some_and(|n| has_atom(n, a));
    Some(TextSpec {
        text: text.to_owned(),
        pos,
        size,
        thickness,
        angle_deg: angle,
        justify_h: if j("left") {
            HJustify::Left
        } else if j("right") {
            HJustify::Right
        } else {
            HJustify::Center
        },
        justify_v: if j("top") {
            VJustify::Top
        } else if j("bottom") {
            VJustify::Bottom
        } else {
            VJustify::Center
        },
        mirror: j("mirror"),
        bold,
        italic,
        keep_upright: true,
        ..TextSpec::default()
    })
}

/// Split a path into dash runs for KiCad `dash`/`dot`/`dash_dot`/
/// `dash_dot_dot` strokes; `None` for solid strokes.
pub(crate) fn dashes(path: &[[f64; 2]], width: f64, kind: &str) -> Option<Vec<Vec<[f64; 2]>>> {
    let (dash, dot, gap) = (6.0 * width, 0.2 * width, 3.0 * width);
    let pattern: Vec<f64> = match kind {
        "dash" => vec![dash, gap],
        "dot" => vec![dot, gap],
        "dash_dot" => vec![dash, gap, dot, gap],
        "dash_dot_dot" => vec![dash, gap, dot, gap, dot, gap],
        _ => return None,
    };
    let mut runs = vec![];
    let mut current: Vec<[f64; 2]> = vec![];
    let (mut index, mut left) = (0, pattern[0]);
    for w in path.windows(2) {
        let (a, z) = (w[0], w[1]);
        let length = ((z[0] - a[0]).powi(2) + (z[1] - a[1]).powi(2)).sqrt();
        let mut done = 0.0;
        while done < length - 1e-9 {
            let step = left.min(length - done);
            let p = |s: f64| {
                [
                    a[0] + (z[0] - a[0]) * s / length,
                    a[1] + (z[1] - a[1]) * s / length,
                ]
            };
            if index % 2 == 0 {
                if current.is_empty() {
                    current.push(p(done));
                }
                current.push(p(done + step));
            }
            done += step;
            left -= step;
            if left <= 1e-9 {
                if index % 2 == 0 && current.len() > 1 {
                    runs.push(std::mem::take(&mut current));
                }
                current.clear();
                index = (index + 1) % pattern.len();
                left = pattern[index];
            }
        }
    }
    if current.len() > 1 {
        runs.push(current);
    }
    Some(runs)
}

/// Points along a shape outline, for dashing.
fn outline_path(p: &Prim) -> Option<Vec<[f64; 2]>> {
    match p {
        Prim::Polyline { points, .. } => Some(points.clone()),
        Prim::Polygon { outer, .. } => {
            let mut v = outer.clone();
            v.push(*outer.first()?);
            Some(v)
        }
        Prim::Circle { center, radius, .. } => Some(
            (0..=72)
                .map(|i| {
                    let t = i as f64 / 72.0 * std::f64::consts::TAU;
                    [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
                })
                .collect(),
        ),
        Prim::Arc {
            center,
            radius,
            start,
            end,
            ..
        } => {
            let sweep = (end - start).rem_euclid(std::f64::consts::TAU);
            Some(
                (0..=48)
                    .map(|i| {
                        let t = start + sweep * i as f64 / 48.0;
                        [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

/// A shape node through the symbol-graphic converter (schematic page
/// coordinates are used as-is), with KiCad dash styles applied.
fn shape(node: &SExp, graphic: SymbolGraphic) -> Option<Annotation> {
    let (w, kind) = stroke(node);
    let mut prims = body_prims(&graphic, |(x, y)| [x, y], |_| w)?;
    if let Some(runs) = outline_path(&prims.outline).and_then(|p| dashes(&p, w, &kind)) {
        prims.outline = Prim::Strokes {
            strokes: runs,
            width: w,
        };
    }
    Some(Annotation::Shape {
        uuid: uuid(node),
        outline: prims.outline,
        fill: prims.fill,
    })
}

fn bezier(node: &SExp) -> Option<Annotation> {
    let pts: Vec<[f64; 2]> = node
        .get("pts")?
        .children_named("xy")
        .filter_map(|p| xy(Some(p)))
        .collect();
    if pts.len() != 4 {
        return None;
    }
    let (w, kind) = stroke(node);
    let curve: Vec<[f64; 2]> = (0..=32)
        .map(|i| {
            let t = i as f64 / 32.0;
            let u = 1.0 - t;
            let c = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            [
                (0..4).map(|k| c[k] * pts[k][0]).sum(),
                (0..4).map(|k| c[k] * pts[k][1]).sum(),
            ]
        })
        .collect();
    let outline = match dashes(&curve, w, &kind) {
        Some(runs) => Prim::Strokes {
            strokes: runs,
            width: w,
        },
        None => Prim::Polyline {
            points: curve,
            width: w,
        },
    };
    Some(Annotation::Shape {
        uuid: uuid(node),
        outline,
        fill: None,
    })
}

/// A KiCad 7+ text box: border (unless stroke type `none`/negative width)
/// and its text, anchored inside the margins by its justification.
fn text_box(node: &SExp, out: &mut Vec<Annotation>) {
    let content = node.string_at(0).unwrap_or_default();
    let (a, size) = match (xy(node.get("at")), xy(node.get("size"))) {
        (Some(a), Some(s)) => (a, s),
        _ => match (xy(node.get("start")), xy(node.get("end"))) {
            (Some(a), Some(z)) => (a, [z[0] - a[0], z[1] - a[1]]),
            _ => return,
        },
    };
    let z = [a[0] + size[0], a[1] + size[1]];
    let negative = node
        .get("stroke")
        .and_then(|s| s.get("width"))
        .and_then(|w| w.float_at(0))
        .is_some_and(|w| w < 0.0);
    let (w, kind) = stroke(node);
    if !negative && kind != "none" {
        let outer = vec![a, [z[0], a[1]], z, [a[0], z[1]]];
        let path = rect(a, z);
        let outline = match dashes(&path, w, &kind) {
            Some(runs) => Prim::Strokes {
                strokes: runs,
                width: w,
            },
            None => Prim::Polygon {
                outer: outer.clone(),
                holes: vec![],
                fill: false,
                stroke: w,
            },
        };
        let fill = node
            .get("fill")
            .and_then(|f| f.get("type"))
            .and_then(|t| t.string_at(0))
            .map(Fill::parse)
            .filter(|f| *f != Fill::None)
            .map(|f| {
                (
                    f,
                    Prim::Polygon {
                        outer,
                        holes: vec![],
                        fill: true,
                        stroke: 0.0,
                    },
                )
            });
        out.push(Annotation::Shape {
            uuid: uuid(node),
            outline,
            fill,
        });
    }
    let Some(mut spec) = text_spec(
        &SExp::list(
            "text_box",
            [
                SExp::list("at", [SExp::atom(a[0]), SExp::atom(a[1])]),
                node.get("effects").cloned().unwrap_or_default(),
            ],
        ),
        content,
    ) else {
        return;
    };
    let margins: Vec<f64> = node
        .get("margins")
        .map(|m| (0..4).filter_map(|i| m.float_at(i)).collect())
        .filter(|m: &Vec<f64>| m.len() == 4)
        .unwrap_or_else(|| vec![spec.size[1] / 2.0; 4]);
    // Text boxes default to top-left justification.
    let justify = node.get("effects").and_then(|e| e.get("justify"));
    if justify.is_none() {
        spec.justify_h = HJustify::Left;
        spec.justify_v = VJustify::Top;
    }
    spec.pos = [
        match spec.justify_h {
            HJustify::Left => a[0] + margins[0],
            HJustify::Right => z[0] - margins[2],
            HJustify::Center => (a[0] + z[0]) / 2.0,
        },
        match spec.justify_v {
            VJustify::Top => a[1] + margins[1],
            VJustify::Bottom => z[1] - margins[3],
            VJustify::Center => (a[1] + z[1]) / 2.0,
        },
    ];
    out.push(Annotation::Text {
        uuid: uuid(node),
        spec,
    });
}

/// All free graphics directly on a page (not inside symbols or sheets).
pub(crate) fn annotations(root: &SExp) -> Vec<Annotation> {
    let mut out = vec![];
    for node in &root.children {
        match node.tag() {
            Some("text") => {
                let content = node.string_at(0).unwrap_or_default();
                if let Some(spec) = text_spec(node, content) {
                    out.push(Annotation::Text {
                        uuid: uuid(node),
                        spec,
                    });
                }
            }
            Some("text_box") => text_box(node, &mut out),
            Some("polyline") => out.extend(shape(
                node,
                SymbolGraphic::Polyline(SymbolPolyline::from_sexp(node)),
            )),
            Some("rectangle") => out.extend(shape(
                node,
                SymbolGraphic::Rectangle(SymbolRectangle::from_sexp(node)),
            )),
            Some("circle") => out.extend(shape(
                node,
                SymbolGraphic::Circle(SymbolCircle::from_sexp(node)),
            )),
            Some("arc") => out.extend(shape(node, SymbolGraphic::Arc(SymbolArc::from_sexp(node)))),
            Some("bezier") => out.extend(bezier(node)),
            Some("bus") => {
                let points: Vec<[f64; 2]> = node
                    .get("pts")
                    .map(|p| p.children_named("xy").filter_map(|q| xy(Some(q))).collect())
                    .unwrap_or_default();
                if points.len() >= 2 {
                    let w = node
                        .get("stroke")
                        .and_then(|s| s.get("width"))
                        .and_then(|w| w.float_at(0))
                        .filter(|w| *w > 0.0)
                        .unwrap_or(0.3048);
                    out.push(Annotation::Bus {
                        uuid: uuid(node),
                        prim: Prim::Polyline { points, width: w },
                    });
                }
            }
            Some("bus_entry") => {
                if let (Some(a), Some(s)) = (xy(node.get("at")), xy(node.get("size"))) {
                    out.push(Annotation::BusEntry {
                        uuid: uuid(node),
                        prim: Prim::Polyline {
                            points: vec![a, [a[0] + s[0], a[1] + s[1]]],
                            width: stroke(node).0.max(0.1524),
                        },
                    });
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> SExp {
        kct::parse(s).unwrap()
    }

    #[test]
    fn paper_sizes() {
        assert_eq!(paper(&parse("(kicad_sch (paper \"A4\"))")).1, 297.0);
        assert_eq!(
            paper(&parse("(kicad_sch (paper \"A3\" portrait))")).1,
            297.0
        );
        let user = paper(&parse("(kicad_sch (paper \"User\" 1604.01 908.05))"));
        assert_eq!((user.1, user.2), (1604.01, 908.05));
        assert_eq!(paper(&parse("(kicad_sch)")).0, "A4");
    }

    #[test]
    fn a4_frame_has_border_grid_and_title_block() {
        let info = TitleInfo {
            title: "Power".into(),
            rev: "B".into(),
            paper: "A4".into(),
            sheet: 2,
            sheets: 3,
            ..Default::default()
        };
        let f = frame(297.0, 210.0, &info);
        // Outer and inner border.
        assert_eq!(f.lines[0], rect([10.0, 10.0], [287.0, 200.0]));
        assert_eq!(f.lines[1], rect([12.0, 12.0], [285.0, 198.0]));
        // 277 mm across: columns 1..6, 190 mm down: rows A..D.
        let labels: Vec<_> = f.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(labels.contains(&"6") && !labels.contains(&"7"));
        assert!(labels.contains(&"D") && !labels.contains(&"E"));
        assert!(labels.contains(&"Title: Power"));
        assert!(labels.contains(&"Rev: B"));
        assert!(labels.contains(&"Id: 2/3"));
        // Title block right edge sits 2 mm inside the outer border corner.
        let title = f.texts.iter().find(|t| t.text == "Title: Power").unwrap();
        assert_eq!(title.pos, [287.0 - 109.0, 200.0 - 10.7]);
    }

    #[test]
    fn notes_rectangles_and_buses_become_annotations() {
        let root = parse(
            r#"(kicad_sch
              (text "SECTION" (at 10 20 0) (effects (font (size 2 2) bold) (justify left)) (uuid "t1"))
              (text "hidden" (at 0 0 0) (effects (font (size 1 1)) (hide yes)) (uuid "t2"))
              (rectangle (start 0 0) (end 50 30) (stroke (width 0.254) (type dash)) (fill (type none)) (uuid "r1"))
              (rectangle (start 0 0) (end 5 5) (stroke (width 0) (type default)) (fill (type background)) (uuid "r2"))
              (polyline (pts (xy 0 0) (xy 10 0)) (stroke (width 0) (type default)) (uuid "p1"))
              (bus (pts (xy 0 0) (xy 0 10)) (stroke (width 0) (type default)) (uuid "b1"))
              (bus_entry (at 0 5) (size 2.54 2.54) (stroke (width 0) (type default)) (uuid "e1"))
              (text_box "Box note" (at 100 100 0) (size 20 10) (margins 1 1 1 1) (stroke (width 0.2) (type solid)) (fill (type none)) (effects (font (size 1.27 1.27))) (uuid "x1"))
              (symbol (lib_id "Device:R") (at 0 0 0) (rectangle (start 0 0) (end 1 1))))"#,
        );
        let a = annotations(&root);
        let texts: Vec<_> = a
            .iter()
            .filter_map(|x| match x {
                Annotation::Text { spec, .. } => Some(spec),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2, "hidden note skipped, text box text kept");
        assert_eq!(texts[0].size, [2.0, 2.0]);
        assert!(texts[0].bold && texts[0].justify_h == HJustify::Left);
        assert_eq!(texts[1].pos, [101.0, 101.0]);
        let shape = |id: &str| {
            a.iter()
                .find_map(|x| match x {
                    Annotation::Shape {
                        uuid,
                        outline,
                        fill,
                    } if uuid == id => Some((outline.clone(), fill.clone())),
                    _ => None,
                })
                .unwrap()
        };
        assert!(matches!(shape("r1").0, Prim::Strokes { ref strokes, .. } if strokes.len() > 20));
        assert!(matches!(shape("r2").1, Some((Fill::Background, _))));
        assert!(matches!(shape("p1").0, Prim::Polyline { .. }));
        assert!(matches!(shape("x1").0, Prim::Polygon { fill: false, .. }));
        assert!(a.iter().any(|x| matches!(x, Annotation::Bus { .. })));
        assert!(a.iter().any(|x| matches!(x, Annotation::BusEntry { .. })));
        // Symbol-internal graphics are not page annotations.
        assert_eq!(
            a.iter()
                .filter(|x| matches!(x, Annotation::Shape { .. }))
                .count(),
            4
        );
    }

    #[test]
    fn dash_runs_cover_pattern() {
        let runs = dashes(&[[0.0, 0.0], [10.0, 0.0]], 0.25, "dash").unwrap();
        // 1.5 mm dashes every 2.25 mm.
        assert_eq!(runs.len(), 5);
        assert!((runs[0][1][0] - 1.5).abs() < 1e-9);
        assert!(dashes(&[[0.0, 0.0], [1.0, 0.0]], 0.25, "default").is_none());
    }
}
