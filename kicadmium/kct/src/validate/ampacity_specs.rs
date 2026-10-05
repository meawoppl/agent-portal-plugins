//! Ampacity-spec derivation from a net-class map (port of
//! `kicad_tools.validate.ampacity_specs`).

use crate::router::rules::NetClassMap;

/// `{net_name: target_ampacity}` for every net whose class declares one.
/// `None` / empty map returns an empty list (standalone `kct check`).
pub fn derive_ampacity_specs(net_class_map: Option<&NetClassMap>) -> Vec<(String, f64)> {
    let Some(map) = net_class_map else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(net, nc)| nc.target_ampacity.map(|t| (net.clone(), t)))
        .collect()
}
