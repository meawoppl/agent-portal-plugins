//! Branch-specific current-path intent declarations (port of
//! `kicad_tools.router.current_paths`, issue #4980).
//!
//! A [`CurrentPathSpec`] names two stable `RefDes.pad` endpoints and a
//! continuous (optionally pulsed) current. [`resolve_current_path`] maps it
//! onto routed copper through a bounded, layer-aware centerline graph and
//! fails closed (`unresolved` / `ambiguous`) whenever that model cannot
//! vouch for a single linear branch. [`audit_current_paths`] adds the
//! uncovered / unmodeled copper inventory.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::core::geometry::{point_to_segment_distance, rotate_pad_offset, segments_intersect};
use crate::core::layers::{via_spans_layer, COPPER_LAYER_ORDER};
use crate::geometry::shapely::{self as sh, Geom};
use crate::physics::ampacity::rms_current_for_duty_cycle;
use crate::pyjson::{self, py_float_repr, py_repr_str, py_round, Json};
use crate::schema::pcb::{Footprint, Pad, Pcb, Segment};
use crate::schema::physical_identity::footprint_keys;
use crate::sexp::SExp;
use crate::validate::rules::clearance::{pad_on_layer, pad_polygon};

pub const CURRENT_PATHS_SIDECAR_BASENAME: &str = "current_paths.json";

const COORD_DECIMALS: usize = 6;
const PAD_EPS: f64 = 1e-6;

pub const THERMAL_BASIS_CONTINUOUS: &str = "continuous";
pub const THERMAL_BASIS_RMS: &str = "rms";
pub const THERMAL_BASIS_PEAK_AS_CONTINUOUS: &str = "peak-as-continuous";

pub const STATUS_RESOLVED: &str = "resolved";
pub const STATUS_UNRESOLVED: &str = "unresolved";
pub const STATUS_AMBIGUOUS: &str = "ambiguous";

/// The current a declared path's IPC-2221 width check runs at.
#[derive(Debug, Clone, PartialEq)]
pub struct ThermalDesignCurrent {
    pub current_a: f64,
    pub basis: &'static str,
    pub description: String,
    pub assumption: String,
}

/// One `RefDes.pad` terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathEndpoint {
    pub reference: String,
    pub pad: String,
}

impl PathEndpoint {
    pub fn label(&self) -> String {
        format!("{}.{}", self.reference, self.pad)
    }

    pub fn to_dict(&self) -> Json {
        crate::jobj! { "ref" => self.reference.clone(), "pad" => self.pad.clone() }
    }

    pub fn from_dict(data: &Json) -> Result<Self, String> {
        let get = |k: &str| data.get(k).and_then(Json::as_str).filter(|s| !s.is_empty());
        let Some(reference) = get("ref") else {
            return Err(format!("current-path endpoint missing 'ref': {}", data.py_repr()));
        };
        let Some(pad) = get("pad") else {
            return Err(format!("current-path endpoint missing 'pad': {}", data.py_repr()));
        };
        Ok(PathEndpoint {
            reference: reference.to_string(),
            pad: pad.to_string(),
        })
    }
}

/// A user-declared current-path intent between two pad endpoints.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CurrentPathSpec {
    pub name: String,
    pub net_name: String,
    pub source: PathEndpoint,
    pub sink: PathEndpoint,
    pub continuous_a: f64,
    pub pulsed_a: Option<f64>,
    pub duty_cycle: Option<f64>,
    pub pulse_duration_s: Option<f64>,
    pub reinforcement_eligible: bool,
    pub notes: String,
}

fn optional_number(data: &Json, key: &str, spec_name: &str) -> Result<Option<f64>, String> {
    match data.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Int(i)) => Ok(Some(*i as f64)),
        Some(Json::Float(f)) => Ok(Some(*f)),
        Some(_) => Err(format!(
            "current-path spec {} has non-numeric {}",
            py_repr_str(spec_name),
            py_repr_str(key)
        )),
    }
}

impl CurrentPathSpec {
    /// `__post_init__` validation (fail closed on incoherent declarations).
    pub fn validate(&self) -> Result<(), String> {
        let n = py_repr_str(&self.name);
        let f = py_float_repr;
        if self.continuous_a <= 0.0 {
            return Err(format!(
                "current-path spec {n}: 'continuous_a' must be positive, got {}",
                f(self.continuous_a)
            ));
        }
        if let Some(p) = self.pulsed_a {
            if p <= 0.0 {
                return Err(format!(
                    "current-path spec {n}: 'pulsed_a' must be positive, got {}",
                    f(p)
                ));
            }
            if p < self.continuous_a {
                return Err(format!(
                    "current-path spec {n}: 'pulsed_a' ({}) is below 'continuous_a' ({}); a \
                     peak below the steady current is a declaration mistake, not a relaxation",
                    f(p),
                    f(self.continuous_a)
                ));
            }
        }
        if let Some(d) = self.duty_cycle {
            if self.pulsed_a.is_none() {
                return Err(format!(
                    "current-path spec {n}: 'duty_cycle' declared without 'pulsed_a' -- a duty \
                     cycle describes a pulse that was never declared"
                ));
            }
            if !(0.0 < d && d <= 1.0) {
                return Err(format!(
                    "current-path spec {n}: 'duty_cycle' must be in (0, 1], got {}",
                    f(d)
                ));
            }
        }
        if let Some(t) = self.pulse_duration_s {
            if self.pulsed_a.is_none() {
                return Err(format!(
                    "current-path spec {n}: 'pulse_duration_s' declared without 'pulsed_a' -- a \
                     pulse duration describes a pulse that was never declared"
                ));
            }
            if t <= 0.0 {
                return Err(format!(
                    "current-path spec {n}: 'pulse_duration_s' must be positive, got {}",
                    f(t)
                ));
            }
        }
        Ok(())
    }

    /// The current the IPC-2221 continuous width check must be run at.
    pub fn thermal_design_current(&self) -> ThermalDesignCurrent {
        let g = |x: f64| crate::utils::pyfmt::format_g(x, 4);
        let Some(peak) = self.pulsed_a else {
            return ThermalDesignCurrent {
                current_a: self.continuous_a,
                basis: THERMAL_BASIS_CONTINUOUS,
                description: format!("{}A continuous", g(self.continuous_a)),
                assumption: String::new(),
            };
        };
        if let Some(d) = self.duty_cycle {
            let rms = rms_current_for_duty_cycle(peak, d, self.continuous_a).unwrap_or(peak);
            return ThermalDesignCurrent {
                current_a: rms.max(self.continuous_a),
                basis: THERMAL_BASIS_RMS,
                description: format!(
                    "{}A RMS ({}A peak at {}% duty over {}A continuous)",
                    g(rms),
                    g(peak),
                    g(d * 100.0),
                    g(self.continuous_a)
                ),
                assumption: String::new(),
            };
        }
        ThermalDesignCurrent {
            current_a: peak.max(self.continuous_a),
            basis: THERMAL_BASIS_PEAK_AS_CONTINUOUS,
            description: format!("{}A peak treated as continuous", g(peak)),
            assumption: "no 'duty_cycle' declared, so the pulsed current cannot be reduced to an \
                         RMS equivalent; the peak is sized as if it were continuous \
                         (conservative)"
                .into(),
        }
    }

    pub fn to_dict(&self) -> Json {
        let opt = |v: Option<f64>| v.map(Json::Float).unwrap_or(Json::Null);
        crate::jobj! {
            "name" => self.name.clone(),
            "net" => self.net_name.clone(),
            "source" => self.source.to_dict(),
            "sink" => self.sink.to_dict(),
            "continuous_a" => Json::Float(self.continuous_a),
            "pulsed_a" => opt(self.pulsed_a),
            "duty_cycle" => opt(self.duty_cycle),
            "pulse_duration_s" => opt(self.pulse_duration_s),
            "reinforcement_eligible" => self.reinforcement_eligible,
            "notes" => self.notes.clone(),
        }
    }

    pub fn from_dict(data: &Json) -> Result<Self, String> {
        if !matches!(data, Json::Obj(_)) {
            return Err(format!(
                "current-path entry must be an object, got {}",
                data.py_type_name()
            ));
        }
        let Some(name) = data.get("name").and_then(Json::as_str).filter(|s| !s.is_empty()) else {
            return Err(format!("current-path spec missing 'name': {}", data.py_repr()));
        };
        let net = match data.get("net") {
            Some(v) if v.truthy() => Some(v),
            _ => data.get("net_name"),
        };
        let Some(net_name) = net.and_then(Json::as_str).filter(|s| !s.is_empty()) else {
            return Err(format!("current-path spec {} missing 'net'", py_repr_str(name)));
        };
        let (Some(src @ Json::Obj(_)), Some(snk @ Json::Obj(_))) = (data.get("source"), data.get("sink"))
        else {
            return Err(format!(
                "current-path spec {} missing 'source'/'sink'",
                py_repr_str(name)
            ));
        };
        let continuous_a = match data.get("continuous_a") {
            Some(Json::Int(i)) => *i as f64,
            Some(Json::Float(f)) => *f,
            Some(Json::Bool(b)) => *b as i64 as f64,
            _ => {
                return Err(format!(
                    "current-path spec {} missing numeric 'continuous_a'",
                    py_repr_str(name)
                ))
            }
        };
        let pulsed_a = optional_number(data, "pulsed_a", name)?;
        let duty_cycle = optional_number(data, "duty_cycle", name)?;
        let pulse_duration_s = optional_number(data, "pulse_duration_s", name)?;
        let source = PathEndpoint::from_dict(src)?;
        let sink = PathEndpoint::from_dict(snk)?;
        let notes = match data.get("notes") {
            None => String::new(),
            Some(Json::Str(s)) => s.clone(),
            Some(v) => v.py_repr(),
        };
        let spec = CurrentPathSpec {
            name: name.to_string(),
            net_name: net_name.to_string(),
            source,
            sink,
            continuous_a,
            pulsed_a,
            duty_cycle,
            pulse_duration_s,
            reinforcement_eligible: data.get("reinforcement_eligible").is_some_and(Json::truthy),
            notes,
        };
        spec.validate()?;
        Ok(spec)
    }
}

pub fn parse_current_path_specs(data: &Json) -> Result<Vec<CurrentPathSpec>, String> {
    let raw = match data {
        Json::Obj(_) => data.get("paths").cloned().unwrap_or(Json::Arr(vec![])),
        Json::Arr(_) => data.clone(),
        other => {
            return Err(format!(
                "current-paths sidecar must be a list or a {{'paths': [...]}} object, got {}",
                other.py_type_name()
            ))
        }
    };
    let Json::Arr(list) = raw else {
        return Err("current-paths sidecar 'paths' key must be a list".into());
    };
    list.iter().map(CurrentPathSpec::from_dict).collect()
}

pub fn load_current_path_specs(path: &Path) -> Result<Vec<CurrentPathSpec>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let data = pyjson::loads(&text).map_err(|e| e.to_string())?;
    parse_current_path_specs(&data)
}

pub fn dump_current_path_specs(specs: &[CurrentPathSpec]) -> Json {
    crate::jobj! { "paths" => Json::Arr(specs.iter().map(CurrentPathSpec::to_dict).collect()) }
}

fn names(stem: &str) -> Vec<String> {
    if stem.is_empty() {
        vec![CURRENT_PATHS_SIDECAR_BASENAME.into()]
    } else {
        vec![
            format!("{stem}.{CURRENT_PATHS_SIDECAR_BASENAME}"),
            CURRENT_PATHS_SIDECAR_BASENAME.into(),
        ]
    }
}

/// Board dir, `output/`, `../output/` crossed with stem-keyed-then-bare
/// names, de-duplicated.
pub fn current_paths_sidecar_candidates(pcb_path: &Path) -> Vec<PathBuf> {
    let dir = pcb_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let up = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = pcb_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out: Vec<PathBuf> = Vec::new();
    for d in [dir.clone(), dir.join("output"), up.join("output")] {
        for n in names(&stem) {
            let c = d.join(n);
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    out
}

pub fn discover_current_paths_sidecar(pcb_path: &Path) -> Option<PathBuf> {
    current_paths_sidecar_candidates(pcb_path)
        .into_iter()
        .find(|c| c.is_file())
}

// ------------------------------------------------------------- resolution

/// A [`PathEndpoint`] resolved against a loaded board.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEndpoint {
    pub reference: String,
    pub pad: String,
    pub position: (f64, f64),
    pub net_number: i64,
    pub net_name: String,
}

/// Same-net copper the centerline graph cannot model (`arc` / `zone`).
#[derive(Debug, Clone, PartialEq)]
pub struct UnmodeledCopper {
    pub kind: &'static str,
    pub layer: String,
    pub location: (f64, f64),
}

/// Outcome of resolving one spec. `segments` are indices into
/// `pcb.segments()`.
#[derive(Debug, Clone, PartialEq)]
pub struct PathResolution {
    pub spec: CurrentPathSpec,
    pub status: &'static str,
    pub reason: String,
    pub source: Option<ResolvedEndpoint>,
    pub sink: Option<ResolvedEndpoint>,
    pub segments: Vec<usize>,
    pub length_mm: f64,
}

impl PathResolution {
    fn new(spec: &CurrentPathSpec, status: &'static str, reason: impl Into<String>) -> Self {
        PathResolution {
            spec: spec.clone(),
            status,
            reason: reason.into(),
            source: None,
            sink: None,
            segments: vec![],
            length_mm: 0.0,
        }
    }

    fn ends(mut self, source: Option<&ResolvedEndpoint>, sink: Option<&ResolvedEndpoint>) -> Self {
        self.source = source.cloned();
        self.sink = sink.cloned();
        self
    }

    pub fn ok(&self) -> bool {
        self.status == STATUS_RESOLVED
    }
}

fn net_child_matches(node: &SExp, net_number: Option<i64>, net_name: &str) -> bool {
    let Some(net) = node.get("net") else {
        return false;
    };
    if net.text_at(0).as_deref() == Some(net_name) {
        return true;
    }
    let as_int = match net.value_at(0) {
        Some(v) => v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())),
        None => None,
    };
    net_number.is_some() && as_int == net_number
}

fn net_number_of(pcb: &Pcb, net_name: &str) -> Option<i64> {
    pcb.get_net_by_name(net_name).map(|n| n.number)
}

/// Inventory same-net routed arcs and non-keepout zones.
pub fn unmodeled_copper(pcb: &Pcb, net_name: &str) -> Vec<UnmodeledCopper> {
    let num = net_number_of(pcb, net_name);
    let mut found = Vec::new();
    for arc in pcb.sexp().find_all("arc") {
        if !net_child_matches(arc, num, net_name) {
            continue;
        }
        let layer = arc
            .get("layer")
            .and_then(|l| l.text_at(0))
            .unwrap_or_default();
        let location = match arc.get("start") {
            Some(s) => (s.float_at(0).unwrap_or(0.0), s.float_at(1).unwrap_or(0.0)),
            None => (0.0, 0.0),
        };
        found.push(UnmodeledCopper {
            kind: "arc",
            layer,
            location,
        });
    }
    for zone in pcb.zones() {
        let same = zone.net_name == net_name || num.is_some_and(|n| zone.net_number == n);
        if zone.keepout.is_some() || !same {
            continue;
        }
        let layer = zone.layers.first().cloned().unwrap_or_else(|| zone.layer.clone());
        let location = zone.polygon.first().copied().unwrap_or((0.0, 0.0));
        found.push(UnmodeledCopper {
            kind: "zone",
            layer,
            location,
        });
    }
    found
}

fn node_key(p: (f64, f64)) -> (f64, f64) {
    // `+ 0.0` folds -0.0 into 0.0 (Python treats them as one dict key).
    (
        py_round(p.0, COORD_DECIMALS) + 0.0,
        py_round(p.1, COORD_DECIMALS) + 0.0,
    )
}

fn seg_length(s: &Segment) -> f64 {
    crate::utils::pymath::hypot(s.end.0 - s.start.0, s.end.1 - s.start.1)
}

fn net_segments(pcb: &Pcb, net_name: &str) -> Vec<usize> {
    let num = net_number_of(pcb, net_name);
    pcb.segments()
        .iter()
        .enumerate()
        .filter(|(_, s)| s.net_name == net_name || num.is_some_and(|n| s.net_number == n))
        .map(|(i, _)| i)
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
struct Node {
    x: f64,
    y: f64,
    layer: String,
}

impl Eq for Node {}

impl Hash for Node {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.x.to_bits().hash(h);
        self.y.to_bits().hash(h);
        self.layer.hash(h);
    }
}

impl Node {
    fn xy(&self) -> (f64, f64) {
        (self.x, self.y)
    }
}

fn copper_node(p: (f64, f64), layer: &str) -> Node {
    let (x, y) = node_key(p);
    Node {
        x,
        y,
        layer: layer.to_string(),
    }
}

/// Edge ids index `edge_seg` (None = via barrel copper).
#[derive(Default)]
struct CopperGraph {
    adjacency: HashMap<Node, Vec<(Node, usize)>>,
    pads: HashMap<(String, String, usize), Node>,
    internal: HashMap<Node, Vec<usize>>,
    edge_seg: Vec<Option<usize>>,
}

impl CopperGraph {
    fn adj(&self, n: &Node) -> &[(Node, usize)] {
        self.adjacency.get(n).map(Vec::as_slice).unwrap_or(&[])
    }
}

fn via_on_net(pcb: &Pcb, v: &crate::schema::pcb::Via, net_name: &str) -> bool {
    v.net_name == net_name || net_number_of(pcb, net_name).is_some_and(|n| v.net_number == n)
}

fn copper_layer_names(pcb: &Pcb) -> Vec<String> {
    pcb.copper_layers().iter().map(|l| l.name.clone()).collect()
}

fn build_graph(segs: &[usize], pcb: &Pcb, net_name: &str) -> CopperGraph {
    let all = pcb.segments();
    let layers = copper_layer_names(pcb);
    let vias: Vec<&crate::schema::pcb::Via> = pcb
        .vias()
        .iter()
        .filter(|v| {
            via_on_net(pcb, v, net_name)
                && v.layers.len() == 2
                && v.layers[0] != v.layers[1]
                && v.layers.iter().all(|l| layers.contains(l))
        })
        .collect();
    // Ordered, de-duplicated contact points per layer.
    let mut contacts: Vec<(String, Vec<(f64, f64)>, HashSet<(u64, u64)>)> = layers
        .iter()
        .map(|l| (l.clone(), Vec::new(), HashSet::new()))
        .collect();
    let add = |contacts: &mut Vec<(String, Vec<(f64, f64)>, HashSet<(u64, u64)>)>,
                   layer: &str,
                   p: (f64, f64)| {
        let i = match contacts.iter().position(|c| c.0 == layer) {
            Some(i) => i,
            None => {
                contacts.push((layer.to_string(), Vec::new(), HashSet::new()));
                contacts.len() - 1
            }
        };
        if contacts[i].2.insert((p.0.to_bits(), p.1.to_bits())) {
            contacts[i].1.push(p);
        }
    };
    for &si in segs {
        let s = &all[si];
        add(&mut contacts, &s.layer, node_key(s.start));
        add(&mut contacts, &s.layer, node_key(s.end));
    }
    for (k, &ia) in segs.iter().enumerate() {
        let a = &all[ia];
        for &ib in &segs[k + 1..] {
            let b = &all[ib];
            if a.layer != b.layer
                || !segments_intersect(
                    a.start.0, a.start.1, a.end.0, a.end.1, b.start.0, b.start.1, b.end.0, b.end.1,
                )
            {
                continue;
            }
            let (dx, dy) = (a.end.0 - a.start.0, a.end.1 - a.start.1);
            let (ex, ey) = (b.end.0 - b.start.0, b.end.1 - b.start.1);
            let (ox, oy) = (b.start.0 - a.start.0, b.start.1 - a.start.1);
            let t = (ox * ey - oy * ex) / (dx * ey - dy * ex);
            add(
                &mut contacts,
                &a.layer,
                node_key((a.start.0 + t * dx, a.start.1 + t * dy)),
            );
        }
    }
    for v in &vias {
        for l in &layers {
            if via_spans_layer(&v.layers, l) {
                add(&mut contacts, l, node_key(v.position));
            }
        }
    }

    let mut g = CopperGraph::default();
    let mut edges: Vec<(Node, Node, usize)> = Vec::new();
    for &si in segs {
        let s = &all[si];
        let Some(c) = contacts.iter().find(|c| c.0 == s.layer) else {
            continue;
        };
        let mut pts: Vec<(f64, f64)> = c
            .1
            .iter()
            .copied()
            .filter(|p| point_to_segment_distance(p.0, p.1, s.start.0, s.start.1, s.end.0, s.end.1) <= PAD_EPS)
            .collect();
        pts.sort_by(|p, q| {
            crate::utils::pymath::dist(*p, s.start).total_cmp(&crate::utils::pymath::dist(*q, s.start))
        });
        for w in pts.windows(2) {
            let id = g.edge_seg.len();
            g.edge_seg.push(Some(si));
            edges.push((copper_node(w[0], &s.layer), copper_node(w[1], &s.layer), id));
        }
    }
    for v in &vias {
        let nodes: Vec<Node> = COPPER_LAYER_ORDER
            .iter()
            .filter(|l| layers.iter().any(|x| x == *l) && via_spans_layer(&v.layers, l))
            .map(|l| copper_node(v.position, l))
            .collect();
        for w in nodes.windows(2) {
            let id = g.edge_seg.len();
            g.edge_seg.push(None);
            edges.push((w[0].clone(), w[1].clone(), id));
        }
    }

    // Union-find over graph nodes, in first-seen order.
    let mut order: Vec<Node> = Vec::new();
    let mut index: HashMap<Node, usize> = HashMap::new();
    for (a, b, _) in &edges {
        for n in [a, b] {
            if !index.contains_key(n) {
                index.insert(n.clone(), order.len());
                order.push(n.clone());
            }
        }
    }
    let mut parent: Vec<usize> = (0..order.len()).collect();
    fn root(parent: &mut [usize], mut n: usize) -> usize {
        while parent[n] != n {
            parent[n] = parent[parent[n]];
            n = parent[n];
        }
        n
    }

    let fps = pcb.footprints();
    let keys = footprint_keys(
        &fps.iter()
            .map(|f| (f.reference.as_str(), f.uuid.as_str()))
            .collect::<Vec<_>>(),
    );
    let mut pad_contacts: Vec<((String, String, usize), Vec<usize>)> = Vec::new();
    let mut internal_edges: HashSet<usize> = HashSet::new();
    let mut wholly_internal: HashSet<usize> = HashSet::new();
    for (cid, fp) in keys.iter().zip(fps) {
        let mut occ: HashMap<&str, usize> = HashMap::new();
        for pad in &fp.pads {
            let o = *occ.get(pad.number.as_str()).unwrap_or(&0);
            occ.insert(pad.number.as_str(), o + 1);
            if pad.net_name != net_name || pad.pad_type == "np_thru_hole" {
                continue;
            }
            let star = pad.layers.iter().any(|l| l == "*.Cu");
            let nodes: Vec<usize> = (0..order.len())
                .filter(|&i| {
                    let n = &order[i];
                    (star || pad.layers.contains(&n.layer)) && physical_pad_covers(fp, pad, n.xy())
                })
                .collect();
            let groups: Vec<Vec<usize>> = if pad.pad_type == "thru_hole" {
                vec![nodes.clone()]
            } else {
                layers
                    .iter()
                    .map(|l| nodes.iter().copied().filter(|&i| order[i].layer == *l).collect())
                    .collect()
            };
            for group in groups {
                if group.is_empty() {
                    continue;
                }
                let members: HashSet<usize> = group.iter().copied().collect();
                for (a, b, e) in &edges {
                    if members.contains(&index[a]) && members.contains(&index[b]) {
                        internal_edges.insert(*e);
                    }
                }
                for &si in segs {
                    let s = &all[si];
                    let a = index.get(&copper_node(s.start, &s.layer));
                    let b = index.get(&copper_node(s.end, &s.layer));
                    if let (Some(a), Some(b)) = (a, b) {
                        if members.contains(a) && members.contains(b) {
                            wholly_internal.insert(si);
                        }
                    }
                }
                let hub = root(&mut parent, group[0]);
                for &n in &group[1..] {
                    let r = root(&mut parent, n);
                    parent[r] = hub;
                }
            }
            pad_contacts.push(((cid.clone(), pad.number.clone(), o), nodes));
        }
    }

    for (key, nodes) in pad_contacts {
        let hubs: HashSet<usize> = nodes.iter().map(|&n| root(&mut parent, n)).collect();
        if hubs.len() == 1 {
            let h = *hubs.iter().next().unwrap();
            g.pads.insert(key, order[h].clone());
        }
    }
    for (a, b, e) in &edges {
        let ra = order[root(&mut parent, index[a])].clone();
        let rb = order[root(&mut parent, index[b])].clone();
        g.adjacency.entry(ra.clone()).or_default();
        g.adjacency.entry(rb.clone()).or_default();
        if internal_edges.contains(e) && a != b {
            if let Some(si) = g.edge_seg[*e] {
                if wholly_internal.contains(&si) {
                    g.internal.entry(ra).or_default().push(si);
                }
            }
            continue;
        }
        g.adjacency.get_mut(&ra).unwrap().push((rb.clone(), *e));
        g.adjacency.get_mut(&rb).unwrap().push((ra, *e));
    }
    g
}

/// Ordered de-duplication keeping each key's first position
/// (`list({id(s): s for s in xs}.values())`).
fn dedupe(xs: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let mut seen = HashSet::new();
    xs.into_iter().filter(|x| seen.insert(*x)).collect()
}

fn bfs_path(g: &CopperGraph, start: &Node, goal: &Node) -> Option<Vec<usize>> {
    let mut visited: HashSet<Node> = HashSet::from([start.clone()]);
    let mut queue = VecDeque::from([start.clone()]);
    let mut parent: HashMap<Node, (Node, usize)> = HashMap::new();
    while let Some(cur) = queue.pop_front() {
        if &cur == goal {
            break;
        }
        for (nxt, e) in g.adj(&cur) {
            if visited.insert(nxt.clone()) {
                parent.insert(nxt.clone(), (cur.clone(), *e));
                queue.push_back(nxt.clone());
            }
        }
    }
    if !visited.contains(goal) {
        return None;
    }
    let mut evidence: Vec<usize> = g.internal.get(goal).cloned().unwrap_or_default();
    let mut node = goal.clone();
    while &node != start {
        let (prev, e) = parent[&node].clone();
        if let Some(s) = g.edge_seg[e] {
            evidence.push(s);
        }
        evidence.extend(g.internal.get(&prev).cloned().unwrap_or_default());
        node = prev;
    }
    Some(dedupe(evidence.into_iter().rev()))
}

struct ViaArray {
    nodes: HashSet<Node>,
    edges: HashSet<usize>,
    members: Vec<usize>,
}

type Exit = (Node, Node, usize);

/// Per-footprint, per-pad `padstack` presence (raw pad nodes zipped with the
/// parsed pads in board order).
fn pad_has_padstack(pcb: &Pcb) -> Vec<Vec<bool>> {
    let raw: Vec<&SExp> = pcb
        .sexp()
        .children
        .iter()
        .filter(|c| c.has_tag("footprint") || c.has_tag("module"))
        .collect();
    pcb.footprints()
        .iter()
        .enumerate()
        .map(|(i, fp)| {
            let pads: Vec<bool> = raw
                .get(i)
                .map(|n| {
                    n.children
                        .iter()
                        .filter(|c| c.has_tag("pad"))
                        .map(|p| p.get("padstack").is_some())
                        .collect()
                })
                .unwrap_or_default();
            if pads.len() == fp.pads.len() {
                pads
            } else {
                vec![false; fp.pads.len()]
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn array_contacts_modeled(
    pcb: &Pcb,
    net_name: &str,
    fp: &Footprint,
    pad: &Pad,
    members: &[usize],
    far_nodes: &[Node],
    exits: &[Exit],
    g: &CopperGraph,
) -> bool {
    let num = net_number_of(pcb, net_name);
    let all = pcb.segments();
    let same_pad = |p: &Pad| p.net_name == net_name || num.is_some_and(|n| p.net_number == n);
    let same_via = |v: &crate::schema::pcb::Via| via_on_net(pcb, v, net_name);

    for arc in pcb.sexp().find_all("arc") {
        if net_child_matches(arc, num, net_name) {
            return false;
        }
    }
    let stacks = pad_has_padstack(pcb);
    for (fi, f) in pcb.footprints().iter().enumerate() {
        for (pi, c) in f.pads.iter().enumerate() {
            if same_pad(c) && stacks[fi][pi] {
                return false;
            }
        }
    }
    for raw in pcb.sexp().find_all("via") {
        if net_child_matches(raw, num, net_name) && raw.get("padstack").is_some() {
            return false;
        }
    }

    let scale = 1.0 / (std::f64::consts::PI / 64.0).cos();
    let seg_buf = |a: (f64, f64), b: (f64, f64), r: f64| sh::segment_buffer_q(a, b, r * scale, 16);
    let pt_buf = |p: (f64, f64), r: f64| sh::point_buffer_q(p, r * scale, 16);
    let pad_envelope = |c: &Pad, f: &Footprint| -> Option<Geom> {
        let poly = pad_polygon(c, f)?;
        if c.shape == "rect" {
            return Some(poly);
        }
        let error = c.size.0.max(c.size.1) * (1.0 - 1.0 / scale);
        Some(sh::buffer_polygon(&poly, error * scale))
    };

    let trunk = *members.last().unwrap();
    let ts = all[trunk].start;
    let mut ordered: Vec<&Node> = far_nodes.iter().collect();
    ordered.sort_by(|a, b| {
        crate::utils::pymath::dist(a.xy(), ts).total_cmp(&crate::utils::pymath::dist(b.xy(), ts))
    });
    let mut pieces: Vec<(String, Geom, f64)> = Vec::new();
    for &m in members {
        let s = &all[m];
        let (a, b) = if m == trunk {
            (ordered[0].xy(), ordered[ordered.len() - 1].xy())
        } else {
            (s.start, s.end)
        };
        pieces.push((s.layer.clone(), seg_buf(a, b, s.width / 2.0), s.width / 2.0));
    }
    let Some(source) = pad_envelope(pad, fp) else {
        return false;
    };
    pieces.push((all[members[0]].layer.clone(), source, 0.0));
    let positions: HashSet<(u64, u64)> = far_nodes
        .iter()
        .map(|n| (n.x.to_bits(), n.y.to_bits()))
        .collect();
    let member_vias: Vec<usize> = pcb
        .vias()
        .iter()
        .enumerate()
        .filter(|(_, v)| {
            let k = node_key(v.position);
            positions.contains(&(k.0.to_bits(), k.1.to_bits()))
        })
        .map(|(i, _)| i)
        .collect();
    for &vi in &member_vias {
        let v = &pcb.vias()[vi];
        for l in pcb.copper_layers() {
            if via_spans_layer(&v.layers, &l.name) {
                pieces.push((l.name.clone(), pt_buf(v.position, v.size / 2.0), v.size / 2.0));
            }
        }
    }
    for si in net_segments(pcb, net_name) {
        if members.contains(&si) {
            continue;
        }
        let s = &all[si];
        let copper = seg_buf(s.start, s.end, s.width / 2.0);
        for (layer, region, radius) in &pieces {
            if *layer != s.layer {
                continue;
            }
            let mut overlap = sh::intersection(&copper, region);
            for (node, _, e) in exits {
                if g.edge_seg[*e] == Some(si) && node.layer == *layer {
                    if sh::covers_point(region, node.xy()) {
                        overlap = Geom::Empty;
                    } else {
                        overlap = sh::difference(
                            &overlap,
                            &sh::point_buffer(
                                node.xy(),
                                (radius + s.width / 2.0) * scale * scale + PAD_EPS,
                            ),
                        );
                    }
                }
            }
            if !overlap.is_empty() {
                return false;
            }
        }
    }
    for f in pcb.footprints() {
        for c in &f.pads {
            if std::ptr::eq(c, pad) || !same_pad(c) || c.pad_type == "np_thru_hole" {
                continue;
            }
            let Some(copper) = pad_envelope(c, f) else {
                return false;
            };
            if pieces
                .iter()
                .any(|(l, region, _)| pad_on_layer(c, l) && sh::intersects(&copper, region))
            {
                return false;
            }
        }
    }
    for (vi, v) in pcb.vias().iter().enumerate() {
        if member_vias.contains(&vi) || !same_via(v) {
            continue;
        }
        let copper = pt_buf(v.position, v.size / 2.0);
        if pieces
            .iter()
            .any(|(l, region, _)| via_spans_layer(&v.layers, l) && sh::intersects(&copper, region))
        {
            return false;
        }
    }
    for z in pcb.zones() {
        let same = z.net_name == net_name || num.is_some_and(|n| z.net_number == n);
        if !same || z.keepout.is_some() {
            continue;
        }
        if z.polygon.len() < 3 {
            return false;
        }
        let xs = z.polygon.iter().map(|p| p.0);
        let ys = z.polygon.iter().map(|p| p.1);
        let envelope = sh::box_poly(
            xs.clone().fold(f64::INFINITY, f64::min),
            ys.clone().fold(f64::INFINITY, f64::min),
            xs.fold(f64::NEG_INFINITY, f64::max),
            ys.fold(f64::NEG_INFINITY, f64::max),
        );
        let zl: Vec<String> = if z.layers.is_empty() {
            vec![z.layer.clone()]
        } else {
            z.layers.clone()
        };
        let star = zl.iter().any(|l| l == "*.Cu");
        if pieces
            .iter()
            .any(|(l, region, _)| (star || zl.contains(l)) && sh::intersects(&envelope, region))
        {
            return false;
        }
    }
    true
}

fn proved_load_exits(g: &CopperGraph, array_nodes: &HashSet<Node>, exits: &[Exit]) -> bool {
    if exits.is_empty() {
        return false;
    }
    let terminals: HashSet<&Node> = g.pads.values().filter(|n| !array_nodes.contains(*n)).collect();
    let mut visited: HashSet<Node> = HashSet::new();
    for (_, first, incoming) in exits {
        let mut pending: Vec<(Node, usize)> = vec![(first.clone(), *incoming)];
        while let Some((node, previous)) = pending.pop() {
            if array_nodes.contains(&node) || visited.contains(&node) {
                return false;
            }
            visited.insert(node.clone());
            let onward: Vec<(Node, usize)> = g
                .adj(&node)
                .iter()
                .filter(|(_, e)| *e != previous)
                .cloned()
                .collect();
            if onward.is_empty() && !terminals.contains(&node) {
                return false;
            }
            pending.extend(onward);
        }
    }
    true
}

fn endpoint_via_array(
    g: &CopperGraph,
    pcb: &Pcb,
    endpoint: &PathEndpoint,
    net_name: &str,
) -> Option<ViaArray> {
    let key = (endpoint.reference.clone(), endpoint.pad.clone(), 0usize);
    let hub = g.pads.get(&key)?.clone();
    let fp = pcb.get_footprint(&endpoint.reference)?;
    let pad = fp.pads.iter().find(|p| p.number == endpoint.pad)?;
    if pad.pad_type != "smd" || !(hub.layer == "F.Cu" || hub.layer == "B.Cu") {
        return None;
    }
    if g.pads.iter().any(|(k, n)| *n == hub && *k != key) {
        return None;
    }
    let far_layer = if hub.layer == "F.Cu" { "B.Cu" } else { "F.Cu" };
    let arms = g.adj(&hub);
    if arms.len() < 2 {
        return None;
    }
    let all = pcb.segments();
    let bound = crate::utils::pymath::hypot(pad.size.0, pad.size.1);
    let mut nodes: HashSet<Node> = HashSet::from([hub.clone()]);
    let mut edges: HashSet<usize> = HashSet::new();
    let mut members: Vec<usize> = g.internal.get(&hub).cloned().unwrap_or_default();
    let mut far_nodes: Vec<Node> = Vec::new();
    let mut direction: Option<(f64, f64)> = None;
    for (top, e) in arms {
        let si = g.edge_seg[*e]?;
        let seg = &all[si];
        if seg.layer != hub.layer || nodes.contains(top) || g.adj(top).len() != 2 {
            return None;
        }
        let anchors: Vec<(f64, f64)> = [seg.start, seg.end]
            .into_iter()
            .filter(|p| pad_covers(pcb, &endpoint.reference, &endpoint.pad, *p))
            .collect();
        if anchors.len() != 1 {
            return None;
        }
        let anchor = anchors[0];
        let tip = if anchor == seg.start { seg.end } else { seg.start };
        if node_key(tip) != top.xy() || seg_length(seg) > bound + PAD_EPS {
            return None;
        }
        let delta = (tip.0 - anchor.0, tip.1 - anchor.1);
        match direction {
            None => direction = Some(delta),
            Some(d) => {
                if (d.0 * delta.1 - d.1 * delta.0).abs() > PAD_EPS || d.0 * delta.0 + d.1 * delta.1 <= 0.0 {
                    return None;
                }
            }
        }
        let matching: Vec<&crate::schema::pcb::Via> = pcb
            .vias()
            .iter()
            .filter(|v| node_key(v.position) == top.xy())
            .collect();
        if matching.len() != 1 {
            return None;
        }
        let via = matching[0];
        let vl: HashSet<&str> = via.layers.iter().map(String::as_str).collect();
        if vl != HashSet::from(["F.Cu", "B.Cu"]) || !via_on_net(pcb, via, net_name) {
            return None;
        }
        members.push(si);
        edges.insert(*e);
        let (mut previous, mut current) = (*e, top.clone());
        loop {
            if nodes.contains(&current) {
                return None;
            }
            nodes.insert(current.clone());
            let onward: Vec<&(Node, usize)> =
                g.adj(&current).iter().filter(|(_, x)| *x != previous).collect();
            if current.layer == far_layer {
                far_nodes.push(current.clone());
                break;
            }
            if onward.len() != 1 || g.edge_seg[onward[0].1].is_some() {
                return None;
            }
            let (nxt, barrel) = onward[0].clone();
            if nxt.xy() != top.xy() {
                return None;
            }
            edges.insert(barrel);
            previous = barrel;
            current = nxt;
        }
    }
    let mut maxd = f64::NEG_INFINITY;
    for a in &far_nodes {
        for b in &far_nodes {
            maxd = maxd.max(crate::utils::pymath::dist(a.xy(), b.xy()));
        }
    }
    if maxd > bound + PAD_EPS {
        return None;
    }
    let candidates = dedupe(g.adj(&far_nodes[0]).iter().filter_map(|(_, e)| {
        let si = g.edge_seg[*e]?;
        let s = &all[si];
        (s.layer == far_layer
            && far_nodes.iter().all(|n| {
                point_to_segment_distance(n.x, n.y, s.start.0, s.start.1, s.end.0, s.end.1) <= PAD_EPS
            }))
        .then_some(si)
    }));
    if candidates.len() != 1 {
        return None;
    }
    let trunk = candidates[0];
    let ts = all[trunk].start;
    let mut ordered = far_nodes.clone();
    ordered.sort_by(|a, b| {
        crate::utils::pymath::dist(a.xy(), ts).total_cmp(&crate::utils::pymath::dist(b.xy(), ts))
    });
    let (first, last) = (ordered[0].clone(), ordered[ordered.len() - 1].clone());
    for node in g.adjacency.keys() {
        if node.layer == far_layer
            && point_to_segment_distance(node.x, node.y, first.x, first.y, last.x, last.y) <= PAD_EPS
        {
            nodes.insert(node.clone());
        }
    }
    if g.pads.values().any(|n| nodes.contains(n) && *n != hub) {
        return None;
    }
    let mut trunk_edges: HashSet<usize> = HashSet::new();
    for node in &nodes {
        for (other, e) in g.adj(node) {
            if nodes.contains(other) && !edges.contains(e) {
                if g.edge_seg[*e] != Some(trunk) {
                    return None;
                }
                trunk_edges.insert(*e);
            }
        }
    }
    let mut reached: HashSet<Node> = HashSet::from([first.clone()]);
    let mut pending = vec![first.clone()];
    while let Some(n) = pending.pop() {
        for (other, e) in g.adj(&n) {
            if trunk_edges.contains(e) && reached.insert(other.clone()) {
                pending.push(other.clone());
            }
        }
    }
    if !far_nodes.iter().all(|n| reached.contains(n)) {
        return None;
    }
    edges.extend(trunk_edges);
    let exits: Vec<Exit> = nodes
        .iter()
        .flat_map(|n| {
            g.adj(n)
                .iter()
                .filter(|(o, _)| !nodes.contains(o))
                .map(move |(o, e)| (n.clone(), o.clone(), *e))
        })
        .collect();
    if !proved_load_exits(g, &nodes, &exits) {
        return None;
    }
    members.push(trunk);
    if !array_contacts_modeled(pcb, net_name, fp, pad, &members, &far_nodes, &exits, g) {
        return None;
    }
    Some(ViaArray {
        nodes,
        edges,
        members: dedupe(members),
    })
}

fn candidate_via_array_leg_count(g: &CopperGraph, pcb: &Pcb, hub: &Node, pad: &Pad, net_name: &str) -> usize {
    let bound = crate::utils::pymath::hypot(pad.size.0, pad.size.1);
    let all = pcb.segments();
    let mut count = 0;
    for (top, e) in g.adj(hub) {
        let Some(si) = g.edge_seg[*e] else {
            continue;
        };
        let seg = &all[si];
        if seg.layer != hub.layer || seg_length(seg) > bound + PAD_EPS {
            continue;
        }
        let matching: Vec<&crate::schema::pcb::Via> = pcb
            .vias()
            .iter()
            .filter(|v| node_key(v.position) == top.xy())
            .collect();
        if matching.len() != 1 {
            continue;
        }
        let via = matching[0];
        if via.layers.len() < 2 || !via_on_net(pcb, via, net_name) {
            continue;
        }
        count += 1;
    }
    count
}

fn component_has_cycle(g: &CopperGraph, start: &Node, arrays: &[ViaArray]) -> bool {
    let mut roots: HashMap<&Node, usize> = HashMap::new();
    for (i, a) in arrays.iter().enumerate() {
        for n in &a.nodes {
            roots.insert(n, i);
        }
    }
    // Root identity: array index for contracted nodes, else the node itself.
    #[derive(PartialEq, Eq, Hash)]
    enum Root<'a> {
        Array(usize),
        Node(&'a Node),
    }
    let mut visited: HashSet<Node> = HashSet::from([start.clone()]);
    let mut visited_roots: HashSet<Root> = HashSet::new();
    visited_roots.insert(match roots.get(start) {
        Some(i) => Root::Array(*i),
        None => Root::Node(start),
    });
    let contracted: HashSet<usize> = arrays.iter().flat_map(|a| a.edges.iter().copied()).collect();
    let mut seen_edges: HashSet<usize> = HashSet::new();
    let mut stack: Vec<&Node> = vec![start];
    while let Some(cur) = stack.pop() {
        for (nxt, e) in g.adj(cur) {
            if !contracted.contains(e) {
                seen_edges.insert(*e);
            }
            if visited.insert(nxt.clone()) {
                visited_roots.insert(match roots.get(nxt) {
                    Some(i) => Root::Array(*i),
                    None => Root::Node(nxt),
                });
                stack.push(nxt);
            }
        }
    }
    seen_edges.len() as i64 > visited_roots.len() as i64 - 1
}

fn pad_covers(pcb: &Pcb, reference: &str, pad_number: &str, p: (f64, f64)) -> bool {
    let fps: Vec<&Footprint> = pcb.footprints().iter().filter(|f| f.reference == reference).collect();
    if fps.len() != 1 {
        return false;
    }
    let pads: Vec<&Pad> = fps[0].pads.iter().filter(|x| x.number == pad_number).collect();
    pads.len() == 1 && physical_pad_covers(fps[0], pads[0], p)
}

/// Exact extent test in the pad's own rotated frame.
pub fn physical_pad_covers(fp: &Footprint, pad: &Pad, p: (f64, f64)) -> bool {
    let off = rotate_pad_offset(pad.position.0, pad.position.1, fp.rotation);
    let center = (fp.position.0 + off.0, fp.position.1 + off.1);
    let (hw, hh) = (pad.size.0 / 2.0, pad.size.1 / 2.0);
    if hw <= 0.0 || hh <= 0.0 {
        return false;
    }
    let (dx, dy) = (p.0 - center.0, p.1 - center.1);
    let a = (-pad.rotation).to_radians();
    let (c, s) = (a.cos(), a.sin());
    let lx = dx * c - dy * s;
    let ly = dx * s + dy * c;
    let hyp = crate::utils::pymath::hypot;
    match pad.shape.as_str() {
        "circle" => hyp(lx, ly) <= hw.min(hh) + PAD_EPS,
        "oval" => {
            let r = hw.min(hh);
            let ex = (lx.abs() - (hw - r)).max(0.0);
            let ey = (ly.abs() - (hh - r)).max(0.0);
            hyp(ex, ey) <= r + PAD_EPS
        }
        "rect" => lx.abs() <= hw + PAD_EPS && ly.abs() <= hh + PAD_EPS,
        "roundrect" => {
            let ratio = pad.roundrect_rratio;
            if !(0.0..=0.5).contains(&ratio) {
                return false;
            }
            let r = 2.0 * hw.min(hh) * ratio;
            let ex = (lx.abs() - (hw - r)).max(0.0);
            let ey = (ly.abs() - (hh - r)).max(0.0);
            hyp(ex, ey) <= r + PAD_EPS
        }
        _ => false,
    }
}

fn resolve_endpoint(pcb: &Pcb, ep: &PathEndpoint, expected_net: &str) -> Result<ResolvedEndpoint, String> {
    let fps: Vec<&Footprint> = pcb.footprints().iter().filter(|f| f.reference == ep.reference).collect();
    if fps.len() > 1 {
        return Err(format!(
            "component {} is ambiguous: multiple physical footprints",
            py_repr_str(&ep.reference)
        ));
    }
    let Some(fp) = fps.first() else {
        return Err(format!("component {} not found on board", py_repr_str(&ep.reference)));
    };
    let matches: Vec<&Pad> = fp.pads.iter().filter(|p| p.number == ep.pad).collect();
    if matches.len() > 1 {
        return Err(format!(
            "pad {} has multiple physical occurrences; contact is unsupported",
            ep.label()
        ));
    }
    let Some(pad) = matches.first() else {
        return Err(format!(
            "pad {} not found on component {}",
            ep.label(),
            py_repr_str(&ep.reference)
        ));
    };
    if pad.net_name != expected_net {
        return Err(format!(
            "pad {} resolves to net {}, expected {}",
            ep.label(),
            py_repr_str(&pad.net_name),
            py_repr_str(expected_net)
        ));
    }
    let Some(pos) = pcb.get_pad_position(&ep.reference, &ep.pad) else {
        return Err(format!("could not compute board position for pad {}", ep.label()));
    };
    Ok(ResolvedEndpoint {
        reference: ep.reference.clone(),
        pad: ep.pad.clone(),
        position: pos,
        net_number: pad.net_number,
        net_name: pad.net_name.clone(),
    })
}

/// Resolve one declared current-path spec against a loaded board.
pub fn resolve_current_path(pcb: &Pcb, spec: &CurrentPathSpec) -> PathResolution {
    let source = match resolve_endpoint(pcb, &spec.source, &spec.net_name) {
        Ok(s) => s,
        Err(e) => return PathResolution::new(spec, STATUS_UNRESOLVED, e),
    };
    let sink = match resolve_endpoint(pcb, &spec.sink, &spec.net_name) {
        Ok(s) => s,
        Err(e) => return PathResolution::new(spec, STATUS_UNRESOLVED, e).ends(Some(&source), None),
    };
    let both = |r: PathResolution| r.ends(Some(&source), Some(&sink));
    if source.net_number != sink.net_number {
        return both(PathResolution::new(
            spec,
            STATUS_UNRESOLVED,
            format!(
                "source {} and sink {} resolve to different nets ({} vs {})",
                spec.source.label(),
                spec.sink.label(),
                py_repr_str(&source.net_name),
                py_repr_str(&sink.net_name)
            ),
        ));
    }
    let segs = net_segments(pcb, &spec.net_name);
    if segs.is_empty() {
        return both(PathResolution::new(
            spec,
            STATUS_UNRESOLVED,
            format!("net {} has no routed copper", py_repr_str(&spec.net_name)),
        ));
    }
    let mut unsupported: Option<String> = None;
    for fp in pcb.footprints() {
        let mut seen: HashSet<&str> = HashSet::new();
        for pad in &fp.pads {
            if pad.net_name != spec.net_name || pad.pad_type == "np_thru_hole" {
                continue;
            }
            if seen.contains(pad.number.as_str())
                || !["rect", "roundrect", "circle", "oval"].contains(&pad.shape.as_str())
            {
                unsupported = Some(format!(
                    "unsupported physical pad contact at {}.{}",
                    fp.reference, pad.number
                ));
            }
            seen.insert(pad.number.as_str());
        }
    }
    let layers = copper_layer_names(pcb);
    let all = pcb.segments();
    if segs.iter().any(|&i| !layers.contains(&all[i].layer)) {
        unsupported = Some("routed copper uses a layer absent from the board stackup".into());
    }
    if let Some(u) = unsupported {
        return both(PathResolution::new(spec, STATUS_UNRESOLVED, u));
    }

    let g = build_graph(&segs, pcb, &spec.net_name);
    if spec.source == spec.sink {
        return both(PathResolution::new(spec, STATUS_RESOLVED, ""));
    }
    let start = g
        .pads
        .get(&(source.reference.clone(), source.pad.clone(), 0))
        .cloned();
    let goal = g.pads.get(&(sink.reference.clone(), sink.pad.clone(), 0)).cloned();
    let Some(start) = start else {
        return both(PathResolution::new(
            spec,
            STATUS_UNRESOLVED,
            format!("source pad {} has no routed copper touching it", spec.source.label()),
        ));
    };
    let Some(goal) = goal else {
        return both(PathResolution::new(
            spec,
            STATUS_UNRESOLVED,
            format!("sink pad {} has no routed copper touching it", spec.sink.label()),
        ));
    };
    let Some(path) = bfs_path(&g, &start, &goal) else {
        return both(PathResolution::new(
            spec,
            STATUS_UNRESOLVED,
            "no continuous copper path found between declared endpoints",
        ));
    };

    let mut arrays: Vec<ViaArray> = Vec::new();
    for ep in [&spec.source, &spec.sink] {
        match endpoint_via_array(&g, pcb, ep, &spec.net_name) {
            Some(a) => {
                if !arrays.iter().any(|p| !a.nodes.is_disjoint(&p.nodes)) {
                    arrays.push(a);
                }
            }
            None => {
                let hub = g.pads.get(&(ep.reference.clone(), ep.pad.clone(), 0));
                let ep_pad = pcb
                    .get_footprint(&ep.reference)
                    .and_then(|f| f.pads.iter().find(|p| p.number == ep.pad));
                let arms: &[(Node, usize)] = hub.map(|h| g.adj(h)).unwrap_or(&[]);
                if let (Some(hub), Some(ep_pad)) = (hub, ep_pad) {
                    let via_arm = arms.iter().any(|(n, e)| {
                        g.edge_seg[*e].is_some()
                            && pcb.vias().iter().any(|v| node_key(v.position) == n.xy())
                    });
                    if ep_pad.pad_type == "smd"
                        && arms.len() >= 2
                        && via_arm
                        && (candidate_via_array_leg_count(&g, pcb, hub, ep_pad, &spec.net_name) >= 2
                            || !proved_load_exits(
                                &g,
                                &HashSet::from([hub.clone()]),
                                &arms
                                    .iter()
                                    .map(|(n, e)| (hub.clone(), n.clone(), *e))
                                    .collect::<Vec<_>>(),
                            ))
                    {
                        return both(PathResolution::new(
                            spec,
                            STATUS_AMBIGUOUS,
                            "endpoint via fanout is outside the proved local pad-to-trunk motif",
                        ));
                    }
                }
            }
        }
    }
    if component_has_cycle(&g, &start, &arrays) {
        return both(PathResolution::new(
            spec,
            STATUS_AMBIGUOUS,
            "declared endpoints sit on a net whose routed copper contains a loop/parallel return \
             path reachable from them -- current split between branches cannot be determined \
             from a single resolved path",
        ));
    }
    let path = dedupe(
        path.into_iter()
            .chain(arrays.iter().flat_map(|a| a.members.iter().copied())),
    );
    let unmodeled = unmodeled_copper(pcb, &spec.net_name);
    if !unmodeled.is_empty() {
        let mut kinds: Vec<&str> = unmodeled.iter().map(|u| u.kind).collect();
        kinds.sort();
        kinds.dedup();
        return both(PathResolution::new(
            spec,
            STATUS_AMBIGUOUS,
            format!(
                "net {} carries {} unmodeled same-net {} object(s) (a routed arc or a copper \
                 pour) this bounded centerline model cannot verify does not form a parallel \
                 return path around the declared branch",
                py_repr_str(&spec.net_name),
                unmodeled.len(),
                kinds.join("/")
            ),
        ));
    }
    let length_mm = crate::utils::pymath::py_sum(path.iter().map(|&i| seg_length(&all[i])));
    let mut r = both(PathResolution::new(spec, STATUS_RESOLVED, ""));
    r.segments = path;
    r.length_mm = length_mm;
    r
}

/// Independent post-write audit of declared current paths.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CurrentPathAudit {
    pub resolutions: Vec<PathResolution>,
    /// `(net, uncovered segment indices)` in first-declared net order.
    pub uncovered: Vec<(String, Vec<usize>)>,
    pub unmodeled: Vec<(String, Vec<UnmodeledCopper>)>,
}

impl CurrentPathAudit {
    pub fn all_resolved(&self) -> bool {
        self.resolutions.iter().all(PathResolution::ok)
    }

    pub fn fully_covered(&self) -> bool {
        self.uncovered.iter().all(|(_, v)| v.is_empty())
    }
}

/// Resolve every declared path and report uncovered / unmodeled copper.
///
/// Upstream iterates the declared nets as a Python `set` (hash order); the
/// port uses first-declaration order, which is deterministic.
pub fn audit_current_paths(pcb: &Pcb, specs: &[CurrentPathSpec]) -> CurrentPathAudit {
    let resolutions: Vec<PathResolution> = specs.iter().map(|s| resolve_current_path(pcb, s)).collect();
    let mut covered: HashMap<&str, HashSet<usize>> = HashMap::new();
    for r in &resolutions {
        if r.ok() {
            covered
                .entry(r.spec.net_name.as_str())
                .or_default()
                .extend(r.segments.iter().copied());
        }
    }
    let mut audit = CurrentPathAudit {
        resolutions: resolutions.clone(),
        ..Default::default()
    };
    let mut nets: Vec<&str> = Vec::new();
    for s in specs {
        if !nets.contains(&s.net_name.as_str()) {
            nets.push(&s.net_name);
        }
    }
    for net in nets {
        let cov = covered.get(net);
        let missing: Vec<usize> = net_segments(pcb, net)
            .into_iter()
            .filter(|i| !cov.is_some_and(|c| c.contains(i)))
            .collect();
        if !missing.is_empty() {
            audit.uncovered.push((net.to_string(), missing));
        }
        let found = unmodeled_copper(pcb, net);
        if !found.is_empty() {
            audit.unmodeled.push((net.to_string(), found));
        }
    }
    audit
}

/// Segment indices that may receive a buttress-wire anchor.
pub fn reinforcement_eligible_segment_ids(pcb: &Pcb, specs: &[CurrentPathSpec]) -> HashSet<usize> {
    let mut out = HashSet::new();
    for s in specs.iter().filter(|s| s.reinforcement_eligible) {
        let r = resolve_current_path(pcb, s);
        if r.ok() {
            out.extend(r.segments);
        }
    }
    out
}
