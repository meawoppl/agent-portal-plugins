//! Trace width, via drill/diameter, annular ring and hole-to-hole rules
//! (port of `kicad_tools.validate.rules.dimensions`).

use crate::core::geometry::rotate_pad_offset;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::rules::DRC_TOLERANCE;
use crate::validate::violations::{DRCResults, DRCViolation};

#[derive(Debug, Clone, Default)]
pub struct DimensionRules;

/// `net.name if net else f"net:{n}"`.
pub fn net_label(pcb: &Pcb, number: i64) -> String {
    match pcb.get_net(number) {
        Some(n) => n.name.clone(),
        None => format!("net:{number}"),
    }
}

impl DimensionRules {
    pub fn check(&self, pcb: &Pcb, rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        let min_width = rules.min_trace_width_mm;
        for s in pcb.segments() {
            if s.width + DRC_TOLERANCE < min_width {
                results.add(
                    DRCViolation::new(
                        "dimension_trace_width",
                        "error",
                        format!("Trace width {:.3}mm < minimum {min_width:.3}mm", s.width),
                    )
                    .at(s.start.0, s.start.1)
                    .layer(s.layer.clone())
                    .actual(s.width)
                    .required(min_width)
                    .items([net_label(pcb, s.net_number)]),
                );
            }
        }

        let (min_drill, min_dia, min_ring) = (
            rules.min_via_drill_mm,
            rules.min_via_diameter_mm,
            rules.min_annular_ring_mm,
        );
        for via in pcb.vias() {
            let name = net_label(pcb, via.net_number);
            if via.via_type.as_deref() == Some("micro") {
                continue;
            }
            let layer = via.layers.first().cloned();
            let mk = |rule: &str, msg: String, actual: f64, req: f64| {
                DRCViolation::new(rule, "error", msg)
                    .at(via.position.0, via.position.1)
                    .layer_opt(layer.clone())
                    .actual(actual)
                    .required(req)
                    .items([name.clone()])
            };
            if via.drill + DRC_TOLERANCE < min_drill {
                results.add(mk(
                    "dimension_via_drill",
                    format!("Via drill {:.3}mm < minimum {min_drill:.3}mm", via.drill),
                    via.drill,
                    min_drill,
                ));
            }
            if via.size + DRC_TOLERANCE < min_dia {
                results.add(mk(
                    "dimension_via_diameter",
                    format!("Via diameter {:.3}mm < minimum {min_dia:.3}mm", via.size),
                    via.size,
                    min_dia,
                ));
            }
            let ring = (via.size - via.drill) / 2.0;
            if ring + DRC_TOLERANCE < min_ring {
                results.add(mk(
                    "dimension_annular_ring",
                    format!("Annular ring {ring:.3}mm < minimum {min_ring:.3}mm"),
                    ring,
                    min_ring,
                ));
            }
        }

        self.check_drill_clearance(pcb, rules, &mut results);
        results.rules_checked = 5;
        results
    }

    fn check_drill_clearance(&self, pcb: &Pcb, rules: &DesignRules, results: &mut DRCResults) {
        let min_clearance = rules.min_hole_to_hole_mm;
        // (pos, drill, item, fp_ref, net)
        let mut drills: Vec<((f64, f64), f64, String, String, String)> = Vec::new();
        for via in pcb.vias() {
            let name = net_label(pcb, via.net_number);
            drills.push((via.position, via.drill, name.clone(), String::new(), name));
        }
        for fp in pcb.footprints() {
            for pad in &fp.pads {
                if pad.pad_type == "thru_hole" && pad.drill > 0.0 {
                    let (rx, ry) = rotate_pad_offset(pad.position.0, pad.position.1, fp.rotation);
                    let pos = (fp.position.0 + rx, fp.position.1 + ry);
                    let net = if pad.net_name.is_empty() {
                        format!("net:{}", pad.net_number)
                    } else {
                        pad.net_name.clone()
                    };
                    drills.push((
                        pos,
                        pad.drill,
                        format!("{}-{}:{net}", fp.reference, pad.number),
                        fp.reference.clone(),
                        net,
                    ));
                }
            }
        }
        // Sort-and-sweep on x preserving the exhaustive (i, j) emission order.
        let max_drill = drills.iter().map(|d| d.1).fold(0.0f64, f64::max);
        let reach = min_clearance + max_drill + 1.0;
        let mut hits: Vec<(usize, usize)> = Vec::new();
        let mut order: Vec<usize> = (0..drills.len()).collect();
        order.sort_by(|&a, &b| drills[a].0 .0.total_cmp(&drills[b].0 .0));
        for (k, &i) in order.iter().enumerate() {
            for &j in &order[k + 1..] {
                if drills[j].0 .0 - drills[i].0 .0 > reach || !drills[j].0 .0.is_finite() {
                    break;
                }
                hits.push(if i < j { (i, j) } else { (j, i) });
            }
        }
        let finite = drills
            .iter()
            .all(|d| d.0 .0.is_finite() && d.0 .1.is_finite() && d.1.is_finite());
        if !finite {
            hits = (0..drills.len())
                .flat_map(|i| (i + 1..drills.len()).map(move |j| (i, j)))
                .collect();
        }
        hits.sort_unstable();
        for (i, j) in hits {
            let (p1, d1, item1, ref1, net1) = &drills[i];
            let (p2, d2, item2, ref2, net2) = &drills[j];
            let dx = p2.0 - p1.0;
            let dy = p2.1 - p1.1;
            let center = (dx * dx + dy * dy).sqrt();
            let edge = center - d1 / 2.0 - d2 / 2.0;
            if edge + DRC_TOLERANCE < min_clearance {
                let same = !ref1.is_empty() && ref1 == ref2;
                results.add(
                    DRCViolation::new(
                        "hole_to_hole_clearance",
                        if same { "warning" } else { "error" },
                        format!(
                            "{}Hole-to-hole clearance {edge:.3}mm < minimum {min_clearance:.3}mm",
                            if same { "(same-footprint) " } else { "" }
                        ),
                    )
                    .at((p1.0 + p2.0) / 2.0, (p1.1 + p2.1) / 2.0)
                    .actual(edge)
                    .required(min_clearance)
                    .items([item1.clone(), item2.clone()])
                    .nets([net1.clone(), net2.clone()]),
                );
            }
        }
    }
}
