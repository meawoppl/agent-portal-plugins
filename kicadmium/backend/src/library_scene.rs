//! Reified vector scenes for library symbol and footprint previews.
//!
//! Library previews are content-addressed by the thumbnail renderer's key.
//! This module deliberately produces the same `eda_view::vector` contract as the
//! project schematic; the browser never parses KiCad source files.

use crate::pcb_model::PcbBoard;
use anyhow::{anyhow, Result};
use eda_view::kicad::schema::library::SymbolLibrary;
use eda_view::schematic::Palette;
use eda_view::vector::{Scene, SceneKind};

/// Symbol previews come from `eda_view::schematic`, so library symbols use the
/// same body geometry, fill semantics and palette as the project schematic.
pub(crate) fn symbol_scene(library_text: &str, symbol_name: &str) -> Result<Scene> {
    let library = SymbolLibrary::load_from_string(library_text)?;
    let symbol = library
        .get_symbol(symbol_name)
        .or_else(|| library.symbols.values().last())
        .ok_or_else(|| anyhow!("symbol library contains no symbols"))?;
    Ok(eda_view::schematic::symbol_scene(
        symbol,
        &Palette::default(),
    ))
}

/// Footprint previews use the board reifier, so pads get the same full
/// geometry (roundrect, chamfer, oval, trapezoid, custom) as the PCB tab.
pub(crate) fn footprint_scene(board: &PcbBoard) -> Scene {
    let mut scene = crate::pcb_scene::scene_from_board(board, "", "");
    scene.kind = SceneKind::Footprint;
    scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_view::vector::{Prim, Role};

    /// Layer ids of `eda_view::schematic::symbol_scene`.
    const SYMBOL_LAYER: u16 = 1;
    const FILL_LAYER: u16 = 4;

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
