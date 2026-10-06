//! KiCad symbol body graphics as `vector-view` primitives, shared by the
//! schematic scene and library symbol previews.
//!
//! KiCad's `(fill (type ...))` has three meanings that a single per-layer
//! fill colour cannot express, so every graphic is split into a stroke-only
//! outline plus an optional fill-only shape the caller routes to a layer:
//!
//! - `none`: outline only.
//! - `outline`: filled with the stroke colour (diode/op-amp triangles,
//!   pin-1 dots), so the fill belongs on the outline's own layer.
//! - `background` (and KiCad 8 `color`): filled with the theme's body
//!   background, a subtle wash drawn on its own layer *under* wires, pins and
//!   text so it can never hide them.

use kct::schema::library::SymbolGraphic;
use vector_view::Prim;

/// KiCad symbol fill semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fill {
    None,
    /// Filled with the stroke colour.
    Outline,
    /// Filled with the body background colour.
    Background,
}

impl Fill {
    pub(crate) fn parse(fill_type: &str) -> Self {
        match fill_type {
            "outline" => Fill::Outline,
            // `color` carries an explicit RGBA we cannot express per item;
            // the body background is the closest non-obscuring rendering.
            "background" | "color" => Fill::Background,
            _ => Fill::None,
        }
    }
}

/// One symbol graphic split by paint: an unfilled outline and, when the
/// graphic is filled, a stroke-less fill shape with its [`Fill`] kind.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BodyPrims {
    pub outline: Prim,
    pub fill: Option<(Fill, Prim)>,
}

/// Convert one library graphic. `map` takes library coordinates to scene
/// coordinates; `width` resolves a KiCad stroke width (0 = default).
pub(crate) fn body_prims(
    graphic: &SymbolGraphic,
    map: impl Fn((f64, f64)) -> [f64; 2],
    width: impl Fn(f64) -> f64,
) -> Option<BodyPrims> {
    let polygon = |outer: Vec<[f64; 2]>, filled: bool, stroke: f64| Prim::Polygon {
        outer,
        holes: vec![],
        fill: filled,
        stroke,
    };
    let fill_of = |fill_type: &str, prim: Prim| match Fill::parse(fill_type) {
        Fill::None => None,
        f => Some((f, prim)),
    };
    Some(match graphic {
        SymbolGraphic::Polyline(g) => {
            let points: Vec<_> = g.points.iter().map(|&p| map(p)).collect();
            if points.len() < 2 {
                return None;
            }
            // KiCad fills a polyline as the closed polygon of its points but
            // strokes only the open path.
            let fill = if points.len() >= 3 {
                fill_of(&g.fill_type, polygon(points.clone(), true, 0.0))
            } else {
                None
            };
            BodyPrims {
                outline: Prim::Polyline {
                    points,
                    width: width(g.stroke_width),
                },
                fill,
            }
        }
        SymbolGraphic::Rectangle(g) => {
            let a = map(g.start);
            let c = map(g.end);
            let outer = vec![a, [c[0], a[1]], c, [a[0], c[1]]];
            BodyPrims {
                outline: polygon(outer.clone(), false, width(g.stroke_width)),
                fill: fill_of(&g.fill_type, polygon(outer, true, 0.0)),
            }
        }
        SymbolGraphic::Circle(g) => {
            let center = map(g.center);
            let circle = |filled: bool, stroke: f64| Prim::Circle {
                center,
                radius: g.radius,
                fill: filled,
                stroke,
            };
            BodyPrims {
                outline: circle(false, width(g.stroke_width)),
                fill: fill_of(&g.fill_type, circle(true, 0.0)),
            }
        }
        // Filled arcs are rare in symbol libraries; draw the stroke only.
        SymbolGraphic::Arc(g) => BodyPrims {
            outline: arc_prim(map(g.start), map(g.mid), map(g.end), width(g.stroke_width)),
            fill: None,
        },
    })
}

/// Three-point arc through `a`, `b`, `c` (scene coordinates). The sweep is
/// oriented so it passes through the mid point `b`.
pub(crate) fn arc_prim(a: [f64; 2], b: [f64; 2], c: [f64; 2], width: f64) -> Prim {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-9 {
        return Prim::Polyline {
            points: vec![a, b, c],
            width,
        };
    }
    let aa = a[0] * a[0] + a[1] * a[1];
    let bb = b[0] * b[0] + b[1] * b[1];
    let cc = c[0] * c[0] + c[1] * c[1];
    let center = [
        (aa * (b[1] - c[1]) + bb * (c[1] - a[1]) + cc * (a[1] - b[1])) / d,
        (aa * (c[0] - b[0]) + bb * (a[0] - c[0]) + cc * (b[0] - a[0])) / d,
    ];
    let angle = |p: [f64; 2]| (p[1] - center[1]).atan2(p[0] - center[0]);
    let mut start = angle(a);
    let mut end = angle(c);
    let mid = angle(b);
    let tau = std::f64::consts::TAU;
    if (mid - start).rem_euclid(tau) > (end - start).rem_euclid(tau) {
        std::mem::swap(&mut start, &mut end);
    }
    Prim::Arc {
        center,
        radius: (a[0] - center[0]).hypot(a[1] - center[1]),
        start,
        end,
        width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kct::schema::library::{SymbolCircle, SymbolPolyline, SymbolRectangle};

    fn id(p: (f64, f64)) -> [f64; 2] {
        [p.0, p.1]
    }
    fn w(x: f64) -> f64 {
        if x > 0.0 {
            x
        } else {
            0.254
        }
    }
    fn rect(fill: &str) -> SymbolGraphic {
        SymbolGraphic::Rectangle(SymbolRectangle {
            start: (-1.0, -2.0),
            end: (1.0, 2.0),
            stroke_width: 0.0,
            stroke_type: "default".into(),
            fill_type: fill.into(),
        })
    }
    fn is_filled(p: &Prim) -> bool {
        match p {
            Prim::Polygon { fill, .. } | Prim::Circle { fill, .. } => *fill,
            _ => false,
        }
    }

    #[test]
    fn fill_types_parse_to_kicad_semantics() {
        assert_eq!(Fill::parse("none"), Fill::None);
        assert_eq!(Fill::parse(""), Fill::None);
        assert_eq!(Fill::parse("outline"), Fill::Outline);
        assert_eq!(Fill::parse("background"), Fill::Background);
        assert_eq!(Fill::parse("color"), Fill::Background);
    }

    #[test]
    fn outline_is_never_filled() {
        for fill in ["none", "outline", "background"] {
            let b = body_prims(&rect(fill), id, w).unwrap();
            assert!(!is_filled(&b.outline), "{fill} outline must stay unfilled");
            assert!(matches!(
                b.outline,
                Prim::Polygon { stroke, .. } if stroke == 0.254
            ));
        }
    }

    #[test]
    fn fill_shape_follows_fill_type() {
        assert!(body_prims(&rect("none"), id, w).unwrap().fill.is_none());
        let (kind, prim) = body_prims(&rect("background"), id, w)
            .unwrap()
            .fill
            .unwrap();
        assert_eq!(kind, Fill::Background);
        // Fill-only: no stroke, so the outline item owns the line colour.
        assert!(matches!(prim, Prim::Polygon { fill: true, stroke, .. } if stroke == 0.0));
        let (kind, _) = body_prims(&rect("outline"), id, w).unwrap().fill.unwrap();
        assert_eq!(kind, Fill::Outline);
    }

    #[test]
    fn polylines_and_circles_split_too() {
        let tri = SymbolGraphic::Polyline(SymbolPolyline {
            points: vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)],
            stroke_width: 0.2,
            stroke_type: "default".into(),
            fill_type: "outline".into(),
        });
        let b = body_prims(&tri, id, w).unwrap();
        assert!(matches!(b.outline, Prim::Polyline { width, .. } if width == 0.2));
        assert!(matches!(
            b.fill,
            Some((Fill::Outline, Prim::Polygon { .. }))
        ));

        let dot = SymbolGraphic::Circle(SymbolCircle {
            center: (0.0, 0.0),
            radius: 1.0,
            stroke_width: 0.0,
            stroke_type: "default".into(),
            fill_type: "background".into(),
        });
        let b = body_prims(&dot, id, w).unwrap();
        assert!(!is_filled(&b.outline));
        assert!(matches!(
            b.fill,
            Some((Fill::Background, Prim::Circle { fill: true, .. }))
        ));
    }

    #[test]
    fn arc_reifies_through_mid() {
        assert!(matches!(
            arc_prim([1., 0.], [0., 1.], [-1., 0.], 0.2),
            Prim::Arc { .. }
        ))
    }
}
