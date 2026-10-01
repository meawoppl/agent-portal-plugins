//! Tombstoning risk checks (port of `explain/checks/tombstoning.py`).

use super::mistake;
use crate::explain::mistakes::{CheckIncomplete, Mistake, MistakeCategory, MistakeCheck};
use crate::schema::pcb::{Footprint, Pad, Pcb};
use crate::utils::pymath::py_sum;

/// Passives whose total SMD pad area is below this (~0603 and smaller).
pub const MAX_PASSIVE_SIZE_FOR_RISK: f64 = 3.0;

/// Check for tombstoning risk on small passives.
#[derive(Debug, Default, Clone, Copy)]
pub struct TombstoningRiskCheck;

fn is_small_passive(fp: &Footprint) -> bool {
    let r = fp.reference.to_uppercase();
    if !(r.starts_with('R') || r.starts_with('C')) || fp.attr != "smd" || fp.pads.is_empty() {
        return false;
    }
    py_sum(
        fp.pads
            .iter()
            .filter(|p| p.pad_type == "smd")
            .map(|p| p.size.0 * p.size.1),
    ) < MAX_PASSIVE_SIZE_FOR_RISK
}

fn zone_count(pcb: &Pcb, pad: &Pad) -> usize {
    pcb.zones()
        .iter()
        .filter(|z| z.net_number == pad.net_number)
        .count()
}

fn zone_net(pcb: &Pcb, pad: &Pad) -> String {
    pcb.zones()
        .iter()
        .find(|z| z.net_number == pad.net_number)
        .map(|z| {
            if z.net_name.is_empty() {
                format!("Net {}", z.net_number)
            } else {
                z.net_name.clone()
            }
        })
        .unwrap_or_else(|| "unknown".into())
}

impl MistakeCheck for TombstoningRiskCheck {
    fn name(&self) -> &'static str {
        "TombstoningRiskCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::Manufacturability
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let mut out = Vec::new();
        for fp in pcb.footprints() {
            if !is_small_passive(fp) || fp.pads.len() != 2 {
                continue;
            }
            let (p1, p2) = (&fp.pads[0], &fp.pads[1]);
            let s1 = p1.size.0 * p1.size.1;
            let s2 = p2.size.0 * p2.size.1;
            let ratio = if s1.min(s2) > 0.0 {
                s1.max(s2) / s1.min(s2)
            } else {
                1.0
            };
            if ratio > 1.2 {
                out.push(mistake(
                    MistakeCategory::Manufacturability,
                    "info",
                    "Asymmetric pads may cause tombstoning",
                    vec![fp.reference.clone()],
                    Some(fp.position),
                    format!(
                        "{} has asymmetric pad sizes ({:.2}x{:.2}mm vs {:.2}x{:.2}mm). Unequal \
                         pads can cause tombstoning during reflow due to unequal solder surface \
                         tension.",
                        fp.reference, p1.size.0, p1.size.1, p2.size.0, p2.size.1
                    ),
                    format!(
                        "Ensure both pads of {} are the same size. Check the footprint \
                         definition for errors.",
                        fp.reference
                    ),
                    "docs/mistakes/tombstoning.md",
                ));
            }
            let (z1, z2) = (zone_count(pcb, p1), zone_count(pcb, p2));
            let imbalance = if z1 > 0 && z2 == 0 {
                Some(p1)
            } else if z2 > 0 && z1 == 0 {
                Some(p2)
            } else {
                None
            };
            if let Some(large) = imbalance {
                out.push(mistake(
                    MistakeCategory::Manufacturability,
                    "warning",
                    "Thermal imbalance may cause tombstoning",
                    vec![fp.reference.clone()],
                    Some(fp.position),
                    format!(
                        "{} pad {} is connected to copper zone ({}), while the other pad is not. \
                         The thermal mass difference can cause tombstoning: the pad on the zone \
                         heats slower, so solder melts unevenly.",
                        fp.reference,
                        large.number,
                        zone_net(pcb, large)
                    ),
                    "Add thermal relief to the zone connection, or add copper to balance thermal \
                     mass on both pads. Consider adding thermal spokes or reducing zone \
                     connection width."
                        .into(),
                    "docs/mistakes/tombstoning.md",
                ));
            }
        }
        Ok(out)
    }
}
