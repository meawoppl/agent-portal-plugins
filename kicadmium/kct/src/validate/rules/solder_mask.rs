//! Solder mask clearance, minimum pad size and PTH annular ring (port of
//! `kicad_tools.validate.rules.solder_mask`).

use super::single_pad_net::absolute_pad_position;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::rules::DRC_TOLERANCE;
use crate::validate::violations::{DRCResults, DRCViolation};

#[derive(Debug, Clone, Default)]
pub struct SolderMaskPadRules;

impl SolderMaskPadRules {
    pub fn check(&self, pcb: &Pcb, rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::new();
        let min_clear = rules.min_solder_mask_clearance_mm;
        let board_clear = pcb.setup().map_or(0.0, |s| s.pad_to_mask_clearance);
        for fp in pcb.footprints() {
            for pad in &fp.pads {
                if !pad.layers.iter().any(|l| l.ends_with(".Mask")) {
                    continue;
                }
                let eff = pad.solder_mask_margin.unwrap_or(board_clear);
                if eff != 0.0 && eff < min_clear {
                    let (x, y) = absolute_pad_position(fp, pad);
                    results.add(
                        DRCViolation::new(
                            "solder_mask_clearance",
                            "warning",
                            format!("Solder mask clearance {eff:.3}mm < minimum {min_clear:.3}mm"),
                        )
                        .at(x, y)
                        .actual(eff)
                        .required(min_clear)
                        .items([format!("{}-{}", fp.reference, pad.number)]),
                    );
                }
            }
        }

        let min_size = rules.min_pad_size_mm;
        for fp in pcb.footprints() {
            for pad in &fp.pads {
                if pad.pad_type == "np_thru_hole" {
                    continue;
                }
                let (w, h) = pad.size;
                let min_dim = if h < w { h } else { w };
                if min_dim > 0.0 && min_dim < min_size {
                    let (x, y) = absolute_pad_position(fp, pad);
                    results.add(
                        DRCViolation::new(
                            "min_pad_size",
                            "error",
                            format!(
                                "Pad size {w:.3}x{h:.3}mm: smallest dimension {min_dim:.3}mm < \
                                 minimum {min_size:.3}mm"
                            ),
                        )
                        .at(x, y)
                        .layer(fp.layer.clone())
                        .actual(min_dim)
                        .required(min_size)
                        .items([format!("{}-{}", fp.reference, pad.number)]),
                    );
                }
            }
        }

        let min_ring = rules
            .min_pth_annular_ring_mm
            .unwrap_or(rules.min_annular_ring_mm);
        for fp in pcb.footprints() {
            for pad in &fp.pads {
                if pad.pad_type != "thru_hole" || pad.drill <= 0.0 {
                    continue;
                }
                let (w, h) = pad.size;
                let min_dim = if h < w { h } else { w };
                let ring = (min_dim - pad.drill) / 2.0;
                if ring + DRC_TOLERANCE < min_ring {
                    let (x, y) = absolute_pad_position(fp, pad);
                    let net = if pad.net_name.is_empty() {
                        format!("net:{}", pad.net_number)
                    } else {
                        pad.net_name.clone()
                    };
                    results.add(
                        DRCViolation::new(
                            "pth_annular_ring",
                            "error",
                            format!(
                                "PTH annular ring {ring:.3}mm < minimum {min_ring:.3}mm (pad \
                                 {min_dim:.3}mm, drill {:.3}mm)",
                                pad.drill
                            ),
                        )
                        .at(x, y)
                        .actual(ring)
                        .required(min_ring)
                        .items([format!("{}-{}", fp.reference, pad.number), net]),
                    );
                }
            }
        }
        results.rules_checked = 3;
        results
    }
}
