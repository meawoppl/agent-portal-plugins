//! Net connectivity status (port of `kicad_tools.analysis.net_status`'s
//! `NetStatusAnalyzer`, as consumed by the `connectivity` DRC rule).
//!
//! Only the pad partition matters to consumers, so the adjacency graph is
//! kept as a union-find over pad nodes; island order (first pad
//! occurrence, then stable largest-first) matches upstream exactly.

use std::collections::HashMap;

use crate::core::layers::via_spans_layer;
use crate::geometry::shapely::{self as sh, Geom};
use crate::schema::pcb::{Pcb, Zone};
use crate::validate::connectivity as cv;

pub const POSITION_TOLERANCE: f64 = 0.01;

/// A pad on a net.
#[derive(Debug, Clone, PartialEq)]
pub struct PadInfo {
    pub reference: String,
    pub pad_number: String,
    pub position: (f64, f64),
    pub is_connected: bool,
    pub layers: Vec<String>,
    pub node_id: String,
}

impl PadInfo {
    pub fn connectivity_id(&self) -> &str {
        if self.node_id.is_empty() {
            return "";
        }
        &self.node_id
    }

    pub fn full_name(&self) -> String {
        format!("{}.{}", self.reference, self.pad_number)
    }
}

/// Status of one net (`NetStatus`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetStatus {
    pub net_number: i64,
    pub net_name: String,
    pub net_class: String,
    pub total_pads: usize,
    pub connected_pads: Vec<PadInfo>,
    pub unconnected_pads: Vec<PadInfo>,
    pub is_plane_net: bool,
    pub plane_layer: String,
    pub plane_layers: Vec<String>,
    pub has_routing: bool,
    pub has_vias: bool,
    pub has_filled_zone: bool,
    pub source_file: String,
    pub island_count: usize,
}

impl NetStatus {
    pub fn connected_count(&self) -> usize {
        self.connected_pads.len()
    }

    pub fn unconnected_count(&self) -> usize {
        self.unconnected_pads.len()
    }

    pub fn status(&self) -> &'static str {
        if self.total_pads == 0 {
            return "unrouted";
        }
        if self.total_pads == 1 || self.unconnected_count() == 0 {
            return "complete";
        }
        if self.connected_count() == 0 {
            return "unrouted";
        }
        "incomplete"
    }

    pub fn net_type(&self) -> &'static str {
        if self.is_plane_net {
            return "plane";
        }
        let n = &self.net_name;
        if n.starts_with(['+', '-', 'V'])
            || ["GND", "AGND", "DGND", "VCC", "VDD", "VSS"].contains(&n.as_str())
        {
            return "power";
        }
        "signal"
    }

    pub fn is_advisory_incomplete(&self) -> bool {
        self.status() == "incomplete"
            && (self.is_plane_net || matches!(self.net_type(), "plane" | "power"))
    }
}

/// Analyzer result (`NetStatusResult`, net list only).
#[derive(Debug, Clone, Default)]
pub struct NetStatusResult {
    pub nets: Vec<NetStatus>,
    pub total_nets: usize,
}

struct Dsu(Vec<usize>);

impl Dsu {
    fn new(n: usize) -> Self {
        Dsu((0..n).collect())
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra] = rb;
        }
    }
}

/// A copper conductor on the analysed net (segment or arc).
struct Conductor {
    layer: String,
    start: (f64, f64),
    end: (f64, f64),
    is_arc: bool,
    poly: Option<Geom>,
}

pub struct NetStatusAnalyzer<'a> {
    pub pcb: &'a Pcb,
    pub strict: bool,
    pub source_file: String,
    pad_polys: std::cell::OnceCell<HashMap<String, Option<Geom>>>,
}

fn points_close(a: (f64, f64), b: (f64, f64)) -> bool {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy < POSITION_TOLERANCE * POSITION_TOLERANCE
}

/// `_pad_layer_matches_zone`.
pub fn pad_layer_matches_zone(pad_layers: &[String], zone_layer: &str) -> bool {
    pad_layers.iter().any(|l| {
        l == zone_layer
            || (l == "*.Cu" && zone_layer.ends_with(".Cu"))
            || (l.starts_with("*.") && zone_layer.ends_with(&l[1..]))
    })
}

impl<'a> NetStatusAnalyzer<'a> {
    pub fn new(pcb: &'a Pcb, strict: bool) -> Self {
        NetStatusAnalyzer {
            pcb,
            strict: strict || !pcb.arcs().is_empty(),
            source_file: pcb
                .path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            pad_polys: Default::default(),
        }
    }

    fn pad_node_id(&self, fi: usize, pi: usize, reference: &str, number: &str) -> String {
        if self.strict {
            format!("__pad:{fi}:{pi}")
        } else {
            format!("{reference}.{number}")
        }
    }

    /// `analyze()` (without the advisory pour-net reclassification set).
    pub fn analyze(&self) -> NetStatusResult {
        let mut result = NetStatusResult::default();
        let nets: Vec<(i64, String)> = self
            .pcb
            .nets()
            .iter()
            .filter(|n| n.number != 0 && !n.name.is_empty())
            .map(|n| (n.number, n.name.clone()))
            .collect();
        result.total_nets = nets.len();
        let zone_nets = build_zone_net_map(self.pcb);
        for (number, name) in &nets {
            result
                .nets
                .push(self.analyze_net(*number, name, &zone_nets));
        }
        let order = |s: &str| match s {
            "incomplete" => 0,
            "unrouted" => 1,
            "complete" => 2,
            _ => 3,
        };
        result.nets.sort_by(|a, b| {
            (order(a.status()), &a.net_name).cmp(&(order(b.status()), &b.net_name))
        });
        result
    }

    fn analyze_net(&self, number: i64, name: &str, zone_nets: &[(i64, Vec<String>)]) -> NetStatus {
        let mut st = NetStatus {
            net_number: number,
            net_name: name.to_string(),
            source_file: self.source_file.clone(),
            ..Default::default()
        };
        if let Some((_, layers)) = zone_nets.iter().find(|(n, _)| *n == number) {
            st.is_plane_net = true;
            st.plane_layers = layers.clone();
            st.plane_layer = layers[0].clone();
            st.has_filled_zone = self
                .pcb
                .zones()
                .iter()
                .any(|z| z.net_number == number && !z.filled_polygons.is_empty());
        }
        let conductors = self.conductors(number);
        st.has_routing = !conductors.is_empty();
        let vias: Vec<&crate::schema::pcb::Via> = self.pcb.vias_in_net(number).collect();
        st.has_vias = !vias.is_empty();
        let mut pads = self.net_pads(number);
        st.total_pads = pads.len();
        if pads.len() < 2 {
            st.island_count = pads.len();
            st.connected_pads = pads;
            return st;
        }
        // Distinct connectivity ids, first-occurrence order.
        let mut ids: Vec<String> = Vec::new();
        for p in &pads {
            if !ids.contains(&p.node_id) {
                ids.push(p.node_id.clone());
            }
        }
        let mut dsu = Dsu::new(ids.len());
        self.build_graph(number, &pads, &ids, &conductors, &vias, &mut dsu);
        // Islands in first-pad order.
        let mut islands: Vec<(usize, Vec<usize>)> = Vec::new();
        for i in 0..ids.len() {
            let r = dsu.find(i);
            match islands.iter_mut().find(|(k, _)| *k == r) {
                Some(e) => e.1.push(i),
                None => islands.push((r, vec![i])),
            }
        }
        st.island_count = islands.len();
        islands.sort_by_key(|(_, m)| std::cmp::Reverse(m.len()));
        let connected: Vec<&str> = islands
            .first()
            .map(|(_, m)| m.iter().map(|&i| ids[i].as_str()).collect())
            .unwrap_or_default();
        for p in pads.iter_mut() {
            p.is_connected = connected.contains(&p.node_id.as_str());
        }
        for p in pads {
            if p.is_connected {
                st.connected_pads.push(p);
            } else {
                st.unconnected_pads.push(p);
            }
        }
        st.unconnected_pads
            .sort_by(|a, b| (&a.reference, &a.pad_number).cmp(&(&b.reference, &b.pad_number)));
        st
    }

    fn net_pads(&self, number: i64) -> Vec<PadInfo> {
        let mut out = Vec::new();
        for (fi, fp) in self.pcb.footprints().iter().enumerate() {
            if fp.reference.is_empty() || fp.reference.starts_with('#') {
                continue;
            }
            for (pi, pad) in fp.pads.iter().enumerate() {
                if pad.net_number == number {
                    out.push(PadInfo {
                        reference: fp.reference.clone(),
                        pad_number: pad.number.clone(),
                        position: cv::transform_pad_position(
                            pad.position,
                            fp.position,
                            fp.rotation,
                        ),
                        is_connected: false,
                        layers: pad.layers.clone(),
                        node_id: self.pad_node_id(fi, pi, &fp.reference, &pad.number),
                    });
                }
            }
        }
        out
    }

    fn conductors(&self, number: i64) -> Vec<Conductor> {
        let mut out: Vec<Conductor> = self
            .pcb
            .segments_in_net(number)
            .map(|s| Conductor {
                layer: s.layer.clone(),
                start: s.start,
                end: s.end,
                is_arc: false,
                poly: Some(segment_copper_polygon(s.start, s.end, s.width)),
            })
            .collect();
        for a in self.pcb.arcs_in_net(number) {
            let error = 0.00001f64.min(a.width / 1000.0);
            let radius = a.width / 2.0;
            let step = 4.0 * (error / (2.0 * radius)).min(0.5).sqrt().asin();
            let quad = (std::f64::consts::PI / (2.0 * step)).ceil().max(16.0) as usize;
            let pts = a
                .centerline_points(error)
                .unwrap_or_else(|_| vec![a.start, a.end]);
            let poly = Some(polyline_buffer(&pts, radius, quad));
            out.push(Conductor {
                layer: a.layer.clone(),
                start: a.start,
                end: a.end,
                is_arc: true,
                poly,
            });
        }
        out
    }

    fn pad_polys(&self) -> &HashMap<String, Option<Geom>> {
        self.pad_polys.get_or_init(|| {
            let mut m = HashMap::new();
            for (fi, fp) in self.pcb.footprints().iter().enumerate() {
                if fp.reference.is_empty() || fp.reference.starts_with('#') {
                    continue;
                }
                for (pi, pad) in fp.pads.iter().enumerate() {
                    if pad.number.is_empty() {
                        continue;
                    }
                    m.insert(
                        self.pad_node_id(fi, pi, &fp.reference, &pad.number),
                        cv::pad_copper_polygon(fp, pad, false),
                    );
                }
            }
            m
        })
    }

    fn pads_touching(&self, geom: &Geom, pads: &[PadInfo], layer: Option<&str>) -> Vec<String> {
        let polys = self.pad_polys();
        let mut out = Vec::new();
        for p in pads {
            if out.contains(&p.node_id) {
                continue;
            }
            if let Some(l) = layer {
                if !pad_layer_matches_zone(&p.layers, l) {
                    continue;
                }
            }
            if let Some(Some(pg)) = polys.get(&p.node_id) {
                if sh::intersects(geom, pg) {
                    out.push(p.node_id.clone());
                }
            }
        }
        out
    }

    fn pads_at(&self, pt: (f64, f64), pads: &[PadInfo]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for p in pads {
            if points_close(pt, p.position) && !out.contains(&p.node_id) {
                out.push(p.node_id.clone());
            }
        }
        out
    }

    fn conductor_pads(&self, c: &Conductor, pads: &[PadInfo]) -> Vec<String> {
        if self.strict {
            match &c.poly {
                Some(g) => self.pads_touching(g, pads, Some(&c.layer)),
                None => vec![],
            }
        } else {
            let mut v = self.pads_at(c.start, pads);
            v.extend(self.pads_at(c.end, pads));
            v
        }
    }

    fn segment_touches_via(c: &Conductor, via_pos: (f64, f64), via_geom: &Geom) -> bool {
        if !c.is_arc && (points_close(c.start, via_pos) || points_close(c.end, via_pos)) {
            return true;
        }
        match &c.poly {
            Some(p) => sh::intersects(p, via_geom),
            None => {
                let line = Geom::Line(vec![c.start, c.end]);
                sh::intersects(&line, via_geom)
            }
        }
    }

    fn segment_components(&self, cs: &[Conductor]) -> Vec<Vec<usize>> {
        let n = cs.len();
        let mut dsu = Dsu::new(n);
        if self.strict {
            let mut by_layer: Vec<(&str, Vec<usize>)> = Vec::new();
            for (i, c) in cs.iter().enumerate() {
                if c.poly.is_some() {
                    match by_layer.iter_mut().find(|(l, _)| *l == c.layer) {
                        Some(e) => e.1.push(i),
                        None => by_layer.push((&c.layer, vec![i])),
                    }
                }
            }
            for (_, idx) in by_layer {
                let bounds: Vec<_> = idx
                    .iter()
                    .map(|&i| cs[i].poly.as_ref().and_then(Geom::bounds).unwrap())
                    .collect();
                for (a, b) in crate::validate::spatial::candidate_pairs(&bounds, 0.0) {
                    let (i, j) = (idx[a], idx[b]);
                    if sh::intersects(cs[i].poly.as_ref().unwrap(), cs[j].poly.as_ref().unwrap()) {
                        dsu.union(i, j);
                    }
                }
            }
        } else {
            for i in 0..n {
                for j in i + 1..n {
                    let (a, b) = (&cs[i], &cs[j]);
                    if points_close(a.start, b.start)
                        || points_close(a.start, b.end)
                        || points_close(a.end, b.start)
                        || points_close(a.end, b.end)
                    {
                        dsu.union(i, j);
                    }
                }
            }
        }
        group(&mut dsu, n)
    }

    fn merge_chains_via_vias(
        &self,
        cs: &[Conductor],
        comps: &[Vec<usize>],
        vias: &[&crate::schema::pcb::Via],
        require_layer_span: bool,
    ) -> Vec<Vec<usize>> {
        let mut dsu = Dsu::new(comps.len());
        for v in vias {
            let vg = cv::via_copper_geom(v.position, v.size.max(0.0) / 2.0);
            let touching: Vec<usize> = comps
                .iter()
                .enumerate()
                .filter(|(_, comp)| {
                    comp.iter().any(|&s| {
                        Self::segment_touches_via(&cs[s], v.position, &vg)
                            && (!require_layer_span || via_spans_layer(&v.layers, &cs[s].layer))
                    })
                })
                .map(|(i, _)| i)
                .collect();
            for &o in touching.iter().skip(1) {
                dsu.union(touching[0], o);
            }
        }
        let mut merged: Vec<(usize, Vec<usize>)> = Vec::new();
        for (ci, comp) in comps.iter().enumerate() {
            let r = dsu.find(ci);
            match merged.iter_mut().find(|(k, _)| *k == r) {
                Some(e) => e.1.extend(comp),
                None => merged.push((r, comp.clone())),
            }
        }
        merged.into_iter().map(|(_, v)| v).collect()
    }

    fn build_graph(
        &self,
        number: i64,
        pads: &[PadInfo],
        ids: &[String],
        cs: &[Conductor],
        vias: &[&crate::schema::pcb::Via],
        dsu: &mut Dsu,
    ) {
        let idx = |id: &str| ids.iter().position(|x| x == id).unwrap();
        let connect = |dsu: &mut Dsu, members: &[String]| {
            for w in members.windows(2) {
                dsu.union(idx(&w[0]), idx(&w[1]));
            }
        };
        let comps = self.segment_components(cs);
        for comp in &comps {
            let mut m: Vec<String> = Vec::new();
            for &s in comp {
                m.extend(self.conductor_pads(&cs[s], pads));
            }
            connect(dsu, &m);
        }
        let via_geoms: Vec<Geom> = vias
            .iter()
            .map(|v| cv::via_copper_geom(v.position, v.size.max(0.0) / 2.0))
            .collect();
        for (v, vg) in vias.iter().zip(&via_geoms) {
            let m = if self.strict {
                self.pads_touching(vg, pads, None)
            } else {
                self.pads_at(v.position, pads)
            };
            connect(dsu, &m);
        }
        for chain in self.merge_chains_via_vias(cs, &comps, vias, true) {
            let mut m: Vec<String> = Vec::new();
            for &s in &chain {
                m.extend(self.conductor_pads(&cs[s], pads));
            }
            for (v, vg) in vias.iter().zip(&via_geoms) {
                if chain.iter().any(|&s| {
                    Self::segment_touches_via(&cs[s], v.position, vg)
                        && via_spans_layer(&v.layers, &cs[s].layer)
                }) {
                    if self.strict {
                        m.extend(self.pads_touching(vg, pads, None));
                    } else {
                        m.extend(self.pads_at(v.position, pads));
                    }
                }
            }
            connect(dsu, &m);
        }
        for g in self.fill_island_groups(number, pads, cs, &comps, vias) {
            connect(dsu, &g);
        }
        if !self.strict {
            let mut copper: Vec<(f64, f64)> = cs.iter().map(|c| c.start).collect();
            copper.extend(cs.iter().map(|c| c.end));
            copper.extend(vias.iter().map(|v| v.position));
            for p in pads {
                for &cp in &copper {
                    if points_close(p.position, cp) {
                        for o in pads {
                            if o.node_id != p.node_id && points_close(cp, o.position) {
                                dsu.union(idx(&p.node_id), idx(&o.node_id));
                            }
                        }
                    }
                }
            }
        }
    }

    fn fill_island_groups(
        &self,
        number: i64,
        pads: &[PadInfo],
        cs: &[Conductor],
        comps: &[Vec<usize>],
        vias: &[&crate::schema::pcb::Via],
    ) -> Vec<Vec<String>> {
        let mut pad_polys: Vec<(String, Geom)> = Vec::new();
        for (fi, fp) in self.pcb.footprints().iter().enumerate() {
            if fp.reference.is_empty() || fp.reference.starts_with('#') {
                continue;
            }
            for (pi, pad) in fp.pads.iter().enumerate() {
                if pad.net_number != number || pad.number.is_empty() {
                    continue;
                }
                let id = self.pad_node_id(fi, pi, &fp.reference, &pad.number);
                if !pads.iter().any(|p| p.node_id == id) {
                    continue;
                }
                if let Some(poly) = cv::pad_copper_polygon(fp, pad, false) {
                    match pad_polys.iter_mut().find(|(k, _)| *k == id) {
                        Some(e) => e.1 = poly,
                        None => pad_polys.push((id, poly)),
                    }
                }
            }
        }
        struct ViaGeoms {
            pos: (f64, f64),
            layers: Vec<String>,
            eroded: Geom,
            raw: Option<Geom>,
            annulus: Geom,
        }
        let vgs: Vec<ViaGeoms> = vias
            .iter()
            .map(|v| {
                let r = v.size.max(0.0) / 2.0;
                ViaGeoms {
                    pos: v.position,
                    layers: v.layers.clone(),
                    eroded: cv::via_copper_geom(v.position, r),
                    raw: (r > 0.0).then(|| sh::point_buffer(v.position, r)),
                    annulus: cv::physical_via_annulus(v),
                }
            })
            .collect();
        let chains = self.merge_chains_via_vias(cs, comps, vias, self.strict);
        let chain_pads: Vec<Vec<String>> = chains
            .iter()
            .map(|ch| {
                let mut m = Vec::new();
                for &s in ch {
                    m.extend(self.conductor_pads(&cs[s], pads));
                }
                m
            })
            .collect();
        let seg_poly = |s: usize| -> Geom {
            cs[s]
                .poly
                .clone()
                .unwrap_or_else(|| segment_copper_polygon(cs[s].start, cs[s].end, 0.0))
        };
        let pad_layers = |id: &str| -> &[String] {
            pads.iter()
                .find(|p| p.node_id == id)
                .map(|p| p.layers.as_slice())
                .unwrap_or(&[])
        };
        let mut groups: Vec<Vec<String>> = Vec::new();
        for zone in self.pcb.zones() {
            if zone.net_number != number || zone.filled_polygons.is_empty() {
                continue;
            }
            let n = zone.filled_polygons.len();
            let regions: Vec<Option<sh::Prepared>> = zone
                .filled_polygons
                .iter()
                .map(|pts| cv::fill_solid_region(pts).map(sh::Prepared::new))
                .collect();
            let layers: Vec<&str> = (0..n).map(|i| zone.filled_polygon_layer(i)).collect();
            let mut dsu = Dsu::new(n);
            let infl = zone.fill_inflation();
            if infl > 0.0 {
                for (i, j) in adjacent_fill_pairs(&regions, &layers, 2.0 * infl) {
                    dsu.union(i, j);
                }
            }
            for vg in &vgs {
                let touched: Vec<usize> = (0..n)
                    .filter(|&i| {
                        regions[i].as_ref().is_some_and(|r| {
                            via_spans_layer(&vg.layers, layers[i])
                                && sh::intersects_prep(&vg.annulus, r)
                        })
                    })
                    .collect();
                for &o in touched.iter().skip(1) {
                    dsu.union(touched[0], o);
                }
            }
            for ch in &chains {
                let chain_vias: Vec<&ViaGeoms> = vgs
                    .iter()
                    .filter(|vg| {
                        ch.iter().any(|&s| {
                            via_spans_layer(&vg.layers, &cs[s].layer)
                                && Self::segment_touches_via(&cs[s], vg.pos, &vg.eroded)
                        })
                    })
                    .collect();
                let touched: Vec<usize> = (0..n)
                    .filter(|&i| {
                        let Some(r) = &regions[i] else { return false };
                        ch.iter().any(|&s| {
                            via_spans_layer(std::slice::from_ref(&cs[s].layer), layers[i])
                                && sh::intersects_prep(&seg_poly(s), r)
                        }) || chain_vias.iter().any(|vg| {
                            via_spans_layer(&vg.layers, layers[i])
                                && sh::intersects_prep(&vg.annulus, r)
                        })
                    })
                    .collect();
                for &o in touched.iter().skip(1) {
                    dsu.union(touched[0], o);
                }
            }
            let mut cluster: Vec<(usize, Vec<String>)> = Vec::new();
            for i in 0..n {
                let Some(region) = &regions[i] else { continue };
                let root = dsu.find(i);
                let ci = match cluster.iter().position(|(k, _)| *k == root) {
                    Some(c) => c,
                    None => {
                        cluster.push((root, Vec::new()));
                        cluster.len() - 1
                    }
                };
                let mut bonded = std::mem::take(&mut cluster[ci].1);
                let add = |b: &mut Vec<String>, id: &str| {
                    if !b.iter().any(|x| x == id) {
                        b.push(id.to_string());
                    }
                };
                for (id, g) in &pad_polys {
                    if pad_layer_matches_zone(pad_layers(id), layers[i])
                        && sh::intersects_prep(g, region)
                    {
                        add(&mut bonded, id);
                    }
                }
                for vg in &vgs {
                    if !via_spans_layer(&vg.layers, layers[i])
                        || !sh::intersects_prep(&vg.annulus, region)
                    {
                        continue;
                    }
                    for id in self.pads_at(vg.pos, pads) {
                        add(&mut bonded, &id);
                    }
                    if let Some(raw) = &vg.raw {
                        for (id, g) in &pad_polys {
                            if !bonded.contains(id) && sh::intersects(raw, g) {
                                add(&mut bonded, id);
                            }
                        }
                    }
                    for (ch, cp) in chains.iter().zip(&chain_pads) {
                        let touches = ch.iter().any(|&s| {
                            Self::segment_touches_via(&cs[s], vg.pos, &vg.eroded)
                                && (!self.strict || via_spans_layer(&vg.layers, &cs[s].layer))
                        });
                        if touches {
                            for id in cp {
                                add(&mut bonded, id);
                            }
                        }
                    }
                }
                for (ch, cp) in chains.iter().zip(&chain_pads) {
                    if cp.is_empty() {
                        continue;
                    }
                    if ch.iter().any(|&s| {
                        via_spans_layer(std::slice::from_ref(&cs[s].layer), layers[i])
                            && sh::intersects_prep(&seg_poly(s), region)
                    }) {
                        for id in cp {
                            add(&mut bonded, id);
                        }
                    }
                }
                cluster[ci].1 = bonded;
            }
            for (_, b) in cluster {
                if !b.is_empty() {
                    groups.push(b);
                }
            }
        }
        groups
    }
}

fn group(dsu: &mut Dsu, n: usize) -> Vec<Vec<usize>> {
    let mut out: Vec<(usize, Vec<usize>)> = Vec::new();
    for i in 0..n {
        let r = dsu.find(i);
        match out.iter_mut().find(|(k, _)| *k == r) {
            Some(e) => e.1.push(i),
            None => out.push((r, vec![i])),
        }
    }
    out.into_iter().map(|(_, v)| v).collect()
}

/// `geometry.copper.segment_copper_polygon`.
pub fn segment_copper_polygon(start: (f64, f64), end: (f64, f64), width: f64) -> Geom {
    if start == end {
        if width <= 0.0 {
            return Geom::Point(start);
        }
        return sh::point_buffer(start, width / 2.0);
    }
    if width <= 0.0 {
        return Geom::Line(vec![start, end]);
    }
    sh::segment_buffer(start, end, width / 2.0)
}

/// `LineString(pts).buffer(r)` approximated as the union of per-edge
/// capsules (identical predicate results for intersects / distance).
pub fn polyline_buffer(pts: &[(f64, f64)], r: f64, quad: usize) -> Geom {
    let polys: Vec<sh::Poly> = pts
        .windows(2)
        .filter_map(|w| match sh::segment_buffer_q(w[0], w[1], r, quad) {
            Geom::Poly(p) => Some(p),
            _ => None,
        })
        .collect();
    Geom::Multi(polys)
}

fn adjacent_fill_pairs(
    regions: &[Option<sh::Prepared>],
    layers: &[&str],
    reach: f64,
) -> Vec<(usize, usize)> {
    let idx: Vec<usize> = (0..regions.len())
        .filter(|&i| regions[i].is_some())
        .collect();
    if idx.len() < 2 {
        return vec![];
    }
    let bounds: Vec<_> = idx
        .iter()
        .map(|&i| regions[i].as_ref().unwrap().bounds().unwrap())
        .collect();
    let mut out = Vec::new();
    for (a, b) in crate::validate::spatial::candidate_pairs(&bounds, reach) {
        let (i, j) = (idx[a], idx[b]);
        if layers[i] == layers[j]
            && sh::distance_prep(
                &regions[i].as_ref().unwrap().geom,
                regions[j].as_ref().unwrap(),
            ) <= reach
        {
            out.push((i, j));
        }
    }
    out
}

/// `build_zone_net_map`: `net -> [zone layers]` (first-seen order).
pub fn build_zone_net_map(pcb: &Pcb) -> Vec<(i64, Vec<String>)> {
    let mut out: Vec<(i64, Vec<String>)> = Vec::new();
    for z in pcb.zones() {
        let z: &Zone = z;
        if z.net_number <= 0 {
            continue;
        }
        let i = match out.iter().position(|(n, _)| *n == z.net_number) {
            Some(i) => i,
            None => {
                out.push((z.net_number, Vec::new()));
                out.len() - 1
            }
        };
        if !out[i].1.contains(&z.layer) {
            out[i].1.push(z.layer.clone());
        }
    }
    out
}
