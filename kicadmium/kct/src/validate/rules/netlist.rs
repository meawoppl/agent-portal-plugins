//! Undeclared-net pad references (port of
//! `kicad_tools.validate.rules.netlist`).

use std::collections::HashSet;

use crate::core::geometry::rotate_pad_offset;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

#[derive(Debug, Clone, Default)]
pub struct NetlistRule;

impl NetlistRule {
    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        let declared: HashSet<&str> = pcb
            .nets()
            .iter()
            .filter(|n| !n.name.is_empty())
            .map(|n| n.name.as_str())
            .collect();
        for fp in pcb.footprints() {
            let r = if fp.reference.is_empty() {
                &fp.name
            } else {
                &fp.reference
            };
            for pad in &fp.pads {
                if pad.net_name.is_empty() || declared.contains(pad.net_name.as_str()) {
                    continue;
                }
                let (rx, ry) = rotate_pad_offset(pad.position.0, pad.position.1, fp.rotation);
                results.add(
                    DRCViolation::new(
                        "net_undeclared",
                        "warning",
                        format!(
                            "Pad {r}-{} references undeclared net \"{}\" on footprint {r}",
                            pad.number, pad.net_name
                        ),
                    )
                    .at(fp.position.0 + rx, fp.position.1 + ry)
                    .items([format!("{r}-{}", pad.number)]),
                );
            }
        }
        results
    }
}
