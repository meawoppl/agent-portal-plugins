//! Reified vector scenes for library symbol and footprint previews.
//!
//! Library previews are content-addressed by the thumbnail renderer's key.
//! This module deliberately produces the same `vector-view` contract as the
//! project schematic; the browser never parses KiCad source files.

use anyhow::{anyhow, Result};
use kct::schema::library::{LibrarySymbol, SymbolGraphic, SymbolLibrary};
use shared::pcb::{PcbBoard, PcbGraphic, PcbShape, PcbText};
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

pub(crate) fn footprint_scene(board: &PcbBoard) -> Scene {
    let mut scene = Scene::new(SceneKind::Footprint, true);
    scene.layers = board
        .layers
        .iter()
        .enumerate()
        .map(|(index, source)| Layer {
            id: index as u16,
            name: source.name.clone(),
            kind: pcb_layer_kind(&source.name),
            side: if source.name.starts_with("F.") {
                Side::Front
            } else if source.name.starts_with("B.") {
                Side::Back
            } else {
                Side::None
            },
            z: index as i32,
            color: pcb_layer_color(&source.name),
            visible: !source.disabled,
        })
        .collect();
    let mut id = 1;
    for footprint in &board.footprints {
        scene.groups.push(Group {
            id: 1,
            kind: GroupKind::Footprint,
            label: footprint.reference.clone(),
            props: vec![
                Prop::new("value", &footprint.value),
                Prop::new("footprint", &footprint.footprint),
            ],
        });
        for pad in &footprint.pads {
            let radius = pad.size[0].min(pad.size[1]) / 2.0;
            let prim = if pad.shape == "circle" {
                Prim::Circle {
                    center: pad.pos,
                    radius,
                    fill: true,
                    stroke: 0.0,
                }
            } else {
                let (w, h) = (pad.size[0] / 2.0, pad.size[1] / 2.0);
                Prim::Polygon {
                    outer: rotated_rect(pad.pos, w, h, pad.angle),
                    holes: vec![],
                    fill: true,
                    stroke: 0.0,
                }
            };
            for &layer in &pad.layers {
                push_grouped(
                    &mut scene,
                    &mut id,
                    layer,
                    Role::Pad,
                    prim.clone(),
                    vec![
                        Prop::new("number", &pad.number),
                        Prop::new("type", &pad.kind),
                    ],
                );
            }
            if let Some(drill) = &pad.drill {
                push_grouped(
                    &mut scene,
                    &mut id,
                    pad.layers.first().copied().unwrap_or(0),
                    Role::Hole,
                    Prim::Hole {
                        center: pad.pos,
                        size: drill.size,
                        rotation: pad.angle.to_radians(),
                    },
                    vec![Prop::new("pad", &pad.number)],
                );
            }
        }
        for graphic in &footprint.graphics {
            add_graphic(&mut scene, &mut id, graphic);
        }
        for text in &footprint.texts {
            add_pcb_text(&mut scene, &mut id, text);
        }
    }
    for graphic in &board.graphics {
        add_graphic(&mut scene, &mut id, graphic);
    }
    for text in &board.texts {
        add_pcb_text(&mut scene, &mut id, text);
    }
    scene.recompute_bbox();
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
fn push_grouped(
    scene: &mut Scene,
    id: &mut u32,
    layer: u16,
    role: Role,
    prim: Prim,
    props: Vec<Prop>,
) {
    push_item(scene, id, layer, role, prim, props);
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

fn add_pcb_text(scene: &mut Scene, id: &mut u32, text: &PcbText) {
    if !text.visible {
        return;
    }
    let strokes = kicad_strokes::to_strokes(&kicad_strokes::TextSpec {
        text: text.text.clone(),
        pos: text.pos,
        size: text.size,
        thickness: text.thickness,
        angle_deg: text.angle,
        mirror: text.mirrored,
        italic: text.italic,
        bold: text.bold,
        line_spacing: text.line_spacing,
        y_down: true,
        ..Default::default()
    });
    if !strokes.strokes.is_empty() {
        push_grouped(
            scene,
            id,
            text.layer,
            Role::Text,
            strokes.into_prim(),
            vec![Prop::new("text", &text.text), Prop::new("kind", &text.kind)],
        );
    }
}

fn add_graphic(scene: &mut Scene, id: &mut u32, graphic: &PcbGraphic) {
    let prim = match &graphic.shape {
        PcbShape::Segment { a, b } => Prim::Polyline {
            points: vec![*a, *b],
            width: graphic.width,
        },
        PcbShape::Polyline { pts } => Prim::Polyline {
            points: pts.clone(),
            width: graphic.width,
        },
        PcbShape::Polygon { pts } => Prim::Polygon {
            outer: pts.clone(),
            holes: vec![],
            fill: graphic.filled,
            stroke: graphic.width,
        },
        PcbShape::Circle { c, r } => Prim::Circle {
            center: *c,
            radius: *r,
            fill: graphic.filled,
            stroke: graphic.width,
        },
        PcbShape::Arc { c, r, start, end } => Prim::Arc {
            center: *c,
            radius: *r,
            start: *start,
            end: *end,
            width: graphic.width,
        },
    };
    push_grouped(
        scene,
        id,
        graphic.layer,
        if scene
            .layers
            .get(graphic.layer as usize)
            .is_some_and(|l| l.kind == LayerKind::EdgeCuts)
        {
            Role::Outline
        } else {
            Role::Graphic
        },
        prim,
        vec![],
    );
}

fn rotated_rect(center: [f64; 2], hw: f64, hh: f64, degrees: f64) -> Vec<[f64; 2]> {
    let (s, c) = degrees.to_radians().sin_cos();
    [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]]
        .into_iter()
        .map(|[x, y]| [center[0] + x * c + y * s, center[1] - x * s + y * c])
        .collect()
}

fn pcb_layer_kind(name: &str) -> LayerKind {
    match name {
        n if n.ends_with(".Cu") => LayerKind::Copper,
        n if n.ends_with(".SilkS") => LayerKind::Silkscreen,
        n if n.ends_with(".Mask") => LayerKind::SolderMask,
        n if n.ends_with(".Paste") => LayerKind::Paste,
        "Edge.Cuts" => LayerKind::EdgeCuts,
        n if n.ends_with(".Fab") => LayerKind::Fabrication,
        n if n.ends_with(".CrtYd") => LayerKind::Courtyard,
        _ => LayerKind::Drawing,
    }
}
fn pcb_layer_color(name: &str) -> [u8; 4] {
    match pcb_layer_kind(name) {
        LayerKind::Copper => [210, 80, 70, 220],
        LayerKind::Silkscreen => [230, 230, 225, 255],
        LayerKind::EdgeCuts => [230, 210, 70, 255],
        LayerKind::SolderMask => [70, 150, 90, 120],
        _ => [130, 150, 190, 230],
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
}
