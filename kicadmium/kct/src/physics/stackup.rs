//! PCB layer stackup for electromagnetic calculations (port of
//! `physics.stackup`): manufacturer presets, `.kicad_pcb` parsing, and
//! geometry queries.
//!
//! PCB parsing reads the `(setup (stackup ...))` and `(layers ...)` blocks
//! directly from the s-expression tree (same semantics as upstream
//! `PCB._parse_stackup` / `_parse_layers`, including native `addsublayer`
//! composite dielectrics).

use std::path::Path;

use super::constants::{
    copper_oz_from_thickness, copper_thickness_from_oz, COPPER_1OZ, COPPER_HALF_OZ, FR4_STANDARD,
};
use super::{PhysResult, ValueError};
use crate::sexp::SExp;

/// Type of layer in the stackup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerType {
    Copper,
    /// Prepreg or core.
    Dielectric,
    SolderMask,
    SilkScreen,
}

impl LayerType {
    /// Upstream enum value string.
    pub fn value(self) -> &'static str {
        match self {
            LayerType::Copper => "copper",
            LayerType::Dielectric => "dielectric",
            LayerType::SolderMask => "solder mask",
            LayerType::SilkScreen => "silk screen",
        }
    }
}

/// Single layer in a stackup.
#[derive(Debug, Clone, PartialEq)]
pub struct StackupLayer {
    pub name: String,
    pub layer_type: LayerType,
    pub thickness_mm: f64,
    pub material: String,
    pub epsilon_r: f64,
    pub loss_tangent: f64,
    pub copper_weight_oz: Option<f64>,
    /// Native copper role (`signal`/`power`), empty for presets.
    pub copper_role: String,
}

impl StackupLayer {
    pub fn new(name: impl Into<String>, layer_type: LayerType) -> Self {
        Self {
            name: name.into(),
            layer_type,
            thickness_mm: 0.0,
            material: String::new(),
            epsilon_r: 0.0,
            loss_tangent: 0.0,
            copper_weight_oz: None,
            copper_role: String::new(),
        }
    }

    fn copper(name: &str, thickness_mm: f64, oz: f64) -> Self {
        Self {
            thickness_mm,
            material: "copper".into(),
            copper_weight_oz: Some(oz),
            ..Self::new(name, LayerType::Copper)
        }
    }

    fn dielectric(name: &str, thickness_mm: f64, material: &str, er: f64, tan_d: f64) -> Self {
        Self {
            thickness_mm,
            material: material.into(),
            epsilon_r: er,
            loss_tangent: tan_d,
            ..Self::new(name, LayerType::Dielectric)
        }
    }

    pub fn is_copper(&self) -> bool {
        self.layer_type == LayerType::Copper
    }

    pub fn is_dielectric(&self) -> bool {
        self.layer_type == LayerType::Dielectric
    }

    /// Copper that is not explicitly declared a power plane.
    pub fn is_signal_layer(&self) -> bool {
        self.is_copper() && self.name.to_lowercase().ends_with(".cu") && self.copper_role != "power"
    }
}

/// Construction identity metadata (upstream `Stackup.construction` dict).
#[derive(Debug, Clone, PartialEq)]
pub enum Construction {
    /// Compatibility-only numerical model with no factory identity.
    Legacy { id: String },
    /// Verified factory construction.
    Factory {
        id: String,
        manufacturer: String,
        source_url: String,
        verified_on: String,
        nominal_board_thickness_mm: f64,
        outer_copper_oz: f64,
        inner_copper_oz: f64,
        assumptions: Vec<String>,
    },
}

impl Construction {
    pub fn id(&self) -> &str {
        match self {
            Construction::Legacy { id } | Construction::Factory { id, .. } => id,
        }
    }

    pub fn factory_id(&self) -> Option<&str> {
        match self {
            Construction::Legacy { .. } => None,
            Construction::Factory { id, .. } => Some(id),
        }
    }

    pub fn source_url(&self) -> Option<&str> {
        match self {
            Construction::Legacy { .. } => None,
            Construction::Factory { source_url, .. } => Some(source_url),
        }
    }

    pub fn compatibility_only(&self) -> bool {
        matches!(self, Construction::Legacy { .. })
    }
}

/// One `(layer ...)` entry of a KiCad `(setup (stackup ...))` block
/// (upstream `schema.pcb.StackupLayer`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PcbStackupLayer {
    pub name: String,
    /// copper, prepreg, core, solder mask, silk screen, ...
    pub layer_type: String,
    pub thickness: f64,
    pub material: String,
    pub epsilon_r: f64,
    pub loss_tangent: f64,
}

/// One `(N "name" type)` entry of the board `(layers ...)` block.
#[derive(Debug, Clone, PartialEq)]
pub struct PcbLayer {
    pub number: i64,
    pub name: String,
    pub layer_type: String,
}

/// Board data needed by [`Stackup::from_pcb_data`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PcbStackupData {
    /// `Some` when a `(setup ...)` block exists; the parsed stackup strata.
    pub setup_stackup: Option<Vec<PcbStackupLayer>>,
    /// Board layer table, deduplicated by number (last wins).
    pub layers: Vec<PcbLayer>,
}

impl PcbStackupData {
    /// Extract from a parsed `kicad_pcb` root.
    pub fn from_sexp(root: &SExp) -> Self {
        let mut data = PcbStackupData::default();
        for child in root.children.iter().filter(|c| c.is_list()) {
            match child.tag() {
                Some("layers") => {
                    for entry in child.children.iter().filter(|c| c.is_list()) {
                        if entry.children.is_empty() {
                            continue;
                        }
                        let Some(number) = entry.tag().and_then(|t| t.parse::<i64>().ok()) else {
                            continue;
                        };
                        let name = entry.text_at(0).unwrap_or_default();
                        let layer_type = entry.text_at(1).unwrap_or_else(|| "user".into());
                        let layer = PcbLayer {
                            number,
                            name,
                            layer_type,
                        };
                        if let Some(existing) = data.layers.iter_mut().find(|l| l.number == number)
                        {
                            *existing = layer;
                        } else {
                            data.layers.push(layer);
                        }
                    }
                }
                Some("setup") => {
                    data.setup_stackup = Some(
                        child
                            .find("stackup")
                            .map(parse_stackup_block)
                            .unwrap_or_default(),
                    );
                }
                _ => {}
            }
        }
        data
    }

    /// Copper layers (type `signal` or `power`).
    pub fn copper_layers(&self) -> Vec<&PcbLayer> {
        self.layers
            .iter()
            .filter(|l| l.layer_type == "signal" || l.layer_type == "power")
            .collect()
    }
}

fn float_or_zero(node: &SExp) -> f64 {
    node.float_at(0).unwrap_or(0.0)
}

/// Parse physical stackup strata, expanding native `addsublayer` entries.
pub fn parse_stackup_block(stackup: &SExp) -> Vec<PcbStackupLayer> {
    let mut layers = Vec::new();
    for child in stackup.children.iter().filter(|c| c.is_list()) {
        if !child.has_tag("layer") {
            continue;
        }
        let name = child.text_at(0).unwrap_or_default();
        let layer_type = child
            .find("type")
            .and_then(|t| t.text_at(0))
            .unwrap_or_default();
        let mut layer = PcbStackupLayer {
            name: name.clone(),
            layer_type: layer_type.clone(),
            ..Default::default()
        };
        let mut sublayer = 1;
        for item in &child.children {
            if item.is_atom() {
                if item.value.as_ref().and_then(|v| v.as_str()) == Some("addsublayer") {
                    layers.push(layer);
                    sublayer += 1;
                    layer = PcbStackupLayer {
                        name: format!("{name} (sublayer {sublayer})"),
                        layer_type: layer_type.clone(),
                        ..Default::default()
                    };
                }
                continue;
            }
            match item.tag() {
                Some("thickness") => layer.thickness = float_or_zero(item),
                Some("material") => layer.material = item.text_at(0).unwrap_or_default(),
                Some("epsilon_r") => layer.epsilon_r = float_or_zero(item),
                Some("loss_tangent") => layer.loss_tangent = float_or_zero(item),
                _ => {}
            }
        }
        layers.push(layer);
    }
    layers
}

/// Per-layer summary row (upstream `summary()["layers"][i]`).
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSummary {
    pub name: String,
    pub layer_type: &'static str,
    pub thickness_mm: f64,
    pub material: String,
    pub epsilon_r: Option<f64>,
    pub copper_oz: Option<f64>,
}

/// Upstream `Stackup.summary()` dict.
#[derive(Debug, Clone, PartialEq)]
pub struct StackupSummary {
    pub construction: Option<Construction>,
    pub board_thickness_mm: f64,
    pub num_copper_layers: usize,
    pub copper_finish: String,
    pub layers: Vec<LayerSummary>,
}

/// Complete PCB layer stackup, ordered top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct Stackup {
    pub layers: Vec<StackupLayer>,
    pub board_thickness_mm: f64,
    pub copper_finish: String,
    /// True when parsed from an explicit `(setup (stackup ...))` block.
    pub has_explicit_data: bool,
    pub construction: Option<Construction>,
}

impl Default for Stackup {
    fn default() -> Self {
        Self {
            layers: Vec::new(),
            board_thickness_mm: 1.6,
            copper_finish: String::new(),
            has_explicit_data: false,
            construction: None,
        }
    }
}

impl std::fmt::Display for Stackup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Stackup(layers={}L, thickness={}mm)",
            self.num_copper_layers(),
            super::py_float_repr(self.board_thickness_mm)
        )
    }
}

impl Stackup {
    pub fn new(layers: Vec<StackupLayer>) -> Self {
        Self {
            layers,
            ..Default::default()
        }
    }

    /// Load a `.kicad_pcb` file and derive its stackup.
    pub fn load(path: impl AsRef<Path>) -> crate::Result<Self> {
        let root = crate::sexp::parse_file(path)?;
        Ok(Self::from_pcb(&root))
    }

    /// Stackup from a parsed `kicad_pcb` tree (upstream `Stackup.from_pcb`).
    pub fn from_pcb(root: &SExp) -> Self {
        Self::from_pcb_data(&PcbStackupData::from_sexp(root))
    }

    /// Stackup from extracted board data; boards without an explicit stackup
    /// get a default based on their copper layer count.
    pub fn from_pcb_data(pcb: &PcbStackupData) -> Self {
        let strata = match &pcb.setup_stackup {
            Some(s) if !s.is_empty() => s,
            _ => return Self::create_default_stackup(pcb.copper_layers().len()),
        };
        let mut layers = Vec::new();
        for data in strata {
            let layer_type = Self::parse_layer_type(&data.layer_type);
            let role = pcb
                .layers
                .iter()
                .rev()
                .find(|l| l.name == data.name)
                .map(|l| l.layer_type.clone())
                .unwrap_or_default();
            let mut layer = StackupLayer {
                name: data.name.clone(),
                layer_type,
                thickness_mm: data.thickness,
                material: data.material.clone(),
                epsilon_r: data.epsilon_r,
                loss_tangent: data.loss_tangent,
                copper_weight_oz: None,
                copper_role: role,
            };
            if layer_type == LayerType::Copper && layer.thickness_mm > 0.0 {
                layer.copper_weight_oz = Some(copper_oz_from_thickness(layer.thickness_mm));
            }
            layers.push(layer);
        }
        let total = super::py_sum(layers.iter().map(|l| l.thickness_mm));
        Self {
            layers,
            board_thickness_mm: if total > 0.0 { total } else { 1.6 },
            copper_finish: String::new(),
            has_explicit_data: true,
            construction: None,
        }
    }

    fn create_default_stackup(num_copper: usize) -> Self {
        match num_copper {
            0..=2 => Self::default_2layer(1.6),
            4 => Self::jlcpcb_4layer(),
            6 => Self::default_6layer(),
            n => Self::create_generic_stackup(n),
        }
    }

    /// Generic N-copper stackup with prepreg/core sandwich.
    pub fn create_generic_stackup(num_copper_layers: usize) -> Self {
        let mut layers = vec![StackupLayer::copper("F.Cu", COPPER_1OZ.thickness_mm, 1.0)];
        for i in 1..num_copper_layers.saturating_sub(1) {
            let kind = if i % 2 == 0 { "core" } else { "prepreg" };
            layers.push(StackupLayer::dielectric(
                &format!("{kind} {i}"),
                0.2,
                "FR4",
                FR4_STANDARD.epsilon_r,
                FR4_STANDARD.loss_tangent,
            ));
            layers.push(StackupLayer::copper(
                &format!("In{i}.Cu"),
                COPPER_HALF_OZ.thickness_mm,
                0.5,
            ));
        }
        layers.push(StackupLayer::dielectric(
            "prepreg bottom",
            0.2,
            "FR4",
            FR4_STANDARD.epsilon_r,
            FR4_STANDARD.loss_tangent,
        ));
        layers.push(StackupLayer::copper("B.Cu", COPPER_1OZ.thickness_mm, 1.0));
        let total = super::py_sum(layers.iter().map(|l| l.thickness_mm));
        Self {
            layers,
            board_thickness_mm: total,
            ..Default::default()
        }
    }

    /// KiCad stackup type string to [`LayerType`].
    pub fn parse_layer_type(type_str: &str) -> LayerType {
        let t = type_str.to_lowercase();
        if t == "copper" {
            LayerType::Copper
        } else if matches!(t.as_str(), "prepreg" | "core" | "dielectric") {
            LayerType::Dielectric
        } else if t.contains("mask") {
            LayerType::SolderMask
        } else if t.contains("silk") {
            LayerType::SilkScreen
        } else {
            LayerType::Dielectric
        }
    }

    // ------------------------------------------------------------- presets

    /// Generic 2-layer FR4 stackup (upstream default thickness 1.6 mm).
    pub fn default_2layer(thickness_mm: f64) -> Self {
        let dielectric = thickness_mm - 2.0 * COPPER_1OZ.thickness_mm;
        Self {
            layers: vec![
                StackupLayer::copper("F.Cu", COPPER_1OZ.thickness_mm, 1.0),
                StackupLayer::dielectric(
                    "core",
                    dielectric,
                    "FR4",
                    FR4_STANDARD.epsilon_r,
                    FR4_STANDARD.loss_tangent,
                ),
                StackupLayer::copper("B.Cu", COPPER_1OZ.thickness_mm, 1.0),
            ],
            board_thickness_mm: thickness_mm,
            ..Default::default()
        }
    }

    /// Compatibility alias for the historical model (not an orderable stack).
    pub fn jlcpcb_4layer() -> Self {
        Self::jlcpcb_4layer_legacy()
    }

    /// Factory 1.6 mm, 1 oz outer / 0.5 oz inner JLCPCB constructions.
    pub fn jlcpcb_named(identifier: &str) -> PhysResult<Self> {
        let (height, epsilon, core, material) = match identifier {
            "JLC04161H-3313" => (0.0994, 4.1, 1.265, "FR4 3313"),
            "JLC04161H-7628" => (0.2104, 4.4, 1.065, "FR4 7628"),
            _ => {
                return Err(ValueError(format!(
                    "Unsupported factory stackup: {identifier}"
                )))
            }
        };
        let mut stack = Self::jlcpcb_4layer_legacy();
        for i in [1, 5] {
            stack.layers[i].thickness_mm = height;
            stack.layers[i].epsilon_r = epsilon;
            stack.layers[i].material = material.into();
        }
        for i in [2, 4] {
            stack.layers[i].thickness_mm = 0.0152;
        }
        stack.layers[3].thickness_mm = core;
        stack.copper_finish = String::new();
        stack.construction = Some(Construction::Factory {
            id: identifier.into(),
            manufacturer: "jlcpcb".into(),
            source_url: "https://jlcpcb.com/impedance".into(),
            verified_on: "2026-09-10".into(),
            nominal_board_thickness_mm: 1.6,
            outer_copper_oz: 1.0,
            inner_copper_oz: 0.5,
            assumptions: vec![
                "Loss tangent 0.02 is a model assumption, not a sourced factory limit.".into(),
            ],
        });
        Ok(stack)
    }

    /// Historical 0.2104 mm / er=4.05 model; no factory ordering identity.
    pub fn jlcpcb_4layer_legacy() -> Self {
        Self {
            layers: vec![
                StackupLayer::copper("F.Cu", 0.035, 1.0),
                StackupLayer::dielectric("prepreg 1", 0.2104, "FR4 7628", 4.05, 0.02),
                StackupLayer::copper("In1.Cu", 0.0175, 0.5),
                StackupLayer::dielectric("core", 1.065, "FR4", 4.6, 0.02),
                StackupLayer::copper("In2.Cu", 0.0175, 0.5),
                StackupLayer::dielectric("prepreg 2", 0.2104, "FR4 7628", 4.05, 0.02),
                StackupLayer::copper("B.Cu", 0.035, 1.0),
            ],
            board_thickness_mm: 1.6,
            copper_finish: "HASL".into(),
            has_explicit_data: false,
            construction: Some(Construction::Legacy {
                id: "jlcpcb-4-legacy".into(),
            }),
        }
    }

    /// OSH Park 4-layer stackup.
    pub fn oshpark_4layer() -> Self {
        Self {
            layers: vec![
                StackupLayer::copper("F.Cu", 0.035, 1.0),
                StackupLayer::dielectric("prepreg 1", 0.17, "FR408", 4.5, 0.012),
                StackupLayer::copper("In1.Cu", 0.0175, 0.5),
                StackupLayer::dielectric("core", 1.2, "FR408", 4.5, 0.012),
                StackupLayer::copper("In2.Cu", 0.0175, 0.5),
                StackupLayer::dielectric("prepreg 2", 0.17, "FR408", 4.5, 0.012),
                StackupLayer::copper("B.Cu", 0.035, 1.0),
            ],
            board_thickness_mm: 1.6,
            copper_finish: "ENIG".into(),
            ..Default::default()
        }
    }

    /// Generic 6-layer FR4 stackup.
    pub fn default_6layer() -> Self {
        Self {
            layers: vec![
                StackupLayer::copper("F.Cu", 0.035, 1.0),
                StackupLayer::dielectric("prepreg 1", 0.18, "FR4", 4.5, 0.02),
                StackupLayer::copper("In1.Cu", 0.0175, 0.5),
                StackupLayer::dielectric("core 1", 0.36, "FR4", 4.5, 0.02),
                StackupLayer::copper("In2.Cu", 0.0175, 0.5),
                StackupLayer::dielectric("prepreg 2", 0.18, "FR4", 4.5, 0.02),
                StackupLayer::copper("In3.Cu", 0.0175, 0.5),
                StackupLayer::dielectric("core 2", 0.36, "FR4", 4.5, 0.02),
                StackupLayer::copper("In4.Cu", 0.0175, 0.5),
                StackupLayer::dielectric("prepreg 3", 0.18, "FR4", 4.5, 0.02),
                StackupLayer::copper("B.Cu", 0.035, 1.0),
            ],
            board_thickness_mm: 1.6,
            ..Default::default()
        }
    }

    // ------------------------------------------------------------- queries

    pub fn copper_layers(&self) -> Vec<&StackupLayer> {
        self.layers.iter().filter(|l| l.is_copper()).collect()
    }

    fn copper_layer_oz(layer: &StackupLayer) -> Option<f64> {
        if layer.thickness_mm > 0.0 {
            Some(copper_oz_from_thickness(layer.thickness_mm))
        } else {
            layer.copper_weight_oz
        }
    }

    /// `(outer_oz, inner_oz)` from an explicit stackup (thinner weight wins on
    /// disagreement); `None` when the stackup is not explicit or has no copper.
    pub fn outer_inner_copper_oz(&self) -> Option<(Option<f64>, Option<f64>)> {
        if !self.has_explicit_data {
            return None;
        }
        let coppers = self.copper_layers();
        if coppers.is_empty() {
            return None;
        }
        let mut outer_layers = vec![coppers[0]];
        if coppers.len() > 1 {
            outer_layers.push(coppers[coppers.len() - 1]);
        }
        let min = |it: &mut dyn Iterator<Item = f64>| it.reduce(f64::min);
        let outer = min(&mut outer_layers.iter().filter_map(|l| Self::copper_layer_oz(l)));
        let inner_layers: &[&StackupLayer] = if coppers.len() > 2 {
            &coppers[1..coppers.len() - 1]
        } else {
            &[]
        };
        let inner = min(&mut inner_layers.iter().filter_map(|l| Self::copper_layer_oz(l)));
        Some((outer, inner))
    }

    pub fn dielectric_layers(&self) -> Vec<&StackupLayer> {
        self.layers.iter().filter(|l| l.is_dielectric()).collect()
    }

    pub fn num_copper_layers(&self) -> usize {
        self.layers.iter().filter(|l| l.is_copper()).count()
    }

    pub fn get_layer(&self, name: &str) -> Option<&StackupLayer> {
        self.layers.iter().find(|l| l.name == name)
    }

    pub fn get_layer_mut(&mut self, name: &str) -> Option<&mut StackupLayer> {
        self.layers.iter_mut().find(|l| l.name == name)
    }

    /// Index of a layer (0 = top), or -1 when absent.
    pub fn get_layer_index(&self, name: &str) -> isize {
        self.layers
            .iter()
            .position(|l| l.name == name)
            .map_or(-1, |i| i as isize)
    }

    /// First or last copper layer (microstrip geometry).
    pub fn is_outer_layer(&self, layer_name: &str) -> bool {
        let coppers = self.copper_layers();
        match (coppers.first(), coppers.last()) {
            (Some(first), Some(last)) => layer_name == first.name || layer_name == last.name,
            _ => false,
        }
    }

    /// Copper thickness in mm, defaulting to 0.035 (1 oz).
    pub fn get_copper_thickness(&self, layer_name: &str) -> f64 {
        if let Some(layer) = self.get_layer(layer_name) {
            if layer.is_copper() {
                if layer.thickness_mm > 0.0 {
                    return layer.thickness_mm;
                }
                if let Some(oz) = layer.copper_weight_oz.filter(|oz| *oz != 0.0) {
                    return copper_thickness_from_oz(oz);
                }
            }
        }
        0.035
    }

    /// Next dielectric toward the bottom (higher index).
    pub fn get_dielectric_above(&self, layer_name: &str) -> Option<&StackupLayer> {
        let idx = self.get_layer_index(layer_name);
        if idx < 0 {
            return None;
        }
        self.layers[idx as usize + 1..]
            .iter()
            .find(|l| l.is_dielectric())
    }

    /// Next dielectric toward the top (lower index).
    pub fn get_dielectric_below(&self, layer_name: &str) -> Option<&StackupLayer> {
        let idx = self.get_layer_index(layer_name);
        if idx < 0 {
            return None;
        }
        self.layers[..idx as usize]
            .iter()
            .rev()
            .find(|l| l.is_dielectric())
    }

    /// Height from a copper layer to its nearest reference plane.
    pub fn get_dielectric_height(&self, layer_name: &str) -> PhysResult<f64> {
        if self.is_outer_layer(layer_name) {
            if let Some(d) = self.microstrip_dielectric(layer_name) {
                return Ok(d.thickness_mm);
            }
            Ok(0.2)
        } else {
            let (a, b) = self.get_stripline_geometry(layer_name)?;
            Ok(a.min(b))
        }
    }

    fn microstrip_dielectric(&self, layer_name: &str) -> Option<&StackupLayer> {
        let coppers = self.copper_layers();
        if coppers.last().is_some_and(|l| l.name == layer_name) {
            self.get_dielectric_below(layer_name)
        } else {
            self.get_dielectric_above(layer_name)
        }
    }

    /// Dielectric constant for a copper layer (stripline averages neighbours).
    pub fn get_dielectric_constant(&self, layer_name: &str) -> f64 {
        if self.is_outer_layer(layer_name) {
            if let Some(d) = self.microstrip_dielectric(layer_name) {
                if d.epsilon_r > 0.0 {
                    return d.epsilon_r;
                }
            }
        } else {
            let eps: Vec<f64> = [
                self.get_dielectric_above(layer_name),
                self.get_dielectric_below(layer_name),
            ]
            .into_iter()
            .flatten()
            .filter(|d| d.epsilon_r > 0.0)
            .map(|d| d.epsilon_r)
            .collect();
            if !eps.is_empty() {
                return super::py_sum(eps.iter().copied()) / eps.len() as f64;
            }
        }
        FR4_STANDARD.epsilon_r
    }

    pub fn get_loss_tangent(&self, layer_name: &str) -> f64 {
        let d = if self.is_outer_layer(layer_name) {
            self.microstrip_dielectric(layer_name)
        } else {
            self.get_dielectric_above(layer_name)
        };
        match d {
            Some(d) if d.loss_tangent > 0.0 => d.loss_tangent,
            _ => FR4_STANDARD.loss_tangent,
        }
    }

    /// Alias of [`Self::get_dielectric_height`].
    pub fn get_reference_plane_distance(&self, layer_name: &str) -> PhysResult<f64> {
        self.get_dielectric_height(layer_name)
    }

    /// Distances `(h_upper, h_lower)` to both reference planes. Explicit
    /// native `power` roles take precedence over adjacent-copper inference.
    pub fn get_stripline_geometry(&self, layer_name: &str) -> PhysResult<(f64, f64)> {
        if self.is_outer_layer(layer_name) {
            let h = self.get_dielectric_height(layer_name)?;
            return Ok((h, h));
        }
        let explicit_planes = self
            .copper_layers()
            .iter()
            .any(|l| l.copper_role == "power");
        let index = self.get_layer_index(layer_name);
        if index < 0 {
            return Ok((0.2, 0.2));
        }
        let index = index as usize;
        let distance = |up: bool| -> PhysResult<f64> {
            let mut height = 0.0;
            let range: Box<dyn Iterator<Item = usize>> = if up {
                Box::new((0..index).rev())
            } else {
                Box::new(index + 1..self.layers.len())
            };
            for i in range {
                let layer = &self.layers[i];
                if layer.is_copper() && (!explicit_planes || layer.copper_role == "power") {
                    return Ok(height);
                }
                height += layer.thickness_mm;
            }
            if explicit_planes {
                return Err(ValueError(format!(
                    "No declared reference plane on both sides of {layer_name}"
                )));
            }
            Ok(0.2)
        };
        Ok((distance(true)?, distance(false)?))
    }

    pub fn summary(&self) -> StackupSummary {
        StackupSummary {
            construction: self.construction.clone(),
            board_thickness_mm: self.board_thickness_mm,
            num_copper_layers: self.num_copper_layers(),
            copper_finish: self.copper_finish.clone(),
            layers: self
                .layers
                .iter()
                .map(|l| LayerSummary {
                    name: l.name.clone(),
                    layer_type: l.layer_type.value(),
                    thickness_mm: l.thickness_mm,
                    material: l.material.clone(),
                    epsilon_r: l.is_dielectric().then_some(l.epsilon_r),
                    copper_oz: if l.is_copper() {
                        l.copper_weight_oz
                    } else {
                        None
                    },
                })
                .collect(),
        }
    }
}
