//! Layer stack and via definitions for PCB routing (port of
//! `kicad_tools.router.layers`).
//!
//! - [`Layer`]: alias for [`CopperLayer`] (F.Cu, In1.Cu, ...)
//! - [`LayerType`]: signal, plane, or mixed
//! - [`LayerDefinition`] / [`LayerStack`]: stackup with presets
//! - [`ViaType`] / [`ViaDefinition`] / [`ViaRules`]: via manufacturing rules

use std::fmt;

use crate::core::types::CopperLayer;
use crate::exceptions::KiCadToolsError;

/// Routing layer (upstream `Layer = CopperLayer`).
pub type Layer = CopperLayer;

/// Python-side helpers for [`Layer`] (`layer.name`, `Layer.from_kicad_name`).
pub trait LayerExt: Sized {
    /// Python enum member name (`Layer.F_CU.name == "F_CU"`).
    fn py_name(self) -> &'static str;
    /// Lookup by KiCad name without an error type.
    fn from_name(name: &str) -> Option<Self>;
}

impl LayerExt for Layer {
    fn py_name(self) -> &'static str {
        match self {
            Layer::FCu => "F_CU",
            Layer::In1Cu => "IN1_CU",
            Layer::In2Cu => "IN2_CU",
            Layer::In3Cu => "IN3_CU",
            Layer::In4Cu => "IN4_CU",
            Layer::BCu => "B_CU",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Layer::ALL.iter().copied().find(|l| l.kicad_name() == name)
    }
}

/// Layer function type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerType {
    /// Primary signal routing layer.
    Signal,
    /// Power/ground plane (routable; zones flow around traces).
    Plane,
    /// Plane with signal routing (split planes).
    Mixed,
}

impl LayerType {
    pub fn value(self) -> &'static str {
        match self {
            LayerType::Signal => "signal",
            LayerType::Plane => "plane",
            LayerType::Mixed => "mixed",
        }
    }

    pub fn from_value(v: &str) -> Option<Self> {
        match v {
            "signal" => Some(LayerType::Signal),
            "plane" => Some(LayerType::Plane),
            "mixed" => Some(LayerType::Mixed),
            _ => None,
        }
    }
}

/// Definition of a single PCB layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerDefinition {
    /// KiCad name: "F.Cu", "In1.Cu", etc.
    pub name: String,
    /// Layer index (0 = top).
    pub index: usize,
    pub layer_type: LayerType,
    /// True for F.Cu and B.Cu.
    pub is_outer: bool,
    /// Net name if this is a plane (e.g. "GND").
    pub plane_net: String,
    /// Adjacent plane for impedance control.
    pub reference_plane: String,
    /// Copper thickness (1oz = 35um).
    pub copper_weight_oz: f64,
}

impl LayerDefinition {
    pub fn new(name: &str, index: usize, layer_type: LayerType) -> Self {
        Self {
            name: name.to_string(),
            index,
            layer_type,
            is_outer: false,
            plane_net: String::new(),
            reference_plane: String::new(),
            copper_weight_oz: 1.0,
        }
    }

    pub fn outer(mut self) -> Self {
        self.is_outer = true;
        self
    }

    pub fn with_plane_net(mut self, net: &str) -> Self {
        self.plane_net = net.to_string();
        self
    }

    pub fn with_reference_plane(mut self, plane: &str) -> Self {
        self.reference_plane = plane.to_string();
        self
    }

    /// Corresponding [`Layer`] enum value (falls back to F.Cu).
    pub fn layer_enum(&self) -> Layer {
        Layer::from_name(&self.name).unwrap_or(Layer::FCu)
    }

    /// All copper layers are routable (plane exclusivity is enforced one
    /// level up via `DesignRules.allowed_layers`; issue #5014/#5789).
    pub fn is_routable(&self) -> bool {
        true
    }
}

/// Complete PCB layer stackup configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerStack {
    pub layers: Vec<LayerDefinition>,
    pub name: String,
    pub description: String,
}

impl LayerStack {
    /// Construct and validate (indices must be sequential from 0).
    pub fn new(
        layers: Vec<LayerDefinition>,
        name: &str,
        description: &str,
    ) -> Result<Self, KiCadToolsError> {
        let indices: Vec<usize> = layers.iter().map(|l| l.index).collect();
        let expected: Vec<usize> = (0..layers.len()).collect();
        if indices != expected {
            return Err(KiCadToolsError::routing("Invalid layer stack configuration")
                .with_context("indices", serde_json::json!(indices))
                .with_context("expected", serde_json::json!(expected))
                .with_suggestion("Layer indices must be sequential starting from 0"));
        }
        Ok(Self {
            layers,
            name: name.to_string(),
            description: description.to_string(),
        })
    }

    fn preset(name: &str, description: &str, layers: Vec<LayerDefinition>) -> Self {
        Self {
            layers,
            name: name.to_string(),
            description: description.to_string(),
        }
    }

    pub fn num_layers(&self) -> usize {
        self.layers.len()
    }

    pub fn signal_layers(&self) -> Vec<&LayerDefinition> {
        self.layers.iter().filter(|l| l.is_routable()).collect()
    }

    pub fn plane_layers(&self) -> Vec<&LayerDefinition> {
        self.layers
            .iter()
            .filter(|l| l.layer_type == LayerType::Plane)
            .collect()
    }

    pub fn outer_layers(&self) -> Vec<&LayerDefinition> {
        self.layers.iter().filter(|l| l.is_outer).collect()
    }

    pub fn get_layer(&self, index: usize) -> Option<&LayerDefinition> {
        self.layers.iter().find(|l| l.index == index)
    }

    pub fn get_layer_by_name(&self, name: &str) -> Option<&LayerDefinition> {
        self.layers.iter().find(|l| l.name == name)
    }

    /// Map Layer enum to grid index for this stackup.
    pub fn layer_enum_to_index(&self, layer: Layer) -> Result<usize, KiCadToolsError> {
        let kicad_name = layer.kicad_name();
        if let Some(def) = self.layers.iter().find(|d| d.name == kicad_name) {
            return Ok(def.index);
        }
        let names: Vec<&str> = self.layers.iter().map(|l| l.name.as_str()).collect();
        Err(KiCadToolsError::routing("Layer not found in stack")
            .with_context("layer", layer.py_name())
            .with_context("stack", self.name.as_str())
            .with_suggestion(format!(
                "Available layers: [{}]",
                names
                    .iter()
                    .map(|n| format!("'{n}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
    }

    /// Map grid index to Layer enum for this stackup.
    pub fn index_to_layer_enum(&self, index: usize) -> Result<Layer, KiCadToolsError> {
        if index >= self.layers.len() {
            return Err(KiCadToolsError::routing("Layer index out of range")
                .with_context("index", index as i64)
                .with_context("num_layers", self.num_layers() as i64)
                .with_suggestion(format!(
                    "Valid indices: 0 to {}",
                    self.num_layers() as i64 - 1
                )));
        }
        let def = &self.layers[index];
        Layer::from_name(&def.name).ok_or_else(|| {
            KiCadToolsError::routing("No Layer enum for layer definition")
                .with_context("layer_name", def.name.as_str())
        })
    }

    pub fn get_routable_indices(&self) -> Vec<usize> {
        self.layers
            .iter()
            .filter(|l| l.is_routable())
            .map(|l| l.index)
            .collect()
    }

    pub fn is_plane_layer(&self, index: usize) -> bool {
        self.get_layer(index)
            .is_some_and(|l| l.layer_type == LayerType::Plane)
    }

    pub fn get_outer_layer_indices(&self) -> Vec<usize> {
        self.layers
            .iter()
            .filter(|l| l.is_outer)
            .map(|l| l.index)
            .collect()
    }

    pub fn get_inner_layer_indices(&self) -> Vec<usize> {
        self.layers
            .iter()
            .filter(|l| !l.is_outer && l.is_routable())
            .map(|l| l.index)
            .collect()
    }

    /// Layer indices adjacent to a plane layer (optionally of `plane_net`).
    pub fn get_layers_adjacent_to_plane(&self, plane_net: Option<&str>) -> Vec<usize> {
        let mut adjacent: Vec<usize> = Vec::new();
        for (i, layer) in self.layers.iter().enumerate() {
            if layer.layer_type == LayerType::Plane
                && plane_net.is_none_or(|n| layer.plane_net == n)
            {
                if i > 0 {
                    let prev = &self.layers[i - 1];
                    if prev.is_routable() && !adjacent.contains(&prev.index) {
                        adjacent.push(prev.index);
                    }
                }
                if i + 1 < self.layers.len() {
                    let next = &self.layers[i + 1];
                    if next.is_routable() && !adjacent.contains(&next.index) {
                        adjacent.push(next.index);
                    }
                }
            }
        }
        adjacent.sort_unstable();
        adjacent
    }

    pub fn get_layers_with_reference_plane(&self) -> Vec<usize> {
        self.layers
            .iter()
            .filter(|l| l.is_routable() && !l.reference_plane.is_empty())
            .map(|l| l.index)
            .collect()
    }

    /// Standard 2-layer board: Signal-Signal.
    pub fn two_layer() -> Self {
        Self::preset(
            "2-Layer",
            "Standard 2-layer PCB",
            vec![
                LayerDefinition::new("F.Cu", 0, LayerType::Signal).outer(),
                LayerDefinition::new("B.Cu", 1, LayerType::Signal).outer(),
            ],
        )
    }

    /// Standard 4-layer: Signal-GND-PWR-Signal.
    pub fn four_layer_sig_gnd_pwr_sig() -> Self {
        Self::preset(
            "4-Layer SIG-GND-PWR-SIG",
            "Standard 4-layer with GND and PWR planes",
            vec![
                LayerDefinition::new("F.Cu", 0, LayerType::Signal)
                    .outer()
                    .with_reference_plane("In1.Cu"),
                LayerDefinition::new("In1.Cu", 1, LayerType::Plane).with_plane_net("GND"),
                LayerDefinition::new("In2.Cu", 2, LayerType::Plane).with_plane_net("+3.3V"),
                LayerDefinition::new("B.Cu", 3, LayerType::Signal)
                    .outer()
                    .with_reference_plane("In2.Cu"),
            ],
        )
    }

    /// 4-layer with 2 signal + 2 plane: Signal-Signal-GND-PWR.
    pub fn four_layer_sig_sig_gnd_pwr() -> Self {
        Self::preset(
            "4-Layer SIG-SIG-GND-PWR",
            "4-layer with 2 signal layers",
            vec![
                LayerDefinition::new("F.Cu", 0, LayerType::Signal).outer(),
                LayerDefinition::new("In1.Cu", 1, LayerType::Signal)
                    .with_reference_plane("In2.Cu"),
                LayerDefinition::new("In2.Cu", 2, LayerType::Plane).with_plane_net("GND"),
                LayerDefinition::new("B.Cu", 3, LayerType::Mixed)
                    .outer()
                    .with_plane_net("+3.3V"),
            ],
        )
    }

    /// 4-layer with all layers as signal (no dedicated planes).
    pub fn four_layer_all_signal() -> Self {
        Self::preset(
            "4-Layer ALL-SIG",
            "4-layer with all copper layers for signal routing",
            vec![
                LayerDefinition::new("F.Cu", 0, LayerType::Signal).outer(),
                LayerDefinition::new("In1.Cu", 1, LayerType::Signal),
                LayerDefinition::new("In2.Cu", 2, LayerType::Signal),
                LayerDefinition::new("B.Cu", 3, LayerType::Signal).outer(),
            ],
        )
    }

    /// 6-layer high-density: Signal-GND-Signal-Signal-PWR-Signal.
    pub fn six_layer_sig_gnd_sig_sig_pwr_sig() -> Self {
        Self::preset(
            "6-Layer SIG-GND-SIG-SIG-PWR-SIG",
            "6-layer high-density with 4 signal layers",
            vec![
                LayerDefinition::new("F.Cu", 0, LayerType::Signal)
                    .outer()
                    .with_reference_plane("In1.Cu"),
                LayerDefinition::new("In1.Cu", 1, LayerType::Plane).with_plane_net("GND"),
                LayerDefinition::new("In2.Cu", 2, LayerType::Signal)
                    .with_reference_plane("In1.Cu"),
                LayerDefinition::new("In3.Cu", 3, LayerType::Signal)
                    .with_reference_plane("In4.Cu"),
                LayerDefinition::new("In4.Cu", 4, LayerType::Plane).with_plane_net("+3.3V"),
                LayerDefinition::new("B.Cu", 5, LayerType::Signal)
                    .outer()
                    .with_reference_plane("In4.Cu"),
            ],
        )
    }
}

impl fmt::Display for LayerStack {
    /// Upstream `LayerStack.__repr__`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .layers
            .iter()
            .map(|l| {
                let t = l.layer_type.value()[..1].to_uppercase();
                let net = if l.plane_net.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", l.plane_net)
                };
                format!("L{}:{}[{t}]{net}", l.index + 1, l.name)
            })
            .collect();
        write!(f, "LayerStack({}: {})", self.name, parts.join(" | "))
    }
}

/// Via types by layer span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViaType {
    Through,
    BlindTop,
    BlindBot,
    Buried,
    Micro,
}

impl ViaType {
    pub fn value(self) -> &'static str {
        match self {
            ViaType::Through => "through",
            ViaType::BlindTop => "blind_top",
            ViaType::BlindBot => "blind_bot",
            ViaType::Buried => "buried",
            ViaType::Micro => "micro",
        }
    }
}

/// Definition of a via type with manufacturing parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct ViaDefinition {
    pub via_type: ViaType,
    pub drill_mm: f64,
    pub annular_ring_mm: f64,
    pub start_layer: i64,
    /// Ending layer index (-1 = bottom).
    pub end_layer: i64,
    pub cost_multiplier: f64,
    pub name: String,
}

impl ViaDefinition {
    pub fn new(via_type: ViaType, drill_mm: f64, annular_ring_mm: f64) -> Self {
        Self {
            via_type,
            drill_mm,
            annular_ring_mm,
            start_layer: 0,
            end_layer: -1,
            cost_multiplier: 1.0,
            name: String::new(),
        }
    }

    /// Total via pad diameter.
    pub fn diameter(&self) -> f64 {
        self.drill_mm + 2.0 * self.annular_ring_mm
    }

    fn end(&self, num_layers: usize) -> i64 {
        if self.end_layer >= 0 {
            self.end_layer
        } else {
            num_layers as i64 - 1
        }
    }

    pub fn spans_layer(&self, layer: i64, num_layers: usize) -> bool {
        self.start_layer <= layer && layer <= self.end(num_layers)
    }

    pub fn blocks_layer(&self, layer: i64, num_layers: usize) -> bool {
        self.spans_layer(layer, num_layers)
    }
}

/// Manufacturing rules for via types.
#[derive(Debug, Clone, PartialEq)]
pub struct ViaRules {
    pub allow_blind: bool,
    pub allow_buried: bool,
    pub allow_micro: bool,
    pub allow_stacked: bool,
    pub allow_via_in_pad: bool,
    pub min_via_to_via_mm: f64,
    pub min_via_to_trace_mm: f64,
    pub min_via_to_plane_mm: f64,
    pub through_via: ViaDefinition,
    pub blind_via: Option<ViaDefinition>,
    pub buried_via: Option<ViaDefinition>,
    pub micro_via: Option<ViaDefinition>,
}

impl Default for ViaRules {
    fn default() -> Self {
        let mut through = ViaDefinition::new(ViaType::Through, 0.3, 0.15);
        through.name = "Standard Through".into();
        Self {
            allow_blind: false,
            allow_buried: false,
            allow_micro: false,
            allow_stacked: false,
            allow_via_in_pad: false,
            min_via_to_via_mm: 0.2,
            min_via_to_trace_mm: 0.15,
            min_via_to_plane_mm: 0.2,
            through_via: through,
            blind_via: None,
            buried_via: None,
            micro_via: None,
        }
    }
}

impl ViaRules {
    pub fn get_available_vias(&self, num_layers: usize) -> Vec<&ViaDefinition> {
        let mut vias = vec![&self.through_via];
        if self.allow_blind {
            if let Some(v) = &self.blind_via {
                vias.push(v);
            }
        }
        if self.allow_buried && num_layers >= 4 {
            if let Some(v) = &self.buried_via {
                vias.push(v);
            }
        }
        if self.allow_micro {
            if let Some(v) = &self.micro_via {
                vias.push(v);
            }
        }
        vias
    }

    /// Lowest-cost via that can connect two layers (first wins on ties,
    /// matching Python's `min`).
    pub fn get_best_via(
        &self,
        from_layer: i64,
        to_layer: i64,
        num_layers: usize,
    ) -> Option<&ViaDefinition> {
        let mut best: Option<&ViaDefinition> = None;
        for via in self.get_available_vias(num_layers) {
            let end = via.end(num_layers);
            let start = via.start_layer;
            if start <= from_layer && from_layer <= end && start <= to_layer && to_layer <= end {
                match best {
                    Some(b) if b.cost_multiplier <= via.cost_multiplier => {}
                    _ => best = Some(via),
                }
            }
        }
        best
    }

    fn through_spanning(end_layer: i64) -> ViaDefinition {
        ViaDefinition {
            via_type: ViaType::Through,
            drill_mm: 0.3,
            annular_ring_mm: 0.15,
            start_layer: 0,
            end_layer,
            cost_multiplier: 1.0,
            name: "Through".into(),
        }
    }

    pub fn standard_2layer() -> Self {
        Self {
            through_via: Self::through_spanning(1),
            ..Self::default()
        }
    }

    pub fn standard_4layer() -> Self {
        Self {
            through_via: Self::through_spanning(3),
            ..Self::default()
        }
    }

    pub fn hdi_4layer() -> Self {
        Self {
            allow_blind: true,
            allow_micro: true,
            through_via: ViaDefinition {
                via_type: ViaType::Through,
                drill_mm: 0.25,
                annular_ring_mm: 0.1,
                start_layer: 0,
                end_layer: 3,
                cost_multiplier: 1.0,
                name: "Through".into(),
            },
            blind_via: Some(ViaDefinition {
                via_type: ViaType::BlindTop,
                drill_mm: 0.15,
                annular_ring_mm: 0.1,
                start_layer: 0,
                end_layer: 1,
                cost_multiplier: 1.5,
                name: "Blind Top".into(),
            }),
            micro_via: Some(ViaDefinition {
                via_type: ViaType::Micro,
                drill_mm: 0.1,
                annular_ring_mm: 0.075,
                start_layer: 0,
                end_layer: 1,
                cost_multiplier: 0.5,
                name: "Micro".into(),
            }),
            ..Self::default()
        }
    }

    pub fn standard_6layer() -> Self {
        Self {
            through_via: Self::through_spanning(5),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_repr() {
        assert_eq!(
            LayerStack::four_layer_sig_gnd_pwr_sig().to_string(),
            "LayerStack(4-Layer SIG-GND-PWR-SIG: L1:F.Cu[S] | L2:In1.Cu[P] (GND) | L3:In2.Cu[P] (+3.3V) | L4:B.Cu[S])"
        );
    }

    #[test]
    fn layer_index_mapping() {
        let s = LayerStack::two_layer();
        assert_eq!(s.layer_enum_to_index(Layer::BCu).unwrap(), 1);
        assert_eq!(s.index_to_layer_enum(1).unwrap(), Layer::BCu);
        assert!(s.layer_enum_to_index(Layer::In1Cu).is_err());
        assert!(s.index_to_layer_enum(2).is_err());
        let six = LayerStack::six_layer_sig_gnd_sig_sig_pwr_sig();
        assert_eq!(six.get_layers_adjacent_to_plane(None), vec![0, 2, 3, 5]);
        assert_eq!(six.get_layers_adjacent_to_plane(Some("GND")), vec![0, 2]);
        assert_eq!(six.get_inner_layer_indices(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn invalid_stack_rejected() {
        let r = LayerStack::new(
            vec![LayerDefinition::new("F.Cu", 1, LayerType::Signal)],
            "x",
            "",
        );
        assert!(r.is_err());
    }

    #[test]
    fn best_via() {
        let r = ViaRules::hdi_4layer();
        assert_eq!(r.get_best_via(0, 1, 4).unwrap().name, "Micro");
        assert_eq!(r.get_best_via(0, 3, 4).unwrap().name, "Through");
        assert!((r.through_via.diameter() - 0.45).abs() < 1e-12);
    }
}
