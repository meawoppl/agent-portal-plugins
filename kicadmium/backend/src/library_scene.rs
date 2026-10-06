//! Reified vector scenes for library symbol and footprint previews.
//!
//! Library previews are content-addressed by the thumbnail renderer's key.
//! This module deliberately produces the same `vector-view` contract as the
//! project schematic; the browser never parses KiCad source files.

use crate::pcb_model::PcbBoard;
use crate::schematic_view::palette;
use crate::symbol_graphics::{body_prims, Fill};
use anyhow::{anyhow, Result};
use kct::schema::library::{LibrarySymbol, SymbolLibrary};
use vector_view::{
    Group, GroupKind, Item, Layer, LayerKind, Prim, Prop, Role, Scene, SceneKind, Side,
};

const SYMBOL_LAYER: u16 = 1;
const PIN_LAYER: u16 = 2;
const TEXT_LAYER: u16 = 3;
const FILL_LAYER: u16 = 4;

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
    // Same palette and fill semantics as the project schematic.
    scene.layers = vec![
        layer(
            FILL_LAYER,
            "Symbol body fill",
            LayerKind::Symbol,
            5,
            palette::BODY_FILL,
        ),
        layer(
            SYMBOL_LAYER,
            "Symbol body",
            LayerKind::Symbol,
            10,
            palette::BODY,
        ),
        layer(PIN_LAYER, "Pins", LayerKind::Symbol, 20, palette::PIN),
        layer(TEXT_LAYER, "Text", LayerKind::Text, 30, palette::PIN_TEXT),
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
        let Some(body) = body_prims(graphic, |(x, y)| [x, -y], stroke_width) else {
            continue;
        };
        if let Some((fill, prim)) = body.fill {
            let layer = match fill {
                Fill::Outline => SYMBOL_LAYER,
                _ => FILL_LAYER,
            };
            push_item(&mut scene, &mut id, layer, Role::SymbolBody, prim, vec![]);
        }
        push_item(
            &mut scene,
            &mut id,
            SYMBOL_LAYER,
            Role::SymbolBody,
            body.outline,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_body_is_a_wash_not_an_opaque_box() {
        let text = r#"(kicad_symbol_lib (version 20231120) (generator test) (symbol "U" (symbol "U_0_1" (rectangle (start -5 5) (end 5 -5) (stroke (width .254) (type default)) (fill (type background)))) (symbol "U_1_1" (pin passive line (at -10 0 0) (length 5) (name "A") (number "1")))))"#;
        let scene = symbol_scene(text, "U").unwrap();
        let bodies: Vec<_> = scene
            .items
            .iter()
            .filter(|i| i.role == Role::SymbolBody)
            .collect();
        assert_eq!(bodies.len(), 2);
        assert!(bodies
            .iter()
            .any(|i| i.layer == FILL_LAYER && matches!(i.prim, Prim::Polygon { fill: true, .. })));
        assert!(bodies.iter().any(
            |i| i.layer == SYMBOL_LAYER && matches!(i.prim, Prim::Polygon { fill: false, .. })
        ));
        let fill = scene.layers.iter().find(|l| l.id == FILL_LAYER).unwrap();
        assert!(fill.color[3] < 64 && fill.z < 10);
    }

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
