//! Dangling track ends and under-bonded vias (port of
//! `kicad_tools.validate.rules.dangling_copper`, Issue #4680), plus the
//! shared per-layer copper index and transitive clustering used by
//! `isolated_copper`.

use std::collections::HashMap;

use super::clearance::{collect_zone_fills, pad_on_layer, pad_polygon};
use crate::core::layers::via_spans_layer;
use crate::geometry::shapely::{self as sh, Geom};
use crate::geometry::strtree::StrTree;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::rules::DRC_TOLERANCE;
use crate::validate::violations::{DRCResults, DRCViolation};

const TOUCH_TOL_MM: f64 = DRC_TOLERANCE;
const VIA_MIN_BONDED_LAYERS: usize = 2;

/// One candidate copper feature on one layer.
#[derive(Debug, Clone)]
pub struct CopperItem {
    /// `segment` | `arc` | `pad` | `via` | `fill`.
    pub kind: &'static str,
    pub geom: Geom,
    pub net_number: i64,
    pub net_name: String,
    /// Identity of the source object (`None` for fills and arcs).
    pub key: Option<usize>,
    /// Indexed copy for large fill polygons.
    pub prep: Option<std::sync::Arc<sh::Prepared>>,
}

impl CopperItem {
    /// `self.geom.distance(other)`.
    pub fn distance_to(&self, other: &Geom) -> f64 {
        match &self.prep {
            Some(p) => sh::distance_prep(other, p),
            None => sh::distance(&self.geom, other),
        }
    }
}

/// STRtree over one layer's copper.
pub struct LayerIndex {
    pub items: Vec<CopperItem>,
    tree: StrTree,
}

impl LayerIndex {
    fn new(items: Vec<CopperItem>) -> Self {
        let tree = StrTree::new(&items.iter().map(|i| i.geom.bounds()).collect::<Vec<_>>());
        LayerIndex { items, tree }
    }

    pub fn candidates(&self, q: (f64, f64, f64, f64)) -> impl Iterator<Item = &CopperItem> {
        self.tree.query(q).into_iter().map(move |i| &self.items[i])
    }
}

/// `(layer, index)` in first-seen layer order.
pub type LayerIndexes = Vec<(String, LayerIndex)>;

fn get<'a>(idx: &'a LayerIndexes, layer: &str) -> Option<&'a LayerIndex> {
    idx.iter().find(|(l, _)| l == layer).map(|(_, i)| i)
}

/// A seed geometry whose cluster is interrogated.
#[derive(Debug, Clone)]
pub struct ClusterSeed {
    pub layer: String,
    pub geom: Geom,
    pub prep: Option<std::sync::Arc<sh::Prepared>>,
    pub net_number: i64,
    pub net_name: String,
}

/// `copper_nets_match`.
pub fn copper_nets_match(an: i64, aname: &str, bn: i64, bname: &str) -> bool {
    if an != 0 && bn != 0 {
        return an == bn;
    }
    if !aname.is_empty() && !bname.is_empty() {
        return aname == bname;
    }
    false
}

fn probe(g: &Geom) -> (f64, f64, f64, f64) {
    let (a, b, c, d) = g.bounds().unwrap_or((0.0, 0.0, 0.0, 0.0));
    (
        a - DRC_TOLERANCE,
        b - DRC_TOLERANCE,
        c + DRC_TOLERANCE,
        d + DRC_TOLERANCE,
    )
}

/// `cluster_copper_kinds`: kinds present in each seed's cluster.
pub fn cluster_copper_kinds(indexes: &LayerIndexes, seeds: &[ClusterSeed]) -> Vec<Vec<&'static str>> {
    if seeds.is_empty() {
        return vec![];
    }
    let seed_numbers: std::collections::HashSet<i64> =
        seeds.iter().filter(|s| s.net_number != 0).map(|s| s.net_number).collect();
    let seed_names: std::collections::HashSet<&str> = seeds
        .iter()
        .filter(|s| !s.net_name.is_empty())
        .map(|s| s.net_name.as_str())
        .collect();
    struct Node<'a> {
        layer: &'a str,
        geom: &'a Geom,
        prep: Option<&'a sh::Prepared>,
        number: i64,
        name: &'a str,
        kind: &'static str,
        key: Option<usize>,
    }
    let mut nodes: Vec<Node> = seeds
        .iter()
        .map(|s| Node {
            layer: &s.layer,
            geom: &s.geom,
            prep: s.prep.as_deref(),
            number: s.net_number,
            name: &s.net_name,
            kind: "fill",
            key: None,
        })
        .collect();
    for (layer, index) in indexes {
        for item in &index.items {
            let keep = (item.net_number != 0 && seed_numbers.contains(&item.net_number))
                || (!item.net_name.is_empty() && seed_names.contains(item.net_name.as_str()));
            if !keep {
                continue;
            }
            nodes.push(Node {
                layer,
                geom: &item.geom,
                prep: item.prep.as_deref(),
                number: item.net_number,
                name: &item.net_name,
                kind: item.kind,
                key: item.key,
            });
        }
    }
    let mut parent: Vec<usize> = (0..nodes.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut by_layer: Vec<(&str, Vec<usize>)> = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        match by_layer.iter_mut().find(|(l, _)| *l == n.layer) {
            Some(e) => e.1.push(i),
            None => by_layer.push((n.layer, vec![i])),
        }
    }
    for (_, idxs) in &by_layer {
        let tree = StrTree::new(&idxs.iter().map(|&i| nodes[i].geom.bounds()).collect::<Vec<_>>());
        for &i in idxs {
            for pos in tree.query(probe(nodes[i].geom)) {
                let j = idxs[pos];
                if j <= i {
                    continue;
                }
                if !copper_nets_match(nodes[i].number, nodes[i].name, nodes[j].number, nodes[j].name) {
                    continue;
                }
                let d = match (nodes[i].prep, nodes[j].prep) {
                    (_, Some(pj)) => sh::distance_prep(nodes[i].geom, pj),
                    (Some(pi), None) => sh::distance_prep(nodes[j].geom, pi),
                    (None, None) => sh::distance(nodes[i].geom, nodes[j].geom),
                };
                if d <= DRC_TOLERANCE {
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    if ri != rj {
                        parent[rj] = ri;
                    }
                }
            }
        }
    }
    let mut by_key: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        if let Some(k) = n.key {
            by_key.entry(k).or_default().push(i);
        }
    }
    for group in by_key.values() {
        for &n in &group[1..] {
            let (ra, rb) = (find(&mut parent, group[0]), find(&mut parent, n));
            if ra != rb {
                parent[rb] = ra;
            }
        }
    }
    let mut kinds: HashMap<usize, Vec<&'static str>> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let r = find(&mut parent, i);
        let e = kinds.entry(r).or_default();
        if !e.contains(&node.kind) {
            e.push(node.kind);
        }
    }
    (0..seeds.len())
        .map(|i| {
            let r = find(&mut parent, i);
            kinds[&r].clone()
        })
        .collect()
}

fn fill_net_matches(net_number: i64, net_name: &str, fill: &CopperItem) -> bool {
    if net_number != 0 && fill.net_number != 0 {
        return net_number == fill.net_number;
    }
    if !net_name.is_empty() && !fill.net_name.is_empty() {
        return net_name == fill.net_name;
    }
    net_number == 0 && net_name.is_empty()
}

/// Key spaces for object identity.
const KEY_SEGMENT: usize = 0;
const KEY_VIA: usize = 1 << 40;
const KEY_PAD: usize = 2 << 40;

/// Segment copper (`_segment_copper`).
fn segment_copper(start: (f64, f64), end: (f64, f64), width: f64) -> Option<Geom> {
    if width <= 0.0 {
        return None;
    }
    Some(sh::segment_buffer(start, end, width / 2.0))
}

/// `build_copper_layer_indexes`.
pub fn build_copper_layer_indexes(
    pcb: &Pcb,
    copper_layers: &[String],
    resolve_net: &dyn Fn(i64, &str) -> i64,
    include_fills: bool,
) -> LayerIndexes {
    let mut by_layer: Vec<(String, Vec<CopperItem>)> = Vec::new();
    let mut add = |layer: &str, item: CopperItem| match by_layer.iter_mut().find(|(l, _)| l == layer) {
        Some(e) => e.1.push(item),
        None => by_layer.push((layer.to_string(), vec![item])),
    };
    for (i, s) in pcb.segments().iter().enumerate() {
        let Some(geom) = segment_copper(s.start, s.end, s.width) else {
            continue;
        };
        if s.layer.is_empty() {
            continue;
        }
        add(
            &s.layer,
            CopperItem {
                kind: "segment",
                geom,
                net_number: resolve_net(s.net_number, &s.net_name),
                net_name: s.net_name.clone(),
                key: Some(KEY_SEGMENT + i),
                prep: None,
            },
        );
    }
    for a in pcb.arcs() {
        if a.width <= 0.0 || a.layer.is_empty() {
            continue;
        }
        let r = a.width / 2.0;
        // LineString(start, mid, end).buffer(r): union of the two chord
        // capsules (identical distance semantics).
        let parts: Vec<sh::Poly> = [(a.start, a.mid), (a.mid, a.end)]
            .iter()
            .filter_map(|(p, q)| match sh::segment_buffer(*p, *q, r) {
                Geom::Poly(p) => Some(p),
                _ => None,
            })
            .collect();
        add(
            &a.layer,
            CopperItem {
                kind: "arc",
                geom: Geom::Multi(parts),
                net_number: resolve_net(a.net_number, &a.net_name),
                net_name: a.net_name.clone(),
                key: None,
                prep: None,
            },
        );
    }
    for (i, v) in pcb.vias().iter().enumerate() {
        let r = v.size / 2.0;
        if r <= 0.0 {
            continue;
        }
        let disc = sh::point_buffer(v.position, r);
        let net = resolve_net(v.net_number, &v.net_name);
        for layer in copper_layers {
            if via_spans_layer(&v.layers, layer) {
                add(
                    layer,
                    CopperItem {
                        kind: "via",
                        geom: disc.clone(),
                        net_number: net,
                        net_name: v.net_name.clone(),
                        key: Some(KEY_VIA + i),
                        prep: None,
                    },
                );
            }
        }
    }
    let mut pad_id = 0usize;
    for fp in pcb.footprints() {
        for pad in &fp.pads {
            pad_id += 1;
            let Some(poly) = pad_polygon(pad, fp) else {
                continue;
            };
            let net = resolve_net(pad.net_number, &pad.net_name);
            for layer in copper_layers {
                if pad_on_layer(pad, layer) {
                    add(
                        layer,
                        CopperItem {
                            kind: "pad",
                            geom: poly.clone(),
                            net_number: net,
                            net_name: pad.net_name.clone(),
                            key: Some(KEY_PAD + pad_id),
                            prep: None,
                        },
                    );
                }
            }
        }
    }
    if include_fills {
        for (layer, fills) in collect_zone_fills(pcb, false) {
            for f in fills {
                add(
                    &layer,
                    CopperItem {
                        kind: "fill",
                        geom: f.polygon,
                        net_number: f.net_number,
                        net_name: f.net_name,
                        key: None,
                        prep: Some(f.prep),
                    },
                );
            }
        }
    }
    by_layer
        .into_iter()
        .map(|(l, items)| (l, LayerIndex::new(items)))
        .collect()
}

/// Board copper layer names, falling back to `F.Cu` / `B.Cu`.
pub fn copper_layer_names(pcb: &Pcb) -> Vec<String> {
    let names: Vec<String> = pcb.copper_layers().iter().map(|l| l.name.clone()).collect();
    if names.is_empty() {
        vec!["F.Cu".into(), "B.Cu".into()]
    } else {
        names
    }
}

/// `{name: number}` resolver for the KiCad-10 name-only dialect.
pub fn net_resolver(pcb: &Pcb) -> impl Fn(i64, &str) -> i64 + '_ {
    let map: HashMap<String, i64> = pcb
        .nets()
        .iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| (n.name.clone(), n.number))
        .collect();
    move |number: i64, name: &str| {
        if number == 0 && !name.is_empty() {
            map.get(name).copied().unwrap_or(0)
        } else {
            number
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DanglingCopperRule;

impl DanglingCopperRule {
    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        results.set_rule("track_dangling", 1);
        results.set_rule("via_dangling", 1);
        let layers = copper_layer_names(pcb);
        let resolve = net_resolver(pcb);
        let indexes = build_copper_layer_indexes(pcb, &layers, &resolve, true);
        self.check_tracks(pcb, &indexes, &resolve, &mut results);
        self.check_vias(pcb, &layers, &indexes, &resolve, &mut results);
        results
    }

    fn point_terminated(
        p: (f64, f64),
        self_key: usize,
        net_number: i64,
        net_name: &str,
        index: Option<&LayerIndex>,
        radius: f64,
    ) -> bool {
        let Some(index) = index else {
            return false;
        };
        let reach = radius + TOUCH_TOL_MM;
        let pt = Geom::Point(p);
        for item in index.candidates((p.0 - reach, p.1 - reach, p.0 + reach, p.1 + reach)) {
            if item.key == Some(self_key) {
                continue;
            }
            if item.kind == "fill" && !fill_net_matches(net_number, net_name, item) {
                continue;
            }
            if item.distance_to(&pt) <= reach {
                return true;
            }
        }
        false
    }

    fn check_tracks(
        &self,
        pcb: &Pcb,
        indexes: &LayerIndexes,
        resolve: &dyn Fn(i64, &str) -> i64,
        results: &mut DRCResults,
    ) {
        for (i, s) in pcb.segments().iter().enumerate() {
            if s.width <= 0.0 || s.layer.is_empty() {
                continue;
            }
            let index = get(indexes, &s.layer);
            let net = resolve(s.net_number, &s.net_name);
            let endpoints = if s.start == s.end {
                vec![s.start]
            } else {
                vec![s.start, s.end]
            };
            let dangling: Vec<(f64, f64)> = endpoints
                .iter()
                .copied()
                .filter(|&p| {
                    !Self::point_terminated(p, KEY_SEGMENT + i, net, &s.net_name, index, s.width / 2.0)
                })
                .collect();
            if dangling.is_empty() {
                continue;
            }
            let length = crate::utils::pymath::dist(s.start, s.end);
            let label = if !s.net_name.is_empty() {
                s.net_name.clone()
            } else if net != 0 {
                format!("net {net}")
            } else {
                "unassigned".into()
            };
            let items = if s.uuid.is_empty() {
                vec![format!("net:{label}")]
            } else {
                vec![s.uuid.clone(), format!("net:{label}")]
            };
            let both = endpoints.len() == 2 && dangling.len() == 2;
            results.add(
                DRCViolation::new(
                    "track_dangling",
                    "warning",
                    format!(
                        "Dangling track end: Track [{label}] on {}, length {length:.4} mm{}",
                        s.layer,
                        if both { " (both ends dangling)" } else { "" }
                    ),
                )
                .at(dangling[0].0, dangling[0].1)
                .layer(s.layer.clone())
                .actual(length)
                .items(items)
                .nets([label]),
            );
        }
    }

    fn check_vias(
        &self,
        pcb: &Pcb,
        layers: &[String],
        indexes: &LayerIndexes,
        resolve: &dyn Fn(i64, &str) -> i64,
        results: &mut DRCResults,
    ) {
        for (i, v) in pcb.vias().iter().enumerate() {
            let r = v.size / 2.0;
            if r <= 0.0 {
                continue;
            }
            let (vx, vy) = v.position;
            let center = Geom::Point(v.position);
            let q = (
                vx - r - TOUCH_TOL_MM,
                vy - r - TOUCH_TOL_MM,
                vx + r + TOUCH_TOL_MM,
                vy + r + TOUCH_TOL_MM,
            );
            let net = resolve(v.net_number, &v.net_name);
            let mut bonded = 0usize;
            for layer in layers {
                if !via_spans_layer(&v.layers, layer) {
                    continue;
                }
                let Some(index) = get(indexes, layer) else {
                    continue;
                };
                for item in index.candidates(q) {
                    if item.key == Some(KEY_VIA + i) {
                        continue;
                    }
                    if item.kind == "fill" && !fill_net_matches(net, &v.net_name, item) {
                        continue;
                    }
                    if item.distance_to(&center) <= r + TOUCH_TOL_MM {
                        bonded += 1;
                        break;
                    }
                }
            }
            if bonded >= VIA_MIN_BONDED_LAYERS {
                continue;
            }
            let label = if !v.net_name.is_empty() {
                v.net_name.clone()
            } else if net != 0 {
                format!("net {net}")
            } else {
                "unassigned".into()
            };
            let top = v.layers.first().cloned().unwrap_or_else(|| "F.Cu".into());
            let bottom = v.layers.last().cloned().unwrap_or_else(|| "B.Cu".into());
            results.add(
                DRCViolation::new(
                    "via_dangling",
                    "warning",
                    format!(
                        "Dangling via: Via [{label}] on {top} - {bottom} bonds copper on {bonded} \
                         layer(s)"
                    ),
                )
                .at(vx, vy)
                .layer(top)
                .actual(bonded as f64)
                .required(VIA_MIN_BONDED_LAYERS as f64)
                .items([format!("net:{label}")])
                .nets([label]),
            );
        }
    }
}
