//! Re-derive diff-pair engagement state from a routed PCB + net-class map
//! (port of `kicad_tools.validate.diffpair_engagement`, issue #2652).

use crate::router::diffpair::should_engage_coupled;
use crate::router::diffpair_detection::{detect_diff_pairs, SynthRouting};
use crate::router::rules::NetClassMap;
use crate::schema::pcb::Pcb;
use crate::utils::pyset::{int_pair_set, PySetOrder};
use crate::validate::rules::diffpair_routing_continuity::DEFAULT_COUPLED_CONTINUITY_THRESHOLD;

/// `{net_id: name}` for nets with a non-empty name, in table order.
pub fn named_nets(pcb: &Pcb) -> Vec<(i64, String)> {
    pcb.nets()
        .iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| (n.number, n.name.clone()))
        .collect()
}

pub fn key(a: i64, b: i64) -> (i64, i64) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Pair -> engagement threshold.
pub type ThresholdMap = Vec<((i64, i64), f64)>;

/// `(engaged_pairs, threshold_map)`; the set keeps CPython iteration order.
pub fn derive_engagement_state(
    pcb: &Pcb,
    net_class_map: Option<&NetClassMap>,
) -> (PySetOrder<(i64, i64)>, ThresholdMap) {
    let Some(map) = net_class_map.filter(|m| !m.is_empty()) else {
        return (PySetOrder::new(), vec![]);
    };
    let names = named_nets(pcb);
    if names.is_empty() {
        return (PySetOrder::new(), vec![]);
    }
    let routing = SynthRouting::from_net_class_map(map);
    let mut keys: Vec<(i64, i64)> = Vec::new();
    let mut thresholds: Vec<((i64, i64), f64)> = Vec::new();
    for d in detect_diff_pairs(&names, Some(&routing), None) {
        let pair = &d.pair;
        let (engaged, _) = should_engage_coupled(pair, |n| routing.lookup_net_class(n));
        if !engaged {
            continue;
        }
        let k = key(pair.positive.net_id, pair.negative.net_id);
        keys.push(k);
        let get = |n: &str| map.iter().find(|(k, _)| k == n).map(|(_, v)| v);
        let nc = get(&pair.positive.net_name).or_else(|| get(&pair.negative.net_name));
        let t = nc
            .map(|c| c.effective_coupled_continuity_threshold(DEFAULT_COUPLED_CONTINUITY_THRESHOLD))
            .unwrap_or(DEFAULT_COUPLED_CONTINUITY_THRESHOLD);
        match thresholds.iter_mut().find(|e| e.0 == k) {
            Some(e) => e.1 = t,
            None => thresholds.push((k, t)),
        }
    }
    (int_pair_set(keys), thresholds)
}
