//! Footprints placed outside the board outline (port of
//! `kicad_tools.validate.rules.placement`).

use super::edge::min_distance_to_outline;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

/// Ray-casting point-in-polygon (`point_in_polygon`).
pub fn point_in_polygon(x: f64, y: f64, polygon: &[(f64, f64)]) -> bool {
    let n = polygon.len();
    if n == 0 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[derive(Debug, Clone, Default)]
pub struct FootprintOutsideBoardRule;

impl FootprintOutsideBoardRule {
    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        let outline = pcb.get_board_outline();
        if outline.len() < 3 {
            results.rules_checked += 1;
            return results;
        }
        let segs = pcb.get_board_outline_segments();
        for fp in pcb.footprints() {
            let (fx, fy) = fp.position;
            if !point_in_polygon(fx, fy, &outline) {
                let d = min_distance_to_outline((fx, fy), &segs);
                results.add(
                    DRCViolation::new(
                        "footprint_outside_board",
                        "error",
                        format!(
                            "Footprint {} at ({fx:.2}, {fy:.2}) is outside the board outline by {d:.2}mm",
                            fp.reference
                        ),
                    )
                    .at(fx, fy)
                    .layer(fp.layer.clone())
                    .actual(d)
                    .required(0.0)
                    .items([fp.reference.clone()]),
                );
            }
        }
        results.rules_checked += 1;
        results
    }
}
