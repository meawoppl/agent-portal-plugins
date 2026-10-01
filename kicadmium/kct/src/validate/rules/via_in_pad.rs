//! Vias drilled inside SMD pads without an eligible fabrication process
//! (port of `kicad_tools.validate.rules.via_in_pad` and
//! `via_pad_geometry`, issues #2635 / #5009).

use super::clearance::{pad_polygon, uuid8};
use crate::core::geometry::rotate_pad_offset;
use crate::geometry::shapely::{self as sh, Geom};
use crate::manufacturers::fabrication_process::{get_fabrication_process, FabricationProcess};
use crate::manufacturers::DesignRules;
use crate::pyjson::{py_repr_str, py_round};
use crate::router::via_in_pad_eligibility::{
    component_holes_from_document, resolve_component_hole_context,
};
use crate::schema::pcb::{Footprint, Pad, Pcb, Via};
use crate::utils::pymath::hypot;
use crate::validate::rules::DRC_TOLERANCE;
use crate::validate::violations::{DRCResults, DRCViolation};

/// `pad_absolute_bbox`.
pub fn pad_absolute_bbox(pad: &Pad, fp: &Footprint) -> (f64, f64, f64, f64) {
    let rot = pad.rotation.rem_euclid(360.0);
    let a = rot.to_radians();
    let (rx, ry) = rotate_pad_offset(pad.position.0, pad.position.1, fp.rotation);
    let (x, y) = (fp.position.0 + rx, fp.position.1 + ry);
    let (w, h) = pad.size;
    let (bw, bh) = if (rot - 90.0).abs() < 0.001 || (rot - 270.0).abs() < 0.001 {
        (h, w)
    } else if rot.abs() < 0.001 || (rot - 180.0).abs() < 0.001 {
        (w, h)
    } else {
        let (c, s) = (a.cos().abs(), a.sin().abs());
        (w * c + h * s, w * s + h * c)
    };
    (x - bw / 2.0, y - bh / 2.0, x + bw / 2.0, y + bh / 2.0)
}

fn max3(a: f64, b: f64, c: f64) -> f64 {
    let m = if b > a { b } else { a };
    if c > m {
        c
    } else {
        m
    }
}

/// `via_inside_pad`.
pub fn via_inside_pad(via: &Via, bbox: (f64, f64, f64, f64), pad: Option<(&Pad, &Footprint)>) -> bool {
    let (cx, cy) = via.position;
    let r = via.drill / 2.0;
    if r <= DRC_TOLERANCE {
        return false;
    }
    let (x0, y0, x1, y1) = bbox;
    let d = hypot(max3(x0 - cx, 0.0, cx - x1), max3(y0 - cy, 0.0, cy - y1));
    if d >= r - DRC_TOLERANCE {
        return false;
    }
    if let Some((pad, fp)) = pad {
        return match pad_polygon(pad, fp) {
            Some(copper) => sh::distance(&copper, &Geom::Point((cx, cy))) < r - DRC_TOLERANCE,
            None => false,
        };
    }
    true
}

#[derive(Debug, Clone, Default)]
pub struct ViaInPadRule;

fn via_ref(via: &Via) -> String {
    if via.uuid.is_empty() {
        "Via".into()
    } else {
        format!("Via-{}", uuid8(&via.uuid))
    }
}

impl ViaInPadRule {
    pub fn check(&self, pcb: &Pcb, rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        let supported = rules.via_in_pad_supported;
        let process_id = rules.via_in_pad_process_id.as_deref();
        let process = if supported {
            get_fabrication_process(process_id)
        } else {
            None
        };
        let mut by_net: Vec<(i64, Vec<(&Footprint, &Pad, (f64, f64, f64, f64))>)> = Vec::new();
        let holes = component_holes_from_document(pcb);
        for fp in pcb.footprints() {
            for pad in &fp.pads {
                if pad.pad_type != "smd" || pad.net_number == 0 {
                    continue;
                }
                let b = pad_absolute_bbox(pad, fp);
                match by_net.iter_mut().find(|(n, _)| *n == pad.net_number) {
                    Some(e) => e.1.push((fp, pad, b)),
                    None => by_net.push((pad.net_number, vec![(fp, pad, b)])),
                }
            }
        }
        let layer_count = pcb.copper_layers().len() as i64;
        for via in pcb.vias() {
            if via.net_number == 0 {
                continue;
            }
            let Some((_, cands)) = by_net.iter().find(|(n, _)| *n == via.net_number) else {
                continue;
            };
            for (fp, pad, b) in cands {
                if !via_inside_pad(via, *b, Some((pad, fp))) {
                    continue;
                }
                if !supported {
                    results.add(self.violation(via, fp, pad));
                    continue;
                }
                let Some(process) = process else {
                    results.add(self.missing(via, fp, pad, process_id));
                    continue;
                };
                let ctx = resolve_component_hole_context(
                    via.position.0,
                    via.position.1,
                    via.drill,
                    holes.as_deref(),
                );
                let reasons = if !ctx.known {
                    vec!["component-hole census has unknown or invalid drilled geometry".to_string()]
                } else {
                    process.eligibility_reasons(
                        Some(layer_count),
                        via.drill,
                        Some((via.size - via.drill) / 2.0),
                        ctx.nearest_distance_mm,
                    )
                };
                if !reasons.is_empty() {
                    results.add(self.ineligible(via, fp, pad, process, &reasons));
                }
            }
        }
        results
    }

    fn base(&self, rule: &str, via: &Via, fp: &Footprint, pad: &Pad, msg: String) -> DRCViolation {
        let label = format!("{}-{}", fp.reference, pad.number);
        let net = if via.net_name.is_empty() {
            pad.net_name.clone()
        } else {
            via.net_name.clone()
        };
        DRCViolation::new(rule, "error", msg)
            .at(py_round(via.position.0, 3), py_round(via.position.1, 3))
            .actual(py_round(via.drill, 4))
            .items([via_ref(via), label])
            .nets([net])
    }

    fn net_of(via: &Via, pad: &Pad) -> String {
        if via.net_name.is_empty() {
            pad.net_name.clone()
        } else {
            via.net_name.clone()
        }
    }

    fn violation(&self, via: &Via, fp: &Footprint, pad: &Pad) -> DRCViolation {
        let msg = format!(
            "Via at ({:.3}, {:.3}) drilled inside pad {}-{} (net '{}'); current manufacturer \
             profile does not support via-in-pad. Switch to jlcpcb-tier1 or pcbway, or move the \
             via off the pad.",
            via.position.0,
            via.position.1,
            fp.reference,
            pad.number,
            Self::net_of(via, pad)
        );
        self.base("via_in_pad", via, fp, pad, msg)
    }

    fn missing(&self, via: &Via, fp: &Footprint, pad: &Pad, process_id: Option<&str>) -> DRCViolation {
        let detail = match process_id.filter(|s| !s.is_empty()) {
            Some(p) => format!("unrecognized via_in_pad_process_id {}", py_repr_str(p)),
            None => "via_in_pad_process_id is unset".to_string(),
        };
        let msg = format!(
            "Via at ({:.3}, {:.3}) drilled inside pad {}-{} (net '{}') requires a \
             filled-and-capped via-in-pad process, but {detail}. A bare \
             via_in_pad_supported=True capability flag is not sufficient -- the manufacturer's \
             general tier supports via-in-pad somewhere in its catalog, but no eligible \
             FabricationProcess is declared for this layer/copper configuration (see \
             kicad_tools.manufacturers.fabrication_process).",
            via.position.0,
            via.position.1,
            fp.reference,
            pad.number,
            Self::net_of(via, pad)
        );
        self.base("via_in_pad_process_missing", via, fp, pad, msg)
    }

    fn ineligible(
        &self,
        via: &Via,
        fp: &Footprint,
        pad: &Pad,
        process: &FabricationProcess,
        reasons: &[String],
    ) -> DRCViolation {
        let msg = format!(
            "Via at ({:.3}, {:.3}) drilled inside pad {}-{} (net '{}') does not meet the \
             declared process {} ({}) eligibility requirements: {}. Source: {}",
            via.position.0,
            via.position.1,
            fp.reference,
            pad.number,
            Self::net_of(via, pad),
            py_repr_str(process.process_id),
            process.name,
            reasons.join("; "),
            process.source
        );
        self.base("via_in_pad_process_ineligible", via, fp, pad, msg)
            .required(process.min_via_drill_mm)
    }
}
