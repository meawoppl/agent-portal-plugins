//! Bypass capacitor placement checks (port of `explain/checks/bypass.py`).

use super::mistake;
use crate::explain::mistakes::{
    distance, is_bypass_cap, is_power_net, CheckIncomplete, Mistake, MistakeCategory, MistakeCheck,
};
use crate::schema::pcb::{Footprint, Pad, Pcb};

/// Maximum recommended distance from bypass cap to power pin (mm).
pub const MAX_BYPASS_DISTANCE_MM: f64 = 3.0;
/// Warning threshold (mm).
pub const BYPASS_WARNING_DISTANCE_MM: f64 = 5.0;

/// Check that bypass capacitors are close to IC power pins.
#[derive(Debug, Default, Clone, Copy)]
pub struct BypassCapDistanceCheck;

impl MistakeCheck for BypassCapDistanceCheck {
    fn name(&self) -> &'static str {
        "BypassCapDistanceCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::BypassCap
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let mut mistakes = Vec::new();
        let ics: Vec<&Footprint> = pcb
            .footprints()
            .iter()
            .filter(|fp| fp.pads.iter().any(|p| is_power_net(&p.net_name)))
            .collect();
        let caps = pcb
            .footprints()
            .iter()
            .filter(|fp| is_bypass_cap(&fp.reference, &fp.value));

        for cap in caps {
            let Some(cap_net) = cap
                .pads
                .iter()
                .find(|p| is_power_net(&p.net_name))
                .map(|p| p.net_name.as_str())
            else {
                continue;
            };
            if cap_net.is_empty() {
                continue;
            }
            for ic in &ics {
                let pads: Vec<&Pad> = ic.pads.iter().filter(|p| p.net_name == cap_net).collect();
                for pad in pads {
                    // TODO upstream: rotation is not handled.
                    let pad_pos = (
                        ic.position.0 + pad.position.0,
                        ic.position.1 + pad.position.1,
                    );
                    let cap_pos = cap.position;
                    let dist = distance(cap_pos, pad_pos);
                    if dist > BYPASS_WARNING_DISTANCE_MM {
                        mistakes.push(mistake(
                            MistakeCategory::BypassCap,
                            "warning",
                            "Bypass capacitor too far from power pin",
                            vec![cap.reference.clone(), ic.reference.clone()],
                            Some(cap_pos),
                            format!(
                                "{} is {dist:.1}mm from {} pin {} ({cap_net}). Bypass capacitors \
                                 should be within 3.0mm of the power pin they're decoupling. At \
                                 {dist:.1}mm, the inductance of the trace reduces filtering \
                                 effectiveness.",
                                cap.reference, ic.reference, pad.number
                            ),
                            format!(
                                "Move {} to within 3.0mm of {} pin {}, with a short, wide trace \
                                 to both {cap_net} and GND.",
                                cap.reference, ic.reference, pad.number
                            ),
                            "docs/mistakes/bypass-cap-placement.md",
                        ));
                    } else if dist > MAX_BYPASS_DISTANCE_MM {
                        mistakes.push(mistake(
                            MistakeCategory::BypassCap,
                            "info",
                            "Bypass capacitor placement could be improved",
                            vec![cap.reference.clone(), ic.reference.clone()],
                            Some(cap_pos),
                            format!(
                                "{} is {dist:.1}mm from {} pin {}. Ideally, bypass capacitors \
                                 should be within 3.0mm for optimal decoupling.",
                                cap.reference, ic.reference, pad.number
                            ),
                            format!(
                                "Consider moving {} closer to {} pin {} if space permits.",
                                cap.reference, ic.reference, pad.number
                            ),
                            "docs/mistakes/bypass-cap-placement.md",
                        ));
                    }
                }
            }
        }
        Ok(mistakes)
    }
}
