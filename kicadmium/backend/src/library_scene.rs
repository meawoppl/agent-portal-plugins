//! Reified vector scenes for library symbol and footprint previews.
//!
//! Library previews are content-addressed by the thumbnail renderer's key.
//! This module deliberately produces the same `vector-view` contract as the
//! project schematic; the browser never parses KiCad source files.

use crate::pcb_model::PcbBoard;
use anyhow::{anyhow, Result};
use kct::schema::library::{LibrarySymbol, SymbolGraphic, SymbolLibrary};
use vector_view::{
    Group, GroupKind, Item, Layer, LayerKind, Prim, Prop, Role, Scene, SceneKind, Side,
};

const SYMBOL_LAYER: u16 = 1;
const PIN_LAYER: u16 = 2;
const TEXT_LAYER: u16 = 3;

pub(crate) fn symbol_scene(library_text: &str, symbol_name: &str) -> Result<Scene> {
    let library = SymbolLibrary::load_from_string(library_text)?;
    let symbol = library
        .get_symbol(symbol_name)
        .or_else(|| library.symbols.values().last())
        .ok_or_else(|| anyhow!("symbol library contains no symbols"))?;
    Ok(scene_from_symbol(symbol))
}

fn scene_from_symbol(symbol: &LibrarySymbol) -> Scene {
    let mut scene = Scene::new(SceneKind::Symbol, true);
    scene.layers = vec![
        layer(
            SYMBOL_LAYER,
            "Symbol body",
            LayerKind::Symbol,
            10,
            [80, 180, 120, 255],
        ),
        layer(
            PIN_LAYER,
            "Pins",
            LayerKind::Symbol,
            20,
            [220, 120, 120, 255],
        ),
        layer(
            TEXT_LAYER,
            "Text",
            LayerKind::Text,
            30,
            [210, 210, 220, 255],
        ),
    ];
    scene.groups.push(Group {
        id: 1,
        kind: GroupKind::Symbol,
        label: symbol.name.clone(),
        props: symbol
            .properties
            .iter()
            .map(|(key, value)| Prop::new(key, value))
            .collect(),
    });
    scene.meta.push(Prop::new("symbol", &symbol.name));
    let mut id = 1;
    for graphic in &symbol.graphics {
        let prim = match graphic {
            SymbolGraphic::Polyline(g) if g.points.len() >= 2 => {
                let points = g.points.iter().map(|&(x, y)| [x, -y]).collect::<Vec<_>>();
                if g.fill_type != "none" && points.len() >= 3 {
                    Prim::Polygon {
                        outer: points,
                        holes: vec![],
                        fill: true,
                        stroke: g.stroke_width,
                    }
                } else {
                    Prim::Polyline {
                        points,
                        width: stroke_width(g.stroke_width),
                    }
                }
            }
            SymbolGraphic::Circle(g) => Prim::Circle {
                center: [g.center.0, -g.center.1],
                radius: g.radius,
                fill: g.fill_type != "none",
                stroke: g.stroke_width,
            },
            SymbolGraphic::Rectangle(g) => Prim::Polygon {
                outer: vec![
                    [g.start.0, -g.start.1],
                    [g.end.0, -g.start.1],
                    [g.end.0, -g.end.1],
                    [g.start.0, -g.end.1],
                ],
                holes: vec![],
                fill: g.fill_type != "none",
                stroke: g.stroke_width,
            },
            SymbolGraphic::Arc(g) => arc_prim(
                [g.start.0, -g.start.1],
                [g.mid.0, -g.mid.1],
                [g.end.0, -g.end.1],
                stroke_width(g.stroke_width),
            ),
            _ => continue,
        };
        push_item(
            &mut scene,
            &mut id,
            SYMBOL_LAYER,
            Role::SymbolBody,
            prim,
            vec![],
        );
    }
    for pin in &symbol.pins {
        let start = [pin.position.0, -pin.position.1];
        let a = (-pin.rotation).to_radians();
        let end = [
            start[0] + pin.length * a.cos(),
            start[1] + pin.length * a.sin(),
        ];
        push_item(
            &mut scene,
            &mut id,
            PIN_LAYER,
            Role::Pin,
            Prim::Polyline {
                points: vec![start, end],
                width: 0.15,
            },
            vec![
                Prop::new("number", &pin.number),
                Prop::new("name", &pin.name),
                Prop::new("type", &pin.pin_type),
            ],
        );
        add_text(
            &mut scene,
            &mut id,
            &pin.number,
            start,
            1.0,
            TEXT_LAYER,
            Role::Field,
        );
        if !pin.name.is_empty() && pin.name != "~" {
            add_text(
                &mut scene,
                &mut id,
                &pin.name,
                end,
                1.0,
                TEXT_LAYER,
                Role::Field,
            );
        }
    }
    scene.recompute_bbox();
    scene
}

/// Footprint previews use the board reifier, so pads get the same full
/// geometry (roundrect, chamfer, oval, trapezoid, custom) as the PCB tab.
pub(crate) fn footprint_scene(board: &PcbBoard) -> Scene {
    let mut scene = crate::pcb_scene::scene_from_board(board, "", "");
    scene.kind = SceneKind::Footprint;
    scene
}

fn layer(id: u16, name: &str, kind: LayerKind, z: i32, color: [u8; 4]) -> Layer {
    Layer {
        id,
        name: name.into(),
        kind,
        side: Side::None,
        z,
        color,
        visible: true,
    }
}
fn stroke_width(width: f64) -> f64 {
    if width > 0.0 {
        width
    } else {
        0.15
    }
}

fn push_item(
    scene: &mut Scene,
    id: &mut u32,
    layer: u16,
    role: Role,
    prim: Prim,
    props: Vec<Prop>,
) {
    scene.items.push(Item {
        id: *id,
        layer,
        role,
        prim,
        net: None,
        group: Some(1),
        props,
    });
    *id += 1;
}

fn add_text(
    scene: &mut Scene,
    id: &mut u32,
    text: &str,
    pos: [f64; 2],
    size: f64,
    layer: u16,
    role: Role,
) {
    let strokes = kicad_strokes::to_strokes(&kicad_strokes::TextSpec {
        text: text.into(),
        pos,
        size: [size, size],
        y_down: true,
        ..Default::default()
    });
    if !strokes.strokes.is_empty() {
        push_item(
            scene,
            id,
            layer,
            role,
            strokes.into_prim(),
            vec![Prop::new("text", text)],
        );
    }
}

fn arc_prim(a: [f64; 2], b: [f64; 2], c: [f64; 2], width: f64) -> Prim {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-9 {
        return Prim::Polyline {
            points: vec![a, c],
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
    #[test]
    fn symbol_is_reified() {
        let text = r#"(kicad_symbol_lib (version 20231120) (generator test) (symbol "R" (property "Value" "R") (symbol "R_0_1" (rectangle (start -1 -1) (end 1 1) (stroke (width .2) (type default)) (fill (type none)))) (symbol "R_1_1" (pin passive line (at -2 0 0) (length 1) (name "A") (number "1")))))"#;
        let scene = symbol_scene(text, "R").unwrap();
        assert_eq!(scene.kind, SceneKind::Symbol);
        assert!(scene.items.iter().any(|i| i.role == Role::Pin));
        assert!(!scene.bbox.is_empty());
    }

    #[test]
    fn footprint_pads_keep_full_geometry() {
        let text = r#"(kicad_pcb (version 20240108) (generator test)
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (37 "F.SilkS" user "F.Silkscreen"))
  (footprint "Test:FP" (layer "F.Cu") (at 0 0)
    (property "Reference" "REF**" (at 0 -2 0) (layer "F.SilkS") (effects (font (size 1 1) (thickness 0.15))))
    (pad "1" smd roundrect (at -1 0) (size 1 1.4) (layers "F.Cu") (roundrect_rratio 0.25))
    (pad "2" thru_hole oval (at 1 0) (size 1.2 2) (drill oval 0.6 1) (layers "*.Cu"))))"#;
        let board = crate::pcb_view::build_board(text).unwrap();
        let scene = footprint_scene(&board);
        assert_eq!(scene.kind, SceneKind::Footprint);
        let pads: Vec<_> = scene.items.iter().filter(|i| i.role == Role::Pad).collect();
        assert!(!pads.is_empty());
        // Rounded and oval outlines are arcs sampled into many vertices; the
        // old preview emitted four-corner rectangles for every non-circle pad.
        for pad in &pads {
            if let Prim::Polygon { outer, .. } = &pad.prim {
                assert!(outer.len() > 4, "pad flattened to a rectangle");
            }
        }
        assert!(scene.items.iter().any(|i| i.role == Role::Hole));
        assert!(scene.items.iter().any(|i| i.role == Role::Text));
    }
}
