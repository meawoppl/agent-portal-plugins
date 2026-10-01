//! Connector mating / edge-access rule (port of
//! `kicad_tools.validate.rules.connector_access`, Issue #4613).

use crate::geometry::courtyard::fp_transform;
use crate::geometry::pcb_adapters::courtyard_geom;
use crate::geometry::shapely::{self as sh, Geom};
use crate::manufacturers::DesignRules;
use crate::schema::pcb::{Footprint, Pcb};
use crate::validate::violations::{DRCResults, DRCViolation};

pub const CONNECTOR_EDGE_ACCESS_RULE_ID: &str = "connector_edge_access";
pub const CONNECTOR_EDGE_DISTANCE_RULE_ID: &str = "connector_edge_distance";
pub const CONNECTOR_EDGE_ACCESS_MAX_MM: f64 = 3.0;
const EPSILON_MM: f64 = 1e-4;

/// `EDGE_ACCESS_FAMILIES`: `(library, needs "_Horizontal")`.
const EDGE_ACCESS_FAMILIES: &[(&str, bool)] = &[
    ("Connector_Audio", true),
    ("Connector_USB", true),
    ("Connector_BarrelJack", false),
    ("Connector_RJ", true),
    ("Connector_Card", false),
];

fn library_prefix(name: &str) -> &str {
    match name.split_once(':') {
        Some((lib, _)) => lib,
        None => "",
    }
}

fn needs_edge_access(lib: &str, name: &str) -> bool {
    let Some((_, horizontal)) = EDGE_ACCESS_FAMILIES.iter().find(|(l, _)| *l == lib) else {
        return false;
    };
    if name.contains("_Vertical") {
        return false;
    }
    !(*horizontal && !name.contains("_Horizontal"))
}

#[derive(Debug, Clone)]
pub struct ConnectorEdgeAccessRule {
    pub emit_inventory: bool,
    pub max_edge_distance_mm: f64,
}

impl Default for ConnectorEdgeAccessRule {
    fn default() -> Self {
        Self::new(false)
    }
}

impl ConnectorEdgeAccessRule {
    pub fn new(emit_inventory: bool) -> Self {
        ConnectorEdgeAccessRule {
            emit_inventory,
            max_edge_distance_mm: CONNECTOR_EDGE_ACCESS_MAX_MM,
        }
    }

    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        let outline = pcb.get_board_outline_segments();
        if outline.is_empty() {
            return results;
        }
        let lines: Vec<Geom> = outline.iter().map(|(a, b)| Geom::Line(vec![*a, *b])).collect();
        for fp in pcb.footprints() {
            let lib = library_prefix(&fp.name);
            if !lib.starts_with("Connector_") {
                continue;
            }
            let g = footprint_geometry(fp);
            let mut distance = f64::INFINITY;
            for (i, l) in lines.iter().enumerate() {
                let d = sh::distance(&g, l);
                if i == 0 || d < distance {
                    distance = d;
                }
            }
            if self.emit_inventory {
                results.add(
                    DRCViolation::new(
                        CONNECTOR_EDGE_DISTANCE_RULE_ID,
                        "info",
                        format!(
                            "{} ({}): {distance:.2}mm from nearest board edge",
                            fp.reference, fp.name
                        ),
                    )
                    .at(fp.position.0, fp.position.1)
                    .layer(fp.layer.clone())
                    .actual(distance)
                    .items([fp.reference.clone()]),
                );
            }
            if needs_edge_access(lib, &fp.name) && distance > self.max_edge_distance_mm + EPSILON_MM {
                results.add(
                    DRCViolation::new(
                        CONNECTOR_EDGE_ACCESS_RULE_ID,
                        "warning",
                        format!(
                            "{} ({}) is a horizontal-mating connector {distance:.2}mm from the \
                             nearest board edge (max {:.2}mm for plug access) -- a mating plug may \
                             not physically reach it; waive via .kct_waivers.json if intentional",
                            fp.reference, fp.name, self.max_edge_distance_mm
                        ),
                    )
                    .at(fp.position.0, fp.position.1)
                    .layer(fp.layer.clone())
                    .actual(distance)
                    .required(self.max_edge_distance_mm)
                    .items([fp.reference.clone()]),
                );
            }
        }
        results.rules_checked += 1;
        results
    }
}

/// `_footprint_geometry`: courtyard, else pad-extent box, else origin.
pub fn footprint_geometry(fp: &Footprint) -> Geom {
    let sides = if fp.layer.starts_with("B.") {
        ["B", "F"]
    } else {
        ["F", "B"]
    };
    for side in sides {
        if let Some(g) = courtyard_geom(fp, side) {
            return g;
        }
    }
    if !fp.pads.is_empty() {
        let t = fp_transform(fp);
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for pad in &fp.pads {
            let (cx, cy) = t(pad.position);
            let half = pad.size.0.max(pad.size.1) / 2.0;
            xs.extend([cx - half, cx + half]);
            ys.extend([cy - half, cy + half]);
        }
        let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
        let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        return sh::box_poly(min(&xs), min(&ys), max(&xs), max(&ys));
    }
    Geom::Point(fp.position)
}
