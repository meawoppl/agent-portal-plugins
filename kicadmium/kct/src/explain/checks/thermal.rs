//! Thermal pad connection checks (port of `explain/checks/thermal.py`).

use super::mistake;
use crate::explain::mistakes::{
    is_ground_net, CheckIncomplete, Mistake, MistakeCategory, MistakeCheck,
};
use crate::schema::pcb::{Footprint, Pad, Pcb};

/// Check that thermal pads are properly connected.
#[derive(Debug, Default, Clone, Copy)]
pub struct ThermalPadConnectionCheck;

impl ThermalPadConnectionCheck {
    /// Large SMD pads (> 3x average SMD pad area) or pads named EP/0/EPAD/THERMAL.
    pub fn find_thermal_pads(fp: &Footprint) -> Vec<&Pad> {
        if fp.pads.is_empty() {
            return Vec::new();
        }
        let sizes: Vec<f64> = fp
            .pads
            .iter()
            .filter(|p| p.pad_type == "smd")
            .map(|p| p.size.0 * p.size.1)
            .collect();
        if sizes.is_empty() {
            return Vec::new();
        }
        let avg = sizes.iter().sum::<f64>() / sizes.len() as f64;
        fp.pads
            .iter()
            .filter(|p| p.pad_type == "smd")
            .filter(|p| {
                let area = p.size.0 * p.size.1;
                area > avg * 3.0
                    || matches!(
                        p.number.to_uppercase().as_str(),
                        "EP" | "0" | "EPAD" | "THERMAL"
                    )
            })
            .collect()
    }

    fn count_nearby_vias(pcb: &Pcb, fp: &Footprint, pad: &Pad) -> usize {
        let px = fp.position.0 + pad.position.0;
        let py = fp.position.1 + pad.position.1;
        let (w, h) = pad.size;
        pcb.vias()
            .iter()
            .filter(|v| {
                let (vx, vy) = v.position;
                (vx - px).abs() <= w / 2.0 + 0.5 && (vy - py).abs() <= h / 2.0 + 0.5
            })
            .count()
    }
}

impl MistakeCheck for ThermalPadConnectionCheck {
    fn name(&self) -> &'static str {
        "ThermalPadConnectionCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::Thermal
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let mut mistakes = Vec::new();
        for fp in pcb.footprints() {
            for pad in Self::find_thermal_pads(fp) {
                let r = &fp.reference;
                if pad.net_name.is_empty() {
                    mistakes.push(mistake(
                        MistakeCategory::Thermal,
                        "error",
                        "Thermal pad not connected",
                        vec![r.clone()],
                        Some(fp.position),
                        format!(
                            "{r} has an exposed thermal pad (pad {}) that is not connected to \
                             any net. Thermal pads must be connected to ground with multiple \
                             vias for heat dissipation. Unconnected thermal pads cause device \
                             overheating and failure.",
                            pad.number
                        ),
                        format!(
                            "Connect the thermal pad on {r} to ground (GND). Add a copper pour \
                             under the pad with 4-9 thermal vias connecting to the ground plane."
                        ),
                        "docs/mistakes/thermal-pad-connection.md",
                    ));
                } else if !is_ground_net(&pad.net_name) {
                    mistakes.push(mistake(
                        MistakeCategory::Thermal,
                        "warning",
                        "Thermal pad not connected to ground",
                        vec![r.clone()],
                        Some(fp.position),
                        format!(
                            "{r} thermal pad (pad {}) is connected to {} instead of ground. \
                             While some devices may specify different connections, most \
                             thermal pads should connect to ground for best heat dissipation.",
                            pad.number, pad.net_name
                        ),
                        format!(
                            "Verify {r} datasheet for thermal pad connection requirements. Most \
                             devices require ground connection with thermal vias."
                        ),
                        "docs/mistakes/thermal-pad-connection.md",
                    ));
                } else {
                    let vias = Self::count_nearby_vias(pcb, fp, pad);
                    if vias < 4 {
                        mistakes.push(mistake(
                            MistakeCategory::Thermal,
                            "info",
                            "Thermal pad may need more vias",
                            vec![r.clone()],
                            Some(fp.position),
                            format!(
                                "{r} thermal pad has only {vias} via(s) nearby. For effective \
                                 heat transfer to the ground plane, 4-9 vias are typically \
                                 recommended under the thermal pad."
                            ),
                            format!(
                                "Add more thermal vias under {r}'s thermal pad. Use 0.3mm drill \
                                 vias in a grid pattern. Consider via-in-pad with filling if \
                                 assembly process allows."
                            ),
                            "docs/mistakes/thermal-pad-connection.md",
                        ));
                    }
                }
            }
        }
        Ok(mistakes)
    }
}
