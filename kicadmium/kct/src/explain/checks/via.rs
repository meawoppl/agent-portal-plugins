//! Via placement checks (port of `explain/checks/via.py`).

use super::mistake;
use crate::explain::mistakes::{CheckIncomplete, Mistake, MistakeCategory, MistakeCheck};
use crate::schema::pcb::Pcb;

/// Check for vias in SMD pads without proper filling.
#[derive(Debug, Default, Clone, Copy)]
pub struct ViaInPadCheck;

impl MistakeCheck for ViaInPadCheck {
    fn name(&self) -> &'static str {
        "ViaInPadCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::Via
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let mut mistakes = Vec::new();
        for fp in pcb.footprints() {
            for pad in fp.pads.iter().filter(|p| p.pad_type == "smd") {
                let px = fp.position.0 + pad.position.0;
                let py = fp.position.1 + pad.position.1;
                let (w, h) = pad.size;
                for via in pcb.vias() {
                    let (vx, vy) = via.position;
                    if (vx - px).abs() <= w / 2.0 && (vy - py).abs() <= h / 2.0 {
                        mistakes.push(mistake(
                            MistakeCategory::Via,
                            "warning",
                            "Via in SMD pad",
                            vec![fp.reference.clone(), format!("pad {}", pad.number)],
                            Some(via.position),
                            format!(
                                "Via found in SMD pad {} of {}. During reflow soldering, solder \
                                 can wick down into the via, resulting in insufficient solder on \
                                 the pad and unreliable connections.",
                                pad.number, fp.reference
                            ),
                            "Move the via outside the pad area, or specify via-in-pad with \
                             filled and capped vias in the manufacturing notes. Via-in-pad \
                             increases cost but is acceptable when properly filled."
                                .into(),
                            "docs/mistakes/via-in-pad.md",
                        ));
                    }
                }
            }
        }
        Ok(mistakes)
    }
}
