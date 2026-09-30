//! Reviewed facts and design requirements. These are inputs, never asserted pass/fail results.
use crate::model::Point;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Intent {
    pub roles: BTreeMap<String, String>,
    pub route: RoutePolicy,
    pub placement: Vec<Placement>,
    pub passive_alignment: PassiveAlignment,
    pub peers: Vec<Peers>,
    pub channels: Vec<ChannelPair>,
    pub flows: Vec<Flow>,
    pub corridors: Vec<Corridor>,
    pub sensitive: Vec<Region>,
    pub loops: Vec<LoopSpec>,
    pub schematic: Option<Schematic>,
    pub netlist: Option<Vec<Pin>>,
    pub baseline: Option<Baseline>,
    pub currents: Vec<Current>,
    pub features: Option<Features>,
    pub swaps: Vec<SwapGroup>,
    pub naming: Vec<Naming>,
    pub interfaces: Vec<Interface>,
    pub mating: Vec<Mating>,
    pub probes: Vec<Probe>,
    pub bodies: Vec<Body>,
    pub pairs: Vec<Pair>,
    pub references: Vec<ReferencePlane>,
    pub launches: Vec<Launch>,
    pub pin_roles: Vec<PinRole>,
    pub straps: Vec<Strap>,
    pub modes: Vec<PowerMode>,
    pub capacitors: Vec<Capacitor>,
    pub packages: Vec<Package>,
    pub access: Vec<Access>,
    pub thermals: Vec<Thermal>,
    pub assembly: Option<Assembly>,
    pub labels: Vec<Label>,
    pub artifacts: Vec<Artifact>,
    pub release: Option<Release>,
    pub native: Option<Native>,
    pub observations: Vec<Observation>,
    pub annotations: Vec<Annotation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoutePolicy {
    pub clearance_mm: f64,
    pub grid_mm: f64,
    pub search_margin_mm: f64,
    pub max_nodes: usize,
    pub minimum_saving_mm: f64,
    pub edge_clearance_mm: f64,
    pub outline: Vec<Point>,
    pub keepouts: Vec<Region>,
    pub tuned_nets: Vec<String>,
    pub width_ratio: f64,
    pub grazing_margin_mm: f64,
    pub max_return_via_mm: f64,
}
/// Advisory geometric grouping; never authorizes component moves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PassiveAlignment {
    pub max_gap_mm: f64,
    pub max_offset_mm: f64,
    pub tolerance_mm: f64,
    pub angle_tolerance_deg: f64,
    pub min_group: usize,
    pub exclude_references: Vec<String>,
}
impl Default for PassiveAlignment {
    fn default() -> Self {
        Self {
            max_gap_mm: 5.,
            max_offset_mm: 0.5,
            tolerance_mm: 0.1,
            angle_tolerance_deg: 2.,
            min_group: 3,
            exclude_references: vec![],
        }
    }
}
impl Default for RoutePolicy {
    fn default() -> Self {
        Self {
            clearance_mm: 0.15,
            grid_mm: 0.25,
            search_margin_mm: 3.,
            max_nodes: 15000,
            minimum_saving_mm: 1.,
            edge_clearance_mm: 0.25,
            outline: vec![],
            keepouts: vec![],
            tuned_nets: vec![],
            width_ratio: 1.5,
            grazing_margin_mm: 0.025,
            max_return_via_mm: 1.,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub reference: String,
    pub partners: Vec<String>,
    pub step_mm: f64,
    pub min_saving_mm: f64,
    pub rotations_deg: Vec<f64>,
    pub max_connections: usize,
    pub radius_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peers {
    pub name: String,
    pub references: Vec<String>,
    pub axis: String,
    pub pitch_mm: f64,
    pub tolerance_mm: f64,
    pub angle_deg: f64,
    pub angle_tolerance_deg: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelPair {
    pub name: String,
    pub pairs: Vec<[String; 2]>,
    pub net_map: BTreeMap<String, String>,
    pub translate: Point,
    pub rotation_deg: f64,
    pub tolerance_mm: f64,
    pub polarized: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    pub name: String,
    pub references: Vec<String>,
    pub axis: String,
    pub minimum_gap_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corridor {
    pub name: String,
    pub nets: Vec<String>,
    pub max_separation_mm: f64,
    pub compatible: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub id: String,
    pub layer: String,
    pub polygon: Vec<Point>,
    #[serde(default)]
    pub nets: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub reference: String,
    pub pad: String,
    pub net: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopSpec {
    pub id: String,
    pub kind: String,
    pub capacitor: String,
    pub power_pin: Pin,
    pub ground_net: String,
    pub max_pin_mm: f64,
    pub max_loop_area_mm2: f64,
    pub path: Vec<String>,
    pub max_path_mm: f64,
    pub allowed_nets: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub min: Point,
    pub max: Point,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrawItem {
    pub id: String,
    pub sheet: String,
    pub kind: String,
    pub bounds: Rect,
    #[serde(default)]
    pub attach_to: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wire {
    pub id: String,
    pub sheet: String,
    pub a: Point,
    pub b: Point,
    pub net: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schematic {
    pub source_sha256: String,
    pub items: Vec<DrawItem>,
    pub wires: Vec<Wire>,
    pub junctions: Vec<(String, Point)>,
    pub max_stem_mm: f64,
    pub reading_order: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub objects: Vec<PreviousObject>,
    pub connectivity: Vec<Vec<String>>,
    pub lengths: BTreeMap<String, f64>,
    pub max_length_change_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviousObject {
    pub id: String,
    pub kind: String,
    pub net: String,
    pub at: Point,
    pub finding_key: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Current {
    pub net: String,
    pub amperes: f64,
    pub copper_um: f64,
    pub temperature_rise_c: f64,
    pub internal: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Features {
    pub allowed_references: Vec<String>,
    pub required_references: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwapPin {
    pub pin: Pin,
    pub bank: String,
    pub voltage: f64,
    pub capabilities: Vec<String>,
    pub required: Vec<String>,
    pub mate_fixed: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwapGroup {
    pub name: String,
    pub pins: Vec<SwapPin>,
    pub minimum_saving_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Naming {
    pub nets: Vec<String>,
    pub pattern: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interface {
    pub name: String,
    pub pins: Vec<Pin>,
    pub required_references: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mating {
    pub name: String,
    pub expected: Vec<Pin>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub net: String,
    pub reference: Option<String>,
    pub radius_mm: f64,
    pub ground_net: String,
    pub max_ground_mm: f64,
    pub expected_label: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub id: String,
    pub side: String,
    pub bounds: Rect,
    pub z_min_mm: f64,
    pub z_max_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    pub name: String,
    pub positive: String,
    pub negative: String,
    pub max_skew_mm: f64,
    pub gap_mm: f64,
    pub gap_tolerance_mm: f64,
    pub max_length_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferencePlane {
    pub net: String,
    pub layer: String,
    pub plane_net: String,
    pub plane_layer: String,
    pub margin_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub reference: String,
    pub signal_pad: String,
    pub ground_net: String,
    pub radius_mm: f64,
    pub minimum_vias: usize,
    pub min_width_mm: f64,
    pub max_width_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinRole {
    pub pin: Pin,
    pub role: String,
    pub source: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strap {
    pub pin: Pin,
    pub pull_reference: String,
    pub target_net: String,
    pub low_ohm: f64,
    pub high_ohm: f64,
    pub source: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub name: String,
    pub positive: String,
    pub negative: String,
    pub volts: f64,
    pub isolation_group: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub from: String,
    pub to: String,
    pub bidirectional: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerMode {
    pub name: String,
    pub sources: Vec<Source>,
    pub links: Vec<Link>,
    pub output_positive: String,
    pub output_negative: String,
    pub target_volts: f64,
    pub tolerance_volts: f64,
    pub allow_parallel: bool,
    pub connector_pins: Vec<Pin>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capacitor {
    pub reference: String,
    pub role: String,
    pub net: String,
    pub max_volts: f64,
    pub rating_volts: f64,
    pub derating_fraction: f64,
    pub effective_uf: f64,
    pub required_uf: f64,
    pub source: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub reference: String,
    pub footprint: String,
    pub pads: Vec<String>,
    pub manufacturer: String,
    pub mpn: String,
    pub source: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    pub reference: String,
    pub side: String,
    pub envelope: Rect,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thermal {
    pub reference: String,
    pub pad: String,
    pub allowed_connection: String,
    pub min_spokes: usize,
    pub max_gap_mm: f64,
    pub min_spoke_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cpl {
    pub reference: String,
    pub at: Point,
    pub angle_deg: f64,
    pub side: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assembly {
    pub fields: Vec<String>,
    pub excluded: Vec<String>,
    pub placements: Vec<Cpl>,
    pub origin: Point,
    pub angle_offsets: BTreeMap<String, f64>,
    pub tolerance_mm: f64,
    pub tolerance_deg: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Label {
    pub reference: String,
    pub text: String,
    pub layer: String,
    pub max_distance_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub name: String,
    pub expected_sha256: String,
    pub actual_sha256: String,
    pub built_from_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRun {
    pub tool: String,
    pub version: String,
    pub source_sha256: String,
    pub unwaived_errors: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub required_artifacts: Vec<String>,
    pub required_checks: Vec<String>,
    pub checks: Vec<CheckRun>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Native {
    pub source_sha256: String,
    pub pads: BTreeMap<String, Point>,
    pub connectivity: Vec<Vec<String>>,
    pub tolerance_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub rule: String,
    pub key: String,
    pub actual: bool,
    pub predicted: bool,
    pub max_false_positive_rate: f64,
    pub max_false_negative_rate: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    pub id: String,
    pub source_sha256: String,
    pub subjects: Vec<String>,
}
impl Intent {
    pub fn validate(&self) -> anyhow::Result<()> {
        use anyhow::bail;
        // serde_json refuses nonfinite numbers on input, but Rust callers can construct them.
        // Optional fields legitimately serialize as null; check every numeric leaf by
        // round-tripping: a nonfinite float becomes null and cannot deserialize as f64.
        let encoded = serde_json::to_value(self)?;
        let _: Intent = serde_json::from_value(encoded.clone())
            .map_err(|e| anyhow::anyhow!("invalid/nonfinite intent measurement: {e}"))?;
        let nn = |v: f64| -> anyhow::Result<()> {
            if !v.is_finite() || v < 0. {
                bail!("measurement must be finite and nonnegative")
            }
            Ok(())
        };
        let alignment = &self.passive_alignment;
        for v in [
            alignment.max_gap_mm,
            alignment.max_offset_mm,
            alignment.tolerance_mm,
            alignment.angle_tolerance_deg,
        ] {
            nn(v)?;
        }
        if alignment.min_group < 2
            || alignment.max_gap_mm <= 0.
            || alignment.max_offset_mm <= alignment.tolerance_mm
            || alignment.angle_tolerance_deg > 10.
        {
            bail!("invalid passive_alignment thresholds");
        }
        let rect = |r: &Rect| -> anyhow::Result<()> {
            if r.min.x > r.max.x || r.min.y > r.max.y {
                bail!("inverted rectangle")
            }
            Ok(())
        };
        for c in &self.corridors {
            if c.nets.len() < 2 {
                bail!("corridor needs at least two nets")
            }
            nn(c.max_separation_mm)?;
        }
        for c in &self.channels {
            nn(c.tolerance_mm)?;
        }
        for f in &self.flows {
            nn(f.minimum_gap_mm)?;
        }
        for l in &self.loops {
            if !["decoupling", "feedback", "hot_loop"].contains(&l.kind.as_str()) {
                bail!("unknown loop kind")
            }
            for v in [l.max_pin_mm, l.max_loop_area_mm2, l.max_path_mm] {
                nn(v)?;
            }
        }
        for p in &self.probes {
            nn(p.radius_mm)?;
            nn(p.max_ground_mm)?;
        }
        for b in &self.bodies {
            rect(&b.bounds)?;
            if b.z_min_mm > b.z_max_mm {
                bail!("inverted body height")
            }
        }
        for a in &self.access {
            rect(&a.envelope)?;
        }
        for p in &self.pairs {
            for v in [p.max_skew_mm, p.gap_mm, p.gap_tolerance_mm, p.max_length_mm] {
                nn(v)?;
            }
        }
        for p in &self.references {
            nn(p.margin_mm)?;
        }
        for l in &self.launches {
            for v in [l.radius_mm, l.min_width_mm, l.max_width_mm] {
                nn(v)?;
            }
            if l.min_width_mm > l.max_width_mm {
                bail!("inverted launch width range")
            }
        }
        for s in &self.straps {
            nn(s.low_ohm)?;
            nn(s.high_ohm)?;
            if s.low_ohm > s.high_ohm {
                bail!("inverted resistor range")
            }
        }
        for s in &self.swaps {
            nn(s.minimum_saving_mm)?;
        }
        for t in &self.thermals {
            nn(t.max_gap_mm)?;
            nn(t.min_spoke_mm)?;
        }
        for l in &self.labels {
            nn(l.max_distance_mm)?;
        }
        if let Some(n) = &self.native {
            nn(n.tolerance_mm)?;
        }
        if let Some(b) = &self.baseline {
            nn(b.max_length_change_mm)?;
        }
        if let Some(a) = &self.assembly {
            nn(a.tolerance_mm)?;
            nn(a.tolerance_deg)?;
        }
        if let Some(s) = &self.schematic {
            nn(s.max_stem_mm)?;
            for i in &s.items {
                rect(&i.bounds)?;
            }
        }
        for o in &self.observations {
            for v in [o.max_false_positive_rate, o.max_false_negative_rate] {
                if !(0. ..=1.).contains(&v) {
                    bail!("noise rate outside 0..1")
                }
            }
        }
        let p = &self.route;
        for v in [
            p.clearance_mm,
            p.grid_mm,
            p.search_margin_mm,
            p.minimum_saving_mm,
            p.edge_clearance_mm,
            p.width_ratio,
            p.grazing_margin_mm,
            p.max_return_via_mm,
        ] {
            if !v.is_finite() || v <= 0. {
                bail!("route policy measurements must be positive")
            }
        }
        if p.max_nodes < 8 || p.max_nodes > 100000 || p.width_ratio <= 1. {
            bail!("invalid route search budget/width ratio")
        }
        for n in &self.naming {
            regex::Regex::new(&n.pattern)?;
        }
        for p in &self.placement {
            if p.step_mm <= 0. || p.radius_mm <= 0. || p.min_saving_mm < 0. {
                bail!("invalid placement thresholds")
            }
        }
        for p in &self.peers {
            if p.references.len() < 2
                || !["row", "column"].contains(&p.axis.as_str())
                || p.pitch_mm <= 0.
                || p.tolerance_mm < 0.
            {
                bail!("invalid peer contract")
            }
        }
        for p in &self.flows {
            if p.references.len() < 2 || !["x", "y"].contains(&p.axis.as_str()) {
                bail!("invalid flow contract")
            }
        }
        for p in &self.currents {
            if p.amperes <= 0. || p.copper_um <= 0. || p.temperature_rise_c <= 0. {
                bail!("invalid current estimate inputs")
            }
        }
        for c in &self.capacitors {
            if c.max_volts < 0.
                || c.rating_volts <= 0.
                || c.derating_fraction <= 0.
                || c.derating_fraction > 1.
                || c.required_uf <= 0.
                || c.effective_uf < 0.
            {
                bail!("invalid capacitor rating")
            }
        }
        for m in &self.modes {
            if m.sources.is_empty()
                || m.tolerance_volts < 0.
                || m.target_volts <= 0.
                || m.sources
                    .iter()
                    .any(|s| s.volts <= 0. || s.name.is_empty() || s.positive == s.negative)
            {
                bail!("invalid power operating mode")
            }
        }
        for p in self.route.keepouts.iter().chain(&self.sensitive) {
            if p.polygon.len() < 3 {
                bail!("region must have at least three points")
            }
        }
        Ok(())
    }
}
