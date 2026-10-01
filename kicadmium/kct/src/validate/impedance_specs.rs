//! Single-ended impedance specs from a net-class map (port of
//! `kicad_tools.validate.impedance_specs`, issue #3157).

use crate::router::rules::NetClassMap;
use crate::validate::rules::impedance::NetImpedanceSpec;

/// One exact-name spec per net whose class declares
/// `target_single_impedance`.
pub fn derive_single_ended_impedance_specs(
    net_class_map: Option<&NetClassMap>,
) -> Vec<NetImpedanceSpec> {
    let Some(map) = net_class_map else {
        return vec![];
    };
    map.iter()
        .filter_map(|(net, nc)| {
            let target = nc.target_single_impedance?;
            Some(NetImpedanceSpec {
                target_z0: Some(target),
                tolerance_percent: nc.impedance_tolerance_percent,
                ..NetImpedanceSpec::new(format!("^{}$", regex::escape(net)))
            })
        })
        .collect()
}
