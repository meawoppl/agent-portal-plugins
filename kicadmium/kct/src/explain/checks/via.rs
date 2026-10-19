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
        if pcb.footprints().iter().flat_map(|fp| &fp.pads).any(|pad| {
            pad.pad_type == "smd"
                && !matches!(
                    pad.shape.as_str(),
                    "rect" | "roundrect" | "circle" | "oval" | "obround"
                )
        }) {
            return Err(CheckIncomplete::new(
                "via-in-pad requires native copper geometry for custom or trapezoid pads",
            ));
        }
        let mut mistakes = Vec::new();
        for fp in pcb.footprints() {
            let mut reported = std::collections::BTreeSet::new();
            for pad in fp.pads.iter().filter(|p| p.pad_type == "smd") {
                let Some(copper) = crate::validate::rules::clearance::pad_polygon(pad, fp) else {
                    continue;
                };
                for (index, via) in pcb.vias().iter().enumerate() {
                    if pad
                        .layers
                        .iter()
                        .any(|layer| layer == "*.Cu" || via.layers.contains(layer))
                        && crate::geometry::shapely::covers_point(&copper, via.position)
                        && reported.insert((pad.number.clone(), index))
                    {
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
