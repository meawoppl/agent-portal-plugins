//! Symbol field placement geometry (port of `kicad_tools.schema.field_geometry`).
//!
//! Library geometry is Y-up relative to the symbol origin; placed (sheet)
//! coordinates are Y-down. The transform matches
//! [`LibrarySymbol::get_pin_position`]: mirror, rotate, negate Y, translate.
//! Bounding boxes are `(min_x, min_y, max_x, max_y)` in sheet coordinates.

use super::library::{py_round, transform_offset, LibrarySymbol, SymbolGraphic, KICAD_GRID};
use super::symbol::OrderedMap;

/// Bounding box in sheet coordinates: `(min_x, min_y, max_x, max_y)`.
pub type BBox = (f64, f64, f64, f64);

/// Clearance between the body bbox edge and a tidied field anchor (mm).
pub const DEFAULT_FIELD_CLEARANCE_MM: f64 = 1.27;

/// Snap to the grid, rounded to 2 decimals (strips float noise).
fn snap_to_grid(value: f64) -> f64 {
    let snapped = py_round(value / KICAD_GRID) * KICAD_GRID;
    py_round(snapped * 100.0) / 100.0
}

/// Library-coordinate points spanning the body: the unit's pins for a known
/// unit of a multi-unit symbol, else graphics extents, else all pins.
fn local_extent_points(lib_symbol: &LibrarySymbol, unit: Option<i64>) -> Vec<(f64, f64)> {
    if let Some(unit) = unit {
        if lib_symbol.units > 1 {
            let pins: Vec<(f64, f64)> = lib_symbol
                .pins
                .iter()
                .filter(|p| p.unit == unit)
                .map(|p| p.position)
                .collect();
            if !pins.is_empty() {
                return pins;
            }
        }
    }
    let mut points = Vec::new();
    for g in &lib_symbol.graphics {
        match g {
            SymbolGraphic::Rectangle(r) => points.extend([r.start, r.end]),
            SymbolGraphic::Polyline(p) => points.extend(p.points.iter().copied()),
            SymbolGraphic::Circle(c) => {
                let (cx, cy) = c.center;
                points.push((cx - c.radius, cy - c.radius));
                points.push((cx + c.radius, cy + c.radius));
            }
            SymbolGraphic::Arc(a) => points.extend([a.start, a.mid, a.end]),
        }
    }
    if points.is_empty() {
        points = lib_symbol.pins.iter().map(|p| p.position).collect();
    }
    points
}

/// Placed body bounding box of a symbol (degenerate at `position` if the
/// symbol has neither graphics nor pins).
pub fn placed_body_bbox(
    lib_symbol: &LibrarySymbol,
    position: (f64, f64),
    rotation: f64,
    mirror: &str,
    unit: Option<i64>,
) -> BBox {
    let points = local_extent_points(lib_symbol, unit);
    if points.is_empty() {
        return (position.0, position.1, position.0, position.1);
    }
    let mut bbox = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for (lx, ly) in points {
        let (dx, dy) = transform_offset(lx, ly, rotation, mirror);
        let (x, y) = (position.0 + dx, position.1 + dy);
        bbox.0 = bbox.0.min(x);
        bbox.1 = bbox.1.min(y);
        bbox.2 = bbox.2.max(x);
        bbox.3 = bbox.3.max(y);
    }
    bbox
}

/// Distance from a field anchor to a bbox (0 inside or on the edge).
pub fn field_offset_mm(field_at: (f64, f64), bbox: BBox) -> f64 {
    let (x, y) = field_at;
    let (min_x, min_y, max_x, max_y) = bbox;
    let dx = (min_x - x).max(0.0).max(x - max_x);
    let dy = (min_y - y).max(0.0).max(y - max_y);
    dx.hypot(dy)
}

/// Default `Reference` (centered above) and `Value` (centered below)
/// positions `clearance` mm from the bbox, grid-snapped: name -> `(x, y, 0)`.
pub fn default_field_positions(bbox: BBox, clearance: f64) -> OrderedMap<(f64, f64, f64)> {
    let (min_x, min_y, max_x, max_y) = bbox;
    let center_x = snap_to_grid((min_x + max_x) / 2.0);
    let mut out = OrderedMap::new();
    out.insert(
        "Reference",
        (center_x, snap_to_grid(min_y - clearance), 0.0),
    );
    out.insert("Value", (center_x, snap_to_grid(max_y + clearance), 0.0));
    out
}
