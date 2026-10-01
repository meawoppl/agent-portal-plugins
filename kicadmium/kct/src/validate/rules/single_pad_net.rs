//! Signal nets attached to a single pad (port of
//! `kicad_tools.validate.rules.single_pad_net`, issue #2613).

use std::sync::LazyLock;

use regex::Regex;

use crate::core::geometry::rotate_pad_offset;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::{Footprint, Pad, Pcb};
use crate::validate::violations::{DRCResults, DRCViolation};

static KICAD_NC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^unconnected-\(.+-Pad\d+\)$").unwrap());
static CONNECTOR_NET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Net-\(([JP])\d+-(?:Pad)?\d+\)$").unwrap());

/// `_classify_net`.
pub fn classify_net(net_name: &str, footprint_ref: &str) -> &'static str {
    if KICAD_NC.is_match(net_name) {
        return "genuine_nc";
    }
    if let Some(c) = CONNECTOR_NET.captures(net_name) {
        let prefix: String = footprint_ref
            .chars()
            .filter(|c| c.is_alphabetic())
            .collect();
        if prefix == c[1] {
            return "connector_nc";
        }
    }
    "defect"
}

/// `netlist._absolute_pad_position`.
pub fn absolute_pad_position(fp: &Footprint, pad: &Pad) -> (f64, f64) {
    let (rx, ry) = rotate_pad_offset(pad.position.0, pad.position.1, fp.rotation);
    (fp.position.0 + rx, fp.position.1 + ry)
}

type NetPads<'a> = Vec<((i64, &'a str), Vec<(&'a Footprint, &'a Pad)>)>;

#[derive(Debug, Clone, Default)]
pub struct SinglePadNetRule;

impl SinglePadNetRule {
    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        let mut net_pads: NetPads = Vec::new();
        let mut index: std::collections::HashMap<(i64, &str), usize> = Default::default();
        for fp in pcb.footprints() {
            for pad in &fp.pads {
                if pad.net_name.is_empty() {
                    continue;
                }
                let key = (pad.net_number, pad.net_name.as_str());
                let i = *index.entry(key).or_insert_with(|| {
                    net_pads.push((key, Vec::new()));
                    net_pads.len() - 1
                });
                net_pads[i].1.push((fp, pad));
            }
        }
        for ((_, net_name), pads) in &net_pads {
            if pads.len() != 1 {
                continue;
            }
            if crate::router::net_class::is_pour_net(net_name) {
                continue;
            }
            let (fp, pad) = pads[0];
            let r = if fp.reference.is_empty() {
                &fp.name
            } else {
                &fp.reference
            };
            let (severity, message) = match classify_net(net_name, r) {
                "genuine_nc" => (
                    "info",
                    format!(
                        "Net '{net_name}' has only 1 pad on {r}-{} -- KiCad-emitted explicit \
                         no-connect (symbol pin marked NC); no action required",
                        pad.number
                    ),
                ),
                "connector_nc" => (
                    "info",
                    format!(
                        "Net '{net_name}' has only 1 pad on {r}-{} -- connector pin with no \
                         asserted connection (typically intentional GPIO no-connect); add \
                         explicit no_connect flag in schematic to silence",
                        pad.number
                    ),
                ),
                _ => (
                    "error",
                    format!(
                        "Net '{net_name}' has only 1 pad on {r}-{} -- likely missing footprint \
                         or schematic/PCB drift",
                        pad.number
                    ),
                ),
            };
            let (x, y) = absolute_pad_position(fp, pad);
            results.add(
                DRCViolation::new("single_pad_net", severity, message)
                    .at(x, y)
                    .items([format!("{r}-{}", pad.number)])
                    .nets([net_name.to_string()]),
            );
        }
        results
    }
}
