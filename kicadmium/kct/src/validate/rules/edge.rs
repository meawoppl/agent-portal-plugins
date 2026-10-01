//! Copper-to-edge and hole-to-edge clearance rules (port of
//! `kicad_tools.validate.rules.edge`).

use crate::core::geometry::point_to_segment_distance;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

const CLEARANCE_EPSILON_MM: f64 = 1e-4;

pub type OutlineSeg = ((f64, f64), (f64, f64));

#[derive(Debug, Clone, Default)]
pub struct EdgeClearanceRule;

/// `_min_distance_to_outline`.
pub fn min_distance_to_outline(point: (f64, f64), outline: &[OutlineSeg]) -> f64 {
    let mut min = f64::INFINITY;
    for (a, b) in outline {
        let d = point_to_segment_distance(point.0, point.1, a.0, a.1, b.0, b.1);
        // Python `min(min_dist, dist)` keeps the first on ties / NaN.
        if d < min {
            min = d;
        }
    }
    min
}

impl EdgeClearanceRule {
    pub fn check(&self, pcb: &Pcb, rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        let outline = pcb.get_board_outline_segments();
        if outline.is_empty() {
            return results;
        }

        let min = rules.min_copper_to_edge_mm;
        for s in pcb.segments() {
            let half = s.width / 2.0;
            for p in [s.start, s.end] {
                let actual = min_distance_to_outline(p, &outline) - half;
                if actual < min - CLEARANCE_EPSILON_MM {
                    results.add(
                        DRCViolation::new(
                            "edge_clearance_trace",
                            "error",
                            format!("Trace to board edge {actual:.3}mm < minimum {min:.2}mm"),
                        )
                        .at(p.0, p.1)
                        .layer(s.layer.clone())
                        .actual(actual)
                        .required(min)
                        .items([format!("Net {}", s.net_number)]),
                    );
                }
            }
        }
        results.rules_checked += 1;

        let min_hole = rules.min_hole_to_edge_mm;
        for v in pcb.vias() {
            let actual = min_distance_to_outline(v.position, &outline) - v.size / 2.0;
            if actual < min_hole - CLEARANCE_EPSILON_MM {
                results.add(
                    DRCViolation::new(
                        "edge_clearance_via",
                        "error",
                        format!("Via to board edge {actual:.3}mm < minimum {min_hole:.2}mm"),
                    )
                    .at(v.position.0, v.position.1)
                    .layer_opt(v.layers.first().cloned())
                    .actual(actual)
                    .required(min_hole)
                    .items([format!("Net {}", v.net_number)]),
                );
            }
        }
        results.rules_checked += 1;

        for fp in pcb.footprints() {
            let (fx, fy) = fp.position;
            let rot = (-fp.rotation).to_radians();
            let (c, s) = (rot.cos(), rot.sin());
            for pad in &fp.pads {
                let (lx, ly) = pad.position;
                let pos = (fx + (lx * c - ly * s), fy + (lx * s + ly * c));
                let dist = min_distance_to_outline(pos, &outline);
                let actual = dist - pad.size.0.max(pad.size.1) / 2.0;
                let (req, rule) = if pad.pad_type == "thru_hole" {
                    (min_hole, "edge_clearance_pad_hole")
                } else {
                    (min, "edge_clearance_pad")
                };
                if actual < req - CLEARANCE_EPSILON_MM {
                    results.add(
                        DRCViolation::new(
                            rule,
                            "error",
                            format!(
                                "Pad {} to board edge {actual:.3}mm < minimum {req:.2}mm",
                                pad.number
                            ),
                        )
                        .at(pos.0, pos.1)
                        .layer(fp.layer.clone())
                        .actual(actual)
                        .required(req)
                        .items([fp.reference.clone(), format!("Pad {}", pad.number)]),
                    );
                }
            }
        }
        results.rules_checked += 1;

        for z in pcb.zones() {
            if z.keepout.is_some() {
                continue;
            }
            let polys: Vec<&Vec<(f64, f64)>> = if z.filled_polygons.is_empty() {
                vec![&z.polygon]
            } else {
                z.filled_polygons.iter().collect()
            };
            for poly in polys {
                for &p in poly {
                    let d = min_distance_to_outline(p, &outline);
                    if d < min - CLEARANCE_EPSILON_MM {
                        let item = if z.net_name.is_empty() {
                            format!("Net {}", z.net_number)
                        } else {
                            z.net_name.clone()
                        };
                        results.add(
                            DRCViolation::new(
                                "edge_clearance_zone",
                                "error",
                                format!("Zone copper to board edge {d:.3}mm < minimum {min:.2}mm"),
                            )
                            .at(p.0, p.1)
                            .layer(z.layer.clone())
                            .actual(d)
                            .required(min)
                            .items([item]),
                        );
                    }
                }
            }
        }
        results.rules_checked += 1;
        results
    }
}
