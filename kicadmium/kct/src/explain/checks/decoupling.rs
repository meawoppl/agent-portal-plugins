//! Missing decoupling capacitor checks (port of
//! `explain/checks/decoupling.py`).

use std::collections::BTreeSet;

use super::mistake;
use crate::explain::mistakes::{
    is_bypass_cap, is_ground_net, is_power_net, CheckIncomplete, Mistake, MistakeCategory,
    MistakeCheck,
};
use crate::pyjson::py_repr_str;
use crate::schema::pcb::{Footprint, Pcb};

const MIN_IC_PADS: usize = 4;
const NON_IC_REFERENCE_PREFIXES: [&str; 12] = [
    "R", "C", "L", "D", "Y", "X", "J", "SW", "TP", "FB", "MH", "FID",
];

/// Every IC power net must carry at least one bypass capacitor to ground.
#[derive(Debug, Default, Clone, Copy)]
pub struct MissingDecouplingCapCheck;

fn find_ics(pcb: &Pcb) -> Vec<&Footprint> {
    pcb.footprints()
        .iter()
        .filter(|fp| {
            fp.pads.len() >= MIN_IC_PADS
                && !NON_IC_REFERENCE_PREFIXES
                    .iter()
                    .any(|p| fp.reference.to_uppercase().starts_with(p))
                && fp
                    .pads
                    .iter()
                    .any(|p| !p.net_name.is_empty() && is_power_net(&p.net_name))
        })
        .collect()
}

fn bypass_cap_nets(pcb: &Pcb) -> BTreeSet<String> {
    let mut nets = BTreeSet::new();
    for fp in pcb.footprints() {
        if !is_bypass_cap(&fp.reference, &fp.value) || fp.pads.len() != 2 {
            continue;
        }
        let pad_nets: BTreeSet<&str> = fp
            .pads
            .iter()
            .filter(|p| !p.net_name.is_empty())
            .map(|p| p.net_name.as_str())
            .collect();
        if pad_nets.len() != 2 || !pad_nets.iter().any(|n| is_ground_net(n)) {
            continue;
        }
        nets.extend(
            pad_nets
                .iter()
                .filter(|n| is_power_net(n))
                .map(|n| n.to_string()),
        );
    }
    nets
}

impl MistakeCheck for MissingDecouplingCapCheck {
    fn name(&self) -> &'static str {
        "MissingDecouplingCapCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::Decoupling
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let caps = bypass_cap_nets(pcb);
        let mut seen: Vec<(String, String)> = Vec::new();
        let mut out = Vec::new();
        for ic in find_ics(pcb) {
            let power: BTreeSet<&str> = ic
                .pads
                .iter()
                .filter(|p| !p.net_name.is_empty() && is_power_net(&p.net_name))
                .map(|p| p.net_name.as_str())
                .collect();
            for net in power {
                if caps.contains(net) {
                    continue;
                }
                let key = (ic.reference.clone(), net.to_string());
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                out.push(mistake(
                    MistakeCategory::Decoupling,
                    "error",
                    "Missing decoupling capacitor",
                    vec![ic.reference.clone()],
                    Some(ic.position),
                    format!(
                        "{} has a power pin on net {}, but no bypass/decoupling capacitor \
                         connects that net to a recognized ground return. Without local \
                         decoupling, supply transients from switching current can couple into \
                         the IC and cause glitches, resets, or radiated noise.",
                        ic.reference,
                        py_repr_str(net)
                    ),
                    format!(
                        "Add a decoupling capacitor (typically 100nF, or per the datasheet) from \
                         {net} to the nearest ground return, placed within a few mm of {}'s \
                         power pin.",
                        ic.reference
                    ),
                    "docs/mistakes/bypass-cap-placement.md",
                ));
            }
        }
        Ok(out)
    }
}
