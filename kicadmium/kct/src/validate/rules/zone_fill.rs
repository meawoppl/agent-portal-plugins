//! Zone fill status and isolated fill islands (port of
//! `kicad_tools.validate.rules.zone_fill`).

use super::dangling_copper::{
    build_copper_layer_indexes, cluster_copper_kinds, copper_layer_names, net_resolver, ClusterSeed,
};
use crate::geometry::shapely::{self as sh, Geom, Poly};
use crate::manufacturers::DesignRules;
use crate::schema::pcb::{Pcb, Zone};
use crate::validate::violations::{DRCResults, DRCViolation};

fn zone_bbox(z: &Zone) -> String {
    if z.polygon.is_empty() {
        return "no boundary".into();
    }
    let xs = z.polygon.iter().map(|p| p.0);
    let ys = z.polygon.iter().map(|p| p.1);
    let min = |it: &mut dyn Iterator<Item = f64>| it.reduce(|a, b| if b < a { b } else { a }).unwrap();
    let max = |it: &mut dyn Iterator<Item = f64>| it.reduce(|a, b| if b > a { b } else { a }).unwrap();
    format!(
        "({:.2}, {:.2}) to ({:.2}, {:.2}) mm",
        min(&mut xs.clone()),
        min(&mut ys.clone()),
        max(&mut xs.clone()),
        max(&mut ys.clone())
    )
}

use crate::utils::pymath::py_sum;

fn zone_center(z: &Zone) -> Option<(f64, f64)> {
    if z.polygon.is_empty() {
        return None;
    }
    let n = z.polygon.len() as f64;
    Some((
        py_sum(z.polygon.iter().map(|p| p.0)) / n,
        py_sum(z.polygon.iter().map(|p| p.1)) / n,
    ))
}

#[derive(Debug, Clone, Default)]
pub struct ZoneFillRule;

impl ZoneFillRule {
    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        for z in pcb.zones() {
            if z.keepout.is_some() {
                continue;
            }
            let net = if z.net_name.is_empty() {
                "unassigned"
            } else {
                &z.net_name
            };
            let layer = if z.layer.is_empty() { "unknown" } else { &z.layer };
            let bbox = zone_bbox(z);
            let center = zone_center(z);
            if !z.is_filled {
                results.add(
                    DRCViolation::new(
                        "zone_fill_disabled",
                        "warning",
                        format!("Zone fill disabled for net '{net}' on {layer} [{bbox}]"),
                    )
                    .location_opt(center)
                    .layer(layer)
                    .items([format!("net:{net}")]),
                );
            } else if z.filled_polygons.is_empty() {
                results.add(
                    DRCViolation::new(
                        "zone_unfilled",
                        "warning",
                        format!(
                            "Zone for net '{net}' on {layer} has fill enabled but no filled \
                             polygons [{bbox}]"
                        ),
                    )
                    .location_opt(center)
                    .layer(layer)
                    .items([format!("net:{net}")]),
                );
            }
            if z.net_number == 0 && z.net_name.is_empty() {
                results.add(
                    DRCViolation::new(
                        "zone_no_net",
                        "warning",
                        format!("Zone on {layer} has no net assigned [{bbox}]"),
                    )
                    .location_opt(center)
                    .layer(layer),
                );
            }
        }
        results
    }
}

struct FillIsland<'a> {
    zone: &'a Zone,
    layer: String,
    net_number: i64,
    net_name: String,
    polygon: Geom,
}

#[derive(Debug, Clone, Default)]
pub struct IsolatedCopperRule;

impl IsolatedCopperRule {
    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        results.set_rule("isolated_copper", 1);
        let islands = Self::collect_islands(pcb);
        if islands.is_empty() {
            return results;
        }
        let layers = copper_layer_names(pcb);
        let resolve = net_resolver(pcb);
        let conductors = build_copper_layer_indexes(pcb, &layers, &resolve, false);
        let seeds: Vec<ClusterSeed> = islands
            .iter()
            .map(|i| ClusterSeed {
                layer: i.layer.clone(),
                geom: i.polygon.clone(),
                prep: Some(std::sync::Arc::new(sh::Prepared::new(i.polygon.clone()))),
                net_number: i.net_number,
                net_name: i.net_name.clone(),
            })
            .collect();
        let kinds = cluster_copper_kinds(&conductors, &seeds);
        for (island, k) in islands.iter().zip(kinds) {
            if k.contains(&"pad") {
                continue;
            }
            let p = sh::representative_point(&island.polygon).unwrap_or((0.0, 0.0));
            let area = island.polygon.area();
            results.add(
                DRCViolation::new(
                    "isolated_copper",
                    "warning",
                    format!(
                        "Isolated copper fill: Zone [{}] on {}, priority {} (island area {area:.4} \
                         mm2)",
                        island.net_name, island.layer, island.zone.priority
                    ),
                )
                .at(p.0, p.1)
                .layer(island.layer.clone())
                .actual(area)
                .items([format!("net:{}", island.net_name)])
                .nets([island.net_name.clone()]),
            );
        }
        results
    }

    fn collect_islands(pcb: &Pcb) -> Vec<FillIsland<'_>> {
        let resolve = net_resolver(pcb);
        let mut out = Vec::new();
        for z in pcb.zones() {
            let net = resolve(z.net_number, &z.net_name);
            if net == 0 {
                continue;
            }
            let name = if z.net_name.is_empty() {
                pcb.get_net(net).map(|n| n.name.clone()).unwrap_or_default()
            } else {
                z.net_name.clone()
            };
            for (i, pts) in z.filled_polygons.iter().enumerate() {
                if pts.len() < 3 {
                    continue;
                }
                let poly = Poly::new(pts.clone());
                let geom = if sh::is_valid(&poly) {
                    Geom::Poly(poly)
                } else {
                    sh::make_valid(&poly)
                };
                if geom.is_empty() {
                    continue;
                }
                let layer = z.filled_polygon_layer(i);
                if layer.is_empty() {
                    continue;
                }
                out.push(FillIsland {
                    zone: z,
                    layer: layer.to_string(),
                    net_number: net,
                    net_name: name.clone(),
                    polygon: geom,
                });
            }
        }
        out
    }
}
