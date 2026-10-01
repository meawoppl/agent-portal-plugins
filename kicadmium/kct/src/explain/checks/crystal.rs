//! Crystal oscillator placement and routing checks (port of
//! `explain/checks/crystal.py`).

use super::mistake;
use crate::explain::mistakes::{
    distance, is_crystal, trace_length, CheckIncomplete, Mistake, MistakeCategory, MistakeCheck,
};
use crate::schema::pcb::{Footprint, Pcb, Segment};

/// Maximum recommended crystal trace length (mm).
pub const MAX_CRYSTAL_TRACE_LENGTH_MM: f64 = 10.0;
/// Minimum distance from crystal to noisy signals (mm).
pub const MIN_NOISE_DISTANCE_MM: f64 = 5.0;

fn find_crystals(pcb: &Pcb) -> impl Iterator<Item = &Footprint> {
    pcb.footprints()
        .iter()
        .filter(|fp| is_crystal(&fp.reference, &fp.name))
}

fn is_power_or_ground(net_name: &str) -> bool {
    let up = net_name.to_uppercase();
    ["VCC", "VDD", "GND", "VSS", "3V3", "5V", "GROUND"]
        .iter()
        .any(|p| up.contains(p))
}

/// Check that crystal traces are short.
#[derive(Debug, Default, Clone, Copy)]
pub struct CrystalTraceLengthCheck;

impl MistakeCheck for CrystalTraceLengthCheck {
    fn name(&self) -> &'static str {
        "CrystalTraceLengthCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::Crystal
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let mut mistakes = Vec::new();
        for crystal in find_crystals(pcb) {
            let nets = crystal
                .pads
                .iter()
                .filter(|p| !p.net_name.is_empty() && !is_power_or_ground(&p.net_name))
                .map(|p| p.net_name.as_str());
            for net_name in nets {
                let segs: Vec<&Segment> = pcb
                    .segments()
                    .iter()
                    .filter(|s| pcb.get_net(s.net_number).is_some_and(|n| n.name == net_name))
                    .collect();
                if segs.is_empty() {
                    continue;
                }
                let length = trace_length(segs);
                if length > MAX_CRYSTAL_TRACE_LENGTH_MM {
                    mistakes.push(mistake(
                        MistakeCategory::Crystal,
                        "warning",
                        "Crystal trace too long",
                        vec![crystal.reference.clone()],
                        Some(crystal.position),
                        format!(
                            "Crystal trace {net_name} is {length:.1}mm long. Crystal oscillator \
                             traces should be kept under 10.0mm to minimize parasitic capacitance \
                             and EMI. Long traces can affect frequency accuracy and increase noise \
                             emissions."
                        ),
                        format!(
                            "Move the microcontroller closer to {}, or reroute {net_name} to \
                             shorten the trace. Place load capacitors close to the crystal pins.",
                            crystal.reference
                        ),
                        "docs/mistakes/crystal-layout.md",
                    ));
                }
            }
        }
        Ok(mistakes)
    }
}

/// Check that crystals are away from noisy signals.
#[derive(Debug, Default, Clone, Copy)]
pub struct CrystalNoiseProximityCheck;

impl CrystalNoiseProximityCheck {
    /// `(footprint, noise_type)` pairs.
    fn find_noise_sources(pcb: &Pcb) -> Vec<(&Footprint, &'static str)> {
        let mut out = Vec::new();
        for fp in pcb.footprints() {
            let r = fp.reference.to_uppercase();
            let n = fp.name.to_lowercase();
            if n.contains("regulator") || n.contains("buck") || n.contains("boost") {
                out.push((fp, "switching regulator"));
            } else if n.contains("usb") {
                out.push((fp, "USB controller"));
            } else if n.contains("motor") || r.starts_with("DRV") {
                out.push((fp, "motor driver"));
            } else if n.contains("ethernet") || n.contains("can") {
                out.push((fp, "high-speed interface"));
            }
        }
        out
    }
}

impl MistakeCheck for CrystalNoiseProximityCheck {
    fn name(&self) -> &'static str {
        "CrystalNoiseProximityCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::Crystal
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let mut mistakes = Vec::new();
        let noise = Self::find_noise_sources(pcb);
        for crystal in find_crystals(pcb) {
            for (src, kind) in &noise {
                let dist = distance(crystal.position, src.position);
                if dist < MIN_NOISE_DISTANCE_MM {
                    mistakes.push(mistake(
                        MistakeCategory::Crystal,
                        "warning",
                        "Crystal near noise source",
                        vec![crystal.reference.clone(), src.reference.clone()],
                        Some(crystal.position),
                        format!(
                            "{} is only {dist:.1}mm from {} ({kind}). Crystal oscillators are \
                             sensitive to electromagnetic interference. Proximity to noise \
                             sources can cause frequency instability or startup failures.",
                            crystal.reference, src.reference
                        ),
                        format!(
                            "Move {} at least 5.0mm away from {}. Consider adding a ground guard \
                             ring around the crystal if space is limited.",
                            crystal.reference, src.reference
                        ),
                        "docs/mistakes/crystal-layout.md",
                    ));
                }
            }
        }
        Ok(mistakes)
    }
}
