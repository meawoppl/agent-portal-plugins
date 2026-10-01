//! Re-derive per-group length skew from a routed PCB (port of
//! `kicad_tools.validate.match_group_skew`, issue #2710).

use crate::router::diffpair_detection::SynthRouting;
use crate::router::match_group_detection::{detect_match_groups, MatchGroup};
use crate::router::rules::NetClassMap;
use crate::schema::pcb::Pcb;
use crate::validate::diffpair_engagement::named_nets;
use crate::validate::diffpair_skew::measure_net_from_pcb;
use crate::validate::rules::match_group_length_skew::DEFAULT_MATCH_GROUP_TOLERANCE_MM;

fn net_has_geometry(pcb: &Pcb, net_id: i64) -> bool {
    pcb.segments_in_net(net_id).next().is_some() || pcb.vias_in_net(net_id).next().is_some()
}

/// `(group_skew_data sorted by name, detected groups, threshold_map)`.
pub type GroupSkew = (Vec<(String, f64)>, Vec<MatchGroup>, Vec<(String, f64)>);

pub fn derive_group_skew_data(
    pcb: &Pcb,
    net_class_map: Option<&NetClassMap>,
    board_thickness_mm: Option<f64>,
    num_copper_layers: i64,
    blind_buried_supported: bool,
) -> GroupSkew {
    let Some(map) = net_class_map.filter(|m| !m.is_empty()) else {
        return (vec![], vec![], vec![]);
    };
    let names = named_nets(pcb);
    if names.is_empty() {
        return (vec![], vec![], vec![]);
    }
    let routing = SynthRouting::from_net_class_map(map);
    let detected = detect_match_groups(&names, Some(&routing), None, false);
    let measure = |id: i64| {
        measure_net_from_pcb(
            pcb,
            id,
            board_thickness_mm,
            num_copper_layers,
            blind_buried_supported,
        )
    };
    let mut skew: Vec<(String, f64)> = Vec::new();
    let mut thresholds: Vec<(String, f64)> = Vec::new();
    for g in &detected {
        if g.net_ids.is_empty() && g.pair_ids.is_empty() {
            continue;
        }
        let mut unrouted = false;
        let mut measured: Vec<f64> = Vec::new();
        for &id in &g.net_ids {
            if !net_has_geometry(pcb, id) {
                unrouted = true;
                break;
            }
            measured.push(measure(id));
        }
        if !unrouted {
            for &(p, n) in &g.pair_ids {
                if !net_has_geometry(pcb, p) || !net_has_geometry(pcb, n) {
                    unrouted = true;
                    break;
                }
                measured.push((measure(p) + measure(n)) / 2.0);
            }
        }
        if unrouted || measured.len() < 2 {
            continue;
        }
        let max = measured.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let min = measured.iter().copied().fold(f64::INFINITY, f64::min);
        let v = max - min;
        match skew.iter_mut().find(|e| e.0 == g.name) {
            Some(e) => e.1 = v,
            None => skew.push((g.name.clone(), v)),
        }
        let first = if g.net_ids.is_empty() {
            g.pair_ids[0].0
        } else {
            g.net_ids[0]
        };
        let first_name = names
            .iter()
            .rev()
            .find(|(i, _)| *i == first)
            .map(|(_, n)| n.as_str());
        let nc = first_name.and_then(|n| map.iter().find(|(k, _)| k == n).map(|(_, v)| v));
        let t = nc
            .map(|c| c.effective_length_match_tolerance(DEFAULT_MATCH_GROUP_TOLERANCE_MM))
            .unwrap_or(DEFAULT_MATCH_GROUP_TOLERANCE_MM);
        match thresholds.iter_mut().find(|e| e.0 == g.name) {
            Some(e) => e.1 = t,
            None => thresholds.push((g.name.clone(), t)),
        }
    }
    skew.sort_by(|a, b| a.0.cmp(&b.0));
    (skew, detected, thresholds)
}
