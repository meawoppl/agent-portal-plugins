//! Fabricator profiles and design-rule presets.
//!
//! This is the native Rust counterpart of `kicad_tools.manufacturers`.  The
//! published upstream YAML and `.kicad_dru` presets are embedded so querying a
//! profile never depends on a Python installation or runtime data directory.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DesignRules {
    pub min_trace_width_mm: f64,
    pub min_clearance_mm: f64,
    pub min_via_drill_mm: f64,
    pub min_via_diameter_mm: f64,
    pub min_annular_ring_mm: f64,
    #[serde(default = "default_min_hole")]
    pub min_hole_diameter_mm: f64,
    #[serde(default = "default_max_hole")]
    pub max_hole_diameter_mm: f64,
    #[serde(default = "default_copper_edge")]
    pub min_copper_to_edge_mm: f64,
    #[serde(default = "default_hole_edge")]
    pub min_hole_to_edge_mm: f64,
    #[serde(default = "default_hole_hole")]
    pub min_hole_to_hole_mm: f64,
    #[serde(default = "default_silk_width")]
    pub min_silkscreen_width_mm: f64,
    #[serde(default = "default_silk_height")]
    pub min_silkscreen_height_mm: f64,
    #[serde(default = "default_mask_dam")]
    pub min_solder_mask_dam_mm: f64,
    #[serde(default = "default_mask_clearance")]
    pub min_solder_mask_clearance_mm: f64,
    #[serde(default)]
    pub min_silk_to_pad_clearance_mm: Option<f64>,
    #[serde(default)]
    pub min_smd_pad_clearance_mm: Option<f64>,
    #[serde(default)]
    pub min_pth_hole_to_track_mm: Option<f64>,
    #[serde(default)]
    pub min_inner_pth_hole_to_copper_mm: Option<f64>,
    #[serde(default)]
    pub min_pth_annular_ring_mm: Option<f64>,
    #[serde(default = "default_pad_size")]
    pub min_pad_size_mm: f64,
    #[serde(default = "default_thickness")]
    pub board_thickness_mm: f64,
    #[serde(default = "default_outer_copper")]
    pub outer_copper_oz: f64,
    #[serde(default = "default_inner_copper")]
    pub inner_copper_oz: f64,
    #[serde(default = "default_board_width")]
    pub max_board_width_mm: f64,
    #[serde(default = "default_board_height")]
    pub max_board_height_mm: f64,
    #[serde(default)]
    pub via_in_pad_supported: bool,
    #[serde(default)]
    pub via_in_pad_process_id: Option<String>,
}

macro_rules! default_fn {
    ($name:ident, $value:expr) => {
        fn $name() -> f64 {
            $value
        }
    };
}
default_fn!(default_min_hole, 0.3);
default_fn!(default_max_hole, 6.3);
default_fn!(default_copper_edge, 0.3);
default_fn!(default_hole_edge, 0.5);
default_fn!(default_hole_hole, 0.5);
default_fn!(default_silk_width, 0.15);
default_fn!(default_silk_height, 0.8);
default_fn!(default_mask_dam, 0.1);
default_fn!(default_mask_clearance, 0.05);
default_fn!(default_pad_size, 0.25);
default_fn!(default_thickness, 1.6);
default_fn!(default_outer_copper, 1.0);
default_fn!(default_inner_copper, 0.5);
default_fn!(default_board_width, 400.0);
default_fn!(default_board_height, 500.0);

impl DesignRules {
    pub fn min_trace_width_mil(&self) -> f64 {
        self.min_trace_width_mm / 0.0254
    }
    pub fn min_clearance_mil(&self) -> f64 {
        self.min_clearance_mm / 0.0254
    }
}

#[derive(Debug, Deserialize)]
struct RulesFile {
    design_rules: BTreeMap<String, DesignRules>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ManufacturerProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub website: &'static str,
    pub supported_layers: &'static [u8],
    pub bom_format: &'static str,
    pub pricing_model: &'static str,
    pub supports_assembly: bool,
    pub parts_library: Option<&'static str>,
}

const PROFILES: &[ManufacturerProfile] = &[
    ManufacturerProfile {
        id: "flashpcb",
        name: "FlashPCB",
        website: "https://flashpcb.com",
        supported_layers: &[2, 4],
        bom_format: "generic",
        pricing_model: "per_pcb",
        supports_assembly: true,
        parts_library: None,
    },
    ManufacturerProfile {
        id: "jlcpcb",
        name: "JLCPCB",
        website: "https://jlcpcb.com",
        supported_layers: &[1, 2, 4, 6],
        bom_format: "jlcpcb",
        pricing_model: "per_pcb",
        supports_assembly: true,
        parts_library: Some("LCSC"),
    },
    ManufacturerProfile {
        id: "jlcpcb-tier1",
        name: "JLCPCB Capability Plus",
        website: "https://jlcpcb.com",
        supported_layers: &[2, 4, 6],
        bom_format: "jlcpcb",
        pricing_model: "per_pcb",
        supports_assembly: true,
        parts_library: Some("LCSC"),
    },
    ManufacturerProfile {
        id: "seeed",
        name: "Seeed Fusion",
        website: "https://www.seeedstudio.com/fusion.html",
        supported_layers: &[1, 2, 4, 6],
        bom_format: "seeed",
        pricing_model: "per_pcb",
        supports_assembly: true,
        parts_library: Some("Seeed OPL"),
    },
    ManufacturerProfile {
        id: "pcbway",
        name: "PCBWay",
        website: "https://www.pcbway.com",
        supported_layers: &[1, 2, 4, 6],
        bom_format: "pcbway",
        pricing_model: "per_pcb",
        supports_assembly: true,
        parts_library: Some("Global Sourcing"),
    },
    ManufacturerProfile {
        id: "oshpark",
        name: "OSHPark",
        website: "https://oshpark.com",
        supported_layers: &[2, 4],
        bom_format: "generic",
        pricing_model: "per_sqin",
        supports_assembly: false,
        parts_library: None,
    },
];

pub fn canonical_id(id: &str) -> String {
    let normalized = id.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "flash" => "flashpcb",
        "jlc" | "lcsc" => "jlcpcb",
        "seeed_fusion" | "seeed-fusion" | "seeedfusion" | "seeedstudio" => "seeed",
        "osh" | "osh_park" => "oshpark",
        "jlcpcb_tier1"
        | "jlcpcb-capabilityplus"
        | "jlcpcb_capabilityplus"
        | "jlcpcb-capability-plus" => "jlcpcb-tier1",
        _ => return normalized,
    }
    .to_owned()
}

pub fn fab_family(id: &str) -> String {
    let id = canonical_id(id);
    if id == "jlcpcb-tier1" {
        "jlcpcb".into()
    } else {
        id
    }
}

pub fn profiles() -> &'static [ManufacturerProfile] {
    PROFILES
}

pub fn profile(id: &str) -> Result<&'static ManufacturerProfile> {
    let id = canonical_id(id);
    PROFILES.iter().find(|p| p.id == id).with_context(|| {
        let names = PROFILES.iter().map(|p| p.id).collect::<Vec<_>>().join(", ");
        format!("unknown manufacturer {id:?}; available: {names}")
    })
}

fn yaml_for(id: &str) -> Result<&'static str> {
    match canonical_id(id).as_str() {
        "flashpcb" => Ok(include_str!("data/flashpcb.yaml")),
        "jlcpcb" => Ok(include_str!("data/jlcpcb.yaml")),
        "jlcpcb-tier1" => Ok(include_str!("data/jlcpcb_tier1.yaml")),
        "oshpark" => Ok(include_str!("data/oshpark.yaml")),
        "pcbway" => Ok(include_str!("data/pcbway.yaml")),
        "seeed" => Ok(include_str!("data/seeed.yaml")),
        other => bail!("unknown manufacturer {other:?}"),
    }
}

pub fn design_rules(id: &str) -> Result<BTreeMap<String, DesignRules>> {
    Ok(serde_yaml::from_str::<RulesFile>(yaml_for(id)?)?.design_rules)
}

pub fn rules(id: &str, layers: u8, copper_oz: f64) -> Result<DesignRules> {
    let all = design_rules(id)?;
    let exact = format!("{layers}layer_{copper_oz:.0}oz");
    let one_oz = format!("{layers}layer_1oz");
    all.get(&exact)
        .or_else(|| all.get(&one_oz))
        .or_else(|| all.get("2layer_1oz"))
        .or_else(|| all.values().next())
        .cloned()
        .context("manufacturer profile has no design rules")
}

pub fn dru_preset(id: &str, layers: u8, copper_oz: f64) -> Result<&'static str> {
    let family = fab_family(id);
    let name = format!("{family}-{layers}layer-{copper_oz:.0}oz");
    match name.as_str() {
        "flashpcb-2layer-1oz" => Ok(include_str!("rules/flashpcb-2layer-1oz.kicad_dru")),
        "flashpcb-2layer-2oz" => Ok(include_str!("rules/flashpcb-2layer-2oz.kicad_dru")),
        "flashpcb-4layer-1oz" => Ok(include_str!("rules/flashpcb-4layer-1oz.kicad_dru")),
        "flashpcb-4layer-2oz" => Ok(include_str!("rules/flashpcb-4layer-2oz.kicad_dru")),
        "jlcpcb-2layer-1oz" => Ok(include_str!("rules/jlcpcb-2layer-1oz.kicad_dru")),
        "jlcpcb-2layer-2oz" => Ok(include_str!("rules/jlcpcb-2layer-2oz.kicad_dru")),
        "jlcpcb-4layer-1oz" => Ok(include_str!("rules/jlcpcb-4layer-1oz.kicad_dru")),
        "jlcpcb-4layer-2oz" => Ok(include_str!("rules/jlcpcb-4layer-2oz.kicad_dru")),
        "jlcpcb-6layer-1oz" => Ok(include_str!("rules/jlcpcb-6layer-1oz.kicad_dru")),
        "oshpark-2layer-1oz" => Ok(include_str!("rules/oshpark-2layer.kicad_dru")),
        "oshpark-4layer-1oz" => Ok(include_str!("rules/oshpark-4layer-1oz.kicad_dru")),
        "pcbway-2layer-1oz" => Ok(include_str!("rules/pcbway-2layer-1oz.kicad_dru")),
        "pcbway-4layer-1oz" => Ok(include_str!("rules/pcbway-4layer-1oz.kicad_dru")),
        "pcbway-6layer-1oz" => Ok(include_str!("rules/pcbway-6layer-1oz.kicad_dru")),
        "seeed-2layer-1oz" => Ok(include_str!("rules/seeed-2layer-1oz.kicad_dru")),
        "seeed-4layer-1oz" => Ok(include_str!("rules/seeed-4layer-1oz.kicad_dru")),
        "seeed-6layer-1oz" => Ok(include_str!("rules/seeed-6layer-1oz.kicad_dru")),
        _ => bail!("no bundled DRU preset for {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_and_family_match_upstream() {
        assert_eq!(canonical_id("JLC"), "jlcpcb");
        assert_eq!(canonical_id("jlcpcb_capabilityplus"), "jlcpcb-tier1");
        assert_eq!(fab_family("jlcpcb-tier1"), "jlcpcb");
    }
    #[test]
    fn every_profile_has_rules() {
        for p in profiles() {
            assert!(!design_rules(p.id).unwrap().is_empty(), "{}", p.id);
        }
    }
    #[test]
    fn exact_and_fallback_selection() {
        assert_eq!(rules("jlc", 4, 1.0).unwrap().outer_copper_oz, 1.0);
        assert!(rules("oshpark", 6, 2.0).is_ok());
    }
    #[test]
    fn presets_are_embedded() {
        assert!(dru_preset("jlcpcb", 4, 1.0).unwrap().contains("version"));
    }
}
