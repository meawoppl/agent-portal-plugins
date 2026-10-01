//! Re-derive per-pair length skew from a routed PCB (port of
//! `kicad_tools.validate.diffpair_skew`, issue #2675).

use crate::router::diffpair_detection::{detect_diff_pairs, SynthRouting};
use crate::router::rules::NetClassMap;
use crate::schema::pcb::Pcb;
use crate::validate::diffpair_engagement::{key, named_nets};
use crate::validate::rules::diffpair_length_skew::DEFAULT_SKEW_TOLERANCE_MM;

/// `CopperLayer` enum value (`F.Cu`=0 .. `B.Cu`=5) for known names.
fn copper_layer_value(name: &str) -> Option<usize> {
    match name {
        "F.Cu" => Some(0),
        "In1.Cu" => Some(1),
        "In2.Cu" => Some(2),
        "In3.Cu" => Some(3),
        "In4.Cu" => Some(4),
        "B.Cu" => Some(5),
        _ => None,
    }
}

fn stack_position(value: usize, num_copper_layers: i64) -> i64 {
    match value {
        0 => 0,
        5 => num_copper_layers - 1,
        v => v as i64,
    }
}

/// `DiffPairLengthTracker.measure_net_from_pcb`.
pub fn measure_net_from_pcb(
    pcb: &Pcb,
    net_id: i64,
    board_thickness_mm: Option<f64>,
    num_copper_layers: i64,
    blind_buried_supported: bool,
) -> f64 {
    let mut total = 0.0;
    for s in pcb.segments_in_net(net_id) {
        let dx = s.end.0 - s.start.0;
        let dy = s.end.1 - s.start.1;
        total += (dx * dx + dy * dy).sqrt();
    }
    let Some(t) = board_thickness_mm else {
        return total;
    };
    if num_copper_layers <= 1 {
        return total;
    }
    for v in pcb.vias_in_net(net_id) {
        if v.layers.len() < 2 {
            continue;
        }
        let micro = v.via_type.as_deref() == Some("micro");
        if !blind_buried_supported && !micro {
            total += t;
            continue;
        }
        let (Some(a), Some(b)) = (
            copper_layer_value(&v.layers[0]),
            copper_layer_value(&v.layers[v.layers.len() - 1]),
        ) else {
            continue;
        };
        let delta =
            (stack_position(a, num_copper_layers) - stack_position(b, num_copper_layers)).abs();
        total += t * delta as f64 / (num_copper_layers - 1) as f64;
    }
    total
}

fn net_has_geometry(pcb: &Pcb, net_id: i64) -> bool {
    pcb.segments_in_net(net_id).next().is_some() || pcb.vias_in_net(net_id).next().is_some()
}

/// `{(p_name, n_name): skew_mm}` in insertion order.
pub type SkewData = Vec<((String, String), f64)>;

/// `derive_skew_data` -> `(skew_data, threshold_map)`.
pub fn derive_skew_data(
    pcb: &Pcb,
    net_class_map: Option<&NetClassMap>,
    board_thickness_mm: Option<f64>,
    num_copper_layers: i64,
) -> (SkewData, crate::validate::diffpair_engagement::ThresholdMap) {
    let Some(map) = net_class_map.filter(|m| !m.is_empty()) else {
        return (vec![], vec![]);
    };
    let names = named_nets(pcb);
    if names.is_empty() {
        return (vec![], vec![]);
    }
    let routing = SynthRouting::from_net_class_map(map);
    let mut skew: SkewData = Vec::new();
    let mut thresholds: Vec<((i64, i64), f64)> = Vec::new();
    for d in detect_diff_pairs(&names, Some(&routing), None) {
        let pair = &d.pair;
        let (p, n) = (pair.positive.net_id, pair.negative.net_id);
        if !net_has_geometry(pcb, p) || !net_has_geometry(pcb, n) {
            continue;
        }
        let lp = measure_net_from_pcb(pcb, p, board_thickness_mm, num_copper_layers, true);
        let ln = measure_net_from_pcb(pcb, n, board_thickness_mm, num_copper_layers, true);
        let names_key = (
            pair.positive.net_name.clone(),
            pair.negative.net_name.clone(),
        );
        let v = (lp - ln).abs();
        match skew.iter_mut().find(|e| e.0 == names_key) {
            Some(e) => e.1 = v,
            None => skew.push((names_key, v)),
        }
        let k = key(p, n);
        let get = |nm: &str| map.iter().find(|(k, _)| k == nm).map(|(_, v)| v);
        let nc = get(&pair.positive.net_name).or_else(|| get(&pair.negative.net_name));
        let t = nc
            .map(|c| c.effective_skew_tolerance(DEFAULT_SKEW_TOLERANCE_MM))
            .unwrap_or(DEFAULT_SKEW_TOLERANCE_MM);
        match thresholds.iter_mut().find(|e| e.0 == k) {
            Some(e) => e.1 = t,
            None => thresholds.push((k, t)),
        }
    }
    (skew, thresholds)
}
