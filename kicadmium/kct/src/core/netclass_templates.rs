//! Netclass templates for common design types (port of
//! `kicad_tools.core.netclass_templates`).
//!
//! Predefined netclass configurations (audio, power supply, digital,
//! mixed-signal, RF) with trace widths, clearances, and net-name patterns,
//! applied to `.kicad_pro` JSON (`serde_json::Value`).

use serde_json::{json, Map, Value};

use crate::exceptions::ValueError;

/// Template for a single netclass.
#[derive(Debug, Clone, PartialEq)]
pub struct NetclassTemplate {
    pub name: &'static str,
    pub track_width: f64,
    pub clearance: f64,
    pub via_diameter: f64,
    pub via_drill: f64,
    pub pcb_color: Option<&'static str>,
    pub patterns: &'static [&'static str],
}

/// Template for a complete design type.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignTypeTemplate {
    pub name: &'static str,
    pub description: &'static str,
    pub netclasses: &'static [NetclassTemplate],
}

/// Netclass color palette (name, KiCad RGBA string).
pub const NETCLASS_COLORS: &[(&str, &str)] = &[
    ("Power", "rgba(255, 0, 0, 0.800)"),
    ("Ground", "rgba(139, 69, 19, 0.800)"),
    ("Audio", "rgba(0, 128, 0, 0.800)"),
    ("Clock", "rgba(255, 165, 0, 0.800)"),
    ("I2S", "rgba(0, 191, 255, 0.800)"),
    ("SPI", "rgba(138, 43, 226, 0.800)"),
    ("I2C", "rgba(255, 20, 147, 0.800)"),
    ("HighSpeed", "rgba(255, 215, 0, 0.800)"),
    ("Debug", "rgba(128, 128, 128, 0.800)"),
    ("Control", "rgba(0, 128, 128, 0.800)"),
    ("HighCurrent", "rgba(178, 34, 34, 0.800)"),
    ("Analog", "rgba(46, 139, 87, 0.800)"),
    ("RF", "rgba(255, 140, 0, 0.800)"),
];

/// `AUDIO_TEMPLATE`.
pub static AUDIO_TEMPLATE: DesignTypeTemplate = DesignTypeTemplate {
    name: "audio",
    description: "Audio DAC/ADC design with I2S and analog paths",
    netclasses: &[
        NetclassTemplate {
            name: "Power",
            track_width: 0.4,
            clearance: 0.15,
            via_diameter: 0.8,
            via_drill: 0.4,
            pcb_color: Some("rgba(255, 0, 0, 0.800)"),
            patterns: &["VCC*", "VDD*", "+*V", "V_*", "PWR_*", "VBUS*"],
        },
        NetclassTemplate {
            name: "Ground",
            track_width: 0.5,
            clearance: 0.15,
            via_diameter: 0.8,
            via_drill: 0.4,
            pcb_color: Some("rgba(139, 69, 19, 0.800)"),
            patterns: &["GND", "GND*", "AGND*", "DGND*", "PGND*", "*_GND"],
        },
        NetclassTemplate {
            name: "Audio",
            track_width: 0.3,
            clearance: 0.2,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(0, 128, 0, 0.800)"),
            patterns: &[
                "AUDIO_*", "AUD_*", "DAC_*", "ADC_*", "LINE_*", "HP_*", "SPK_*",
            ],
        },
        NetclassTemplate {
            name: "I2S",
            track_width: 0.2,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(0, 191, 255, 0.800)"),
            patterns: &[
                "I2S_*", "I2S*", "BCLK*", "LRCLK*", "MCLK*", "SDATA*", "DOUT*", "DIN*",
            ],
        },
        NetclassTemplate {
            name: "Clock",
            track_width: 0.2,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(255, 165, 0, 0.800)"),
            patterns: &["CLK*", "*_CLK", "OSC*", "XTAL*"],
        },
        NetclassTemplate {
            name: "SPI",
            track_width: 0.2,
            clearance: 0.1,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(138, 43, 226, 0.800)"),
            patterns: &["SPI_*", "MOSI*", "MISO*", "SCK*", "CS_*", "SS_*"],
        },
        NetclassTemplate {
            name: "I2C",
            track_width: 0.2,
            clearance: 0.1,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(255, 20, 147, 0.800)"),
            patterns: &["I2C_*", "SDA*", "SCL*"],
        },
        NetclassTemplate {
            name: "Debug",
            track_width: 0.2,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(128, 128, 128, 0.800)"),
            patterns: &[
                "SWDIO*", "SWCLK*", "NRST*", "TDI*", "TDO*", "TCK*", "TMS*", "JTAG_*",
            ],
        },
    ],
};

/// `POWER_SUPPLY_TEMPLATE`.
pub static POWER_SUPPLY_TEMPLATE: DesignTypeTemplate = DesignTypeTemplate {
    name: "power_supply",
    description: "Power supply design with high current paths",
    netclasses: &[
        NetclassTemplate {
            name: "HighCurrent",
            track_width: 0.8,
            clearance: 0.2,
            via_diameter: 1.0,
            via_drill: 0.5,
            pcb_color: Some("rgba(178, 34, 34, 0.800)"),
            patterns: &["VIN*", "VOUT*", "SW*", "PHASE*", "BOOST*", "BUCK*"],
        },
        NetclassTemplate {
            name: "Power",
            track_width: 0.5,
            clearance: 0.15,
            via_diameter: 0.8,
            via_drill: 0.4,
            pcb_color: Some("rgba(255, 0, 0, 0.800)"),
            patterns: &["VCC*", "VDD*", "+*V", "V_*", "VBUS*", "VREG*"],
        },
        NetclassTemplate {
            name: "Ground",
            track_width: 0.8,
            clearance: 0.2,
            via_diameter: 1.0,
            via_drill: 0.5,
            pcb_color: Some("rgba(139, 69, 19, 0.800)"),
            patterns: &["GND*", "PGND*", "AGND*", "*_GND"],
        },
        NetclassTemplate {
            name: "Control",
            track_width: 0.2,
            clearance: 0.1,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(0, 128, 128, 0.800)"),
            patterns: &["FB_*", "EN_*", "COMP*", "SS_*", "PGOOD*", "FAULT*"],
        },
        NetclassTemplate {
            name: "Analog",
            track_width: 0.25,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(46, 139, 87, 0.800)"),
            patterns: &["SENSE*", "ISENSE*", "VSENSE*", "REF*"],
        },
    ],
};

/// `DIGITAL_TEMPLATE`.
pub static DIGITAL_TEMPLATE: DesignTypeTemplate = DesignTypeTemplate {
    name: "digital",
    description: "Digital design with high-speed signals",
    netclasses: &[
        NetclassTemplate {
            name: "Power",
            track_width: 0.4,
            clearance: 0.15,
            via_diameter: 0.8,
            via_drill: 0.4,
            pcb_color: Some("rgba(255, 0, 0, 0.800)"),
            patterns: &["VCC*", "VDD*", "+*V", "V_*", "VCORE*", "VIO*"],
        },
        NetclassTemplate {
            name: "Ground",
            track_width: 0.5,
            clearance: 0.15,
            via_diameter: 0.8,
            via_drill: 0.4,
            pcb_color: Some("rgba(139, 69, 19, 0.800)"),
            patterns: &["GND*", "DGND*", "*_GND"],
        },
        NetclassTemplate {
            name: "HighSpeed",
            track_width: 0.2,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(255, 215, 0, 0.800)"),
            patterns: &["USB_*", "HDMI_*", "ETH_*", "LVDS_*", "DP_*", "DDR_*"],
        },
        NetclassTemplate {
            name: "Clock",
            track_width: 0.2,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(255, 165, 0, 0.800)"),
            patterns: &["CLK*", "*_CLK", "OSC*", "XTAL*", "REF_CLK*"],
        },
        NetclassTemplate {
            name: "SPI",
            track_width: 0.2,
            clearance: 0.1,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(138, 43, 226, 0.800)"),
            patterns: &["SPI_*", "MOSI*", "MISO*", "SCK*", "CS_*", "QSPI_*"],
        },
        NetclassTemplate {
            name: "I2C",
            track_width: 0.2,
            clearance: 0.1,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(255, 20, 147, 0.800)"),
            patterns: &["I2C_*", "SDA*", "SCL*"],
        },
        NetclassTemplate {
            name: "Debug",
            track_width: 0.2,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(128, 128, 128, 0.800)"),
            patterns: &[
                "SWDIO*", "SWCLK*", "NRST*", "TDI*", "TDO*", "TCK*", "TMS*", "JTAG_*", "UART_*",
                "TX*", "RX*",
            ],
        },
    ],
};

/// `MIXED_SIGNAL_TEMPLATE`.
pub static MIXED_SIGNAL_TEMPLATE: DesignTypeTemplate = DesignTypeTemplate {
    name: "mixed_signal",
    description: "Mixed-signal design with analog and digital domains",
    netclasses: &[
        NetclassTemplate {
            name: "Power",
            track_width: 0.4,
            clearance: 0.15,
            via_diameter: 0.8,
            via_drill: 0.4,
            pcb_color: Some("rgba(255, 0, 0, 0.800)"),
            patterns: &["VCC*", "VDD*", "+*V", "V_*", "AVDD*", "DVDD*"],
        },
        NetclassTemplate {
            name: "Ground",
            track_width: 0.5,
            clearance: 0.15,
            via_diameter: 0.8,
            via_drill: 0.4,
            pcb_color: Some("rgba(139, 69, 19, 0.800)"),
            patterns: &["GND", "GND*", "AGND*", "DGND*", "*_GND"],
        },
        NetclassTemplate {
            name: "Analog",
            track_width: 0.3,
            clearance: 0.2,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(46, 139, 87, 0.800)"),
            patterns: &["AIN*", "AOUT*", "ADC_*", "DAC_*", "VREF*", "SENSE*"],
        },
        NetclassTemplate {
            name: "Clock",
            track_width: 0.2,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(255, 165, 0, 0.800)"),
            patterns: &["CLK*", "*_CLK", "OSC*", "XTAL*"],
        },
        NetclassTemplate {
            name: "SPI",
            track_width: 0.2,
            clearance: 0.1,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(138, 43, 226, 0.800)"),
            patterns: &["SPI_*", "MOSI*", "MISO*", "SCK*", "CS_*"],
        },
        NetclassTemplate {
            name: "I2C",
            track_width: 0.2,
            clearance: 0.1,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(255, 20, 147, 0.800)"),
            patterns: &["I2C_*", "SDA*", "SCL*"],
        },
        NetclassTemplate {
            name: "Debug",
            track_width: 0.2,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(128, 128, 128, 0.800)"),
            patterns: &[
                "SWDIO*", "SWCLK*", "NRST*", "TDI*", "TDO*", "TCK*", "TMS*", "JTAG_*",
            ],
        },
    ],
};

/// `RF_TEMPLATE`.
pub static RF_TEMPLATE: DesignTypeTemplate = DesignTypeTemplate {
    name: "rf",
    description: "RF design with controlled impedance traces",
    netclasses: &[
        NetclassTemplate {
            name: "Power",
            track_width: 0.4,
            clearance: 0.15,
            via_diameter: 0.8,
            via_drill: 0.4,
            pcb_color: Some("rgba(255, 0, 0, 0.800)"),
            patterns: &["VCC*", "VDD*", "+*V", "V_*", "VPA*", "VRF*"],
        },
        NetclassTemplate {
            name: "Ground",
            track_width: 0.5,
            clearance: 0.15,
            via_diameter: 0.8,
            via_drill: 0.4,
            pcb_color: Some("rgba(139, 69, 19, 0.800)"),
            patterns: &["GND*", "RFGND*", "*_GND"],
        },
        NetclassTemplate {
            name: "RF",
            track_width: 0.35,
            clearance: 0.25,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(255, 140, 0, 0.800)"),
            patterns: &["RF_*", "ANT*", "LNA_*", "PA_*", "MIX_*", "LO_*", "IF_*"],
        },
        NetclassTemplate {
            name: "Clock",
            track_width: 0.2,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(255, 165, 0, 0.800)"),
            patterns: &["CLK*", "*_CLK", "OSC*", "XTAL*", "TCXO*", "REFCLK*"],
        },
        NetclassTemplate {
            name: "Control",
            track_width: 0.2,
            clearance: 0.1,
            via_diameter: 0.6,
            via_drill: 0.3,
            pcb_color: Some("rgba(0, 128, 128, 0.800)"),
            patterns: &["SPI_*", "I2C_*", "EN_*", "CTRL_*"],
        },
    ],
};

/// Registry of all templates, in upstream order.
pub static DESIGN_TYPE_TEMPLATES: [(&str, &DesignTypeTemplate); 5] = [
    ("audio", &AUDIO_TEMPLATE),
    ("power_supply", &POWER_SUPPLY_TEMPLATE),
    ("digital", &DIGITAL_TEMPLATE),
    ("mixed_signal", &MIXED_SIGNAL_TEMPLATE),
    ("rf", &RF_TEMPLATE),
];

/// Color for a palette entry.
pub fn netclass_color(name: &str) -> Option<&'static str> {
    NETCLASS_COLORS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
}

/// Available design type names.
pub fn get_available_design_types() -> Vec<&'static str> {
    DESIGN_TYPE_TEMPLATES.iter().map(|(k, _)| *k).collect()
}

/// Template by name.
pub fn get_design_template(design_type: &str) -> Result<&'static DesignTypeTemplate, ValueError> {
    DESIGN_TYPE_TEMPLATES
        .iter()
        .find(|(k, _)| *k == design_type)
        .map(|(_, t)| *t)
        .ok_or_else(|| {
            ValueError::new(format!(
                "Unknown design type '{design_type}'. Available: {}",
                get_available_design_types().join(", ")
            ))
        })
}

/// Add the template's netclasses and patterns to project data (and, with
/// `update_default`, reset `Default` to 0.2/0.15/0.6/0.3).
pub fn apply_design_template(
    data: &mut Value,
    design_type: &str,
    update_default: bool,
) -> Result<(), ValueError> {
    let template = get_design_template(design_type)?;
    if update_default {
        project_file::add_netclass_definition(data, "Default", 0.2, 0.15, 0.6, 0.3, None);
    }
    for nc in template.netclasses {
        project_file::add_netclass_definition(
            data,
            nc.name,
            nc.track_width,
            nc.clearance,
            nc.via_diameter,
            nc.via_drill,
            nc.pcb_color,
        );
        if !nc.patterns.is_empty() {
            project_file::add_netclass_patterns(data, nc.name, nc.patterns);
        }
    }
    Ok(())
}

/// Per-netclass summary rows: name, track_width, clearance, via_diameter,
/// pattern_count.
pub fn get_netclass_summary(data: &mut Value) -> Vec<Value> {
    let patterns = project_file::get_netclass_patterns(data).clone();
    let classes = project_file::get_netclass_definitions(data).clone();
    let count = |name: &str| {
        patterns
            .iter()
            .filter(|p| p.get("netclass").and_then(Value::as_str).unwrap_or("") == name)
            .count()
    };
    classes
        .iter()
        .map(|cls| {
            let get = |k: &str, d: Value| cls.get(k).cloned().unwrap_or(d);
            let name_key = cls.get("name").and_then(Value::as_str).unwrap_or("");
            let mut row = Map::new();
            row.insert("name".into(), get("name", json!("Unknown")));
            row.insert("track_width".into(), get("track_width", json!(0.25)));
            row.insert("clearance".into(), get("clearance", json!(0.2)));
            row.insert("via_diameter".into(), get("via_diameter", json!(0.6)));
            row.insert("pattern_count".into(), json!(count(name_key)));
            Value::Object(row)
        })
        .collect()
}

/// Minimal mirror of the `core.project_file` netclass helpers that
/// `apply_design_template` needs (same semantics as upstream
/// `get_net_settings` / `add_netclass_definition` / `add_netclass_pattern`).
/// Kept local so this module does not depend on the concurrently-ported
/// `core::project_file`; switch to it once available.
pub mod project_file {
    use serde_json::{json, Map, Value};

    pub const DEFAULT_NETCLASS_CLEARANCE_MM: f64 = 0.15;

    pub fn default_netclass_definition() -> Value {
        json!({
            "bus_width": 12,
            "clearance": DEFAULT_NETCLASS_CLEARANCE_MM,
            "diff_pair_gap": 0.25,
            "diff_pair_via_gap": 0.25,
            "diff_pair_width": 0.2,
            "line_style": 0,
            "microvia_diameter": 0.3,
            "microvia_drill": 0.1,
            "name": "Default",
            "pcb_color": "rgba(0, 0, 0, 0.000)",
            "schematic_color": "rgba(0, 0, 0, 0.000)",
            "track_width": 0.25,
            "via_diameter": 0.6,
            "via_drill": 0.3,
            "wire_width": 6
        })
    }

    fn object(data: &mut Value) -> &mut Map<String, Value> {
        if !data.is_object() {
            *data = Value::Object(Map::new());
        }
        data.as_object_mut().expect("object")
    }

    pub fn get_net_settings(data: &mut Value) -> &mut Map<String, Value> {
        let root = object(data);
        let settings = root.entry("net_settings").or_insert_with(|| {
            json!({
                "classes": [default_netclass_definition()],
                "meta": {"version": 3},
                "net_colors": null,
                "netclass_assignments": null,
                "netclass_patterns": [],
            })
        });
        object(settings)
    }

    fn list<'a>(
        settings: &'a mut Map<String, Value>,
        key: &str,
        init: impl FnOnce() -> Vec<Value>,
    ) -> &'a mut Vec<Value> {
        let entry = settings.entry(key).or_insert_with(|| Value::Array(init()));
        if !entry.is_array() {
            *entry = Value::Array(Vec::new());
        }
        entry.as_array_mut().expect("array")
    }

    pub fn get_netclass_definitions(data: &mut Value) -> &mut Vec<Value> {
        list(get_net_settings(data), "classes", || {
            vec![default_netclass_definition()]
        })
    }

    pub fn get_netclass_patterns(data: &mut Value) -> &mut Vec<Value> {
        list(get_net_settings(data), "netclass_patterns", Vec::new)
    }

    pub fn create_netclass_definition(
        name: &str,
        track_width: f64,
        clearance: f64,
        via_diameter: f64,
        via_drill: f64,
        pcb_color: Option<&str>,
    ) -> Value {
        let mut def = default_netclass_definition();
        let obj = def.as_object_mut().expect("object");
        obj.insert("name".into(), json!(name));
        obj.insert("track_width".into(), json!(track_width));
        obj.insert("clearance".into(), json!(clearance));
        obj.insert("via_diameter".into(), json!(via_diameter));
        obj.insert("via_drill".into(), json!(via_drill));
        obj.insert("diff_pair_width".into(), json!(0.2));
        obj.insert("diff_pair_gap".into(), json!(0.25));
        if let Some(color) = pcb_color {
            obj.insert("pcb_color".into(), json!(color));
        }
        def
    }

    pub fn add_netclass_definition(
        data: &mut Value,
        name: &str,
        track_width: f64,
        clearance: f64,
        via_diameter: f64,
        via_drill: f64,
        pcb_color: Option<&str>,
    ) {
        let def = create_netclass_definition(
            name,
            track_width,
            clearance,
            via_diameter,
            via_drill,
            pcb_color,
        );
        let classes = get_netclass_definitions(data);
        match classes
            .iter_mut()
            .find(|c| c.get("name").and_then(Value::as_str) == Some(name))
        {
            Some(existing) => *existing = def,
            None => classes.push(def),
        }
    }

    pub fn add_netclass_patterns(data: &mut Value, netclass: &str, patterns: &[&str]) {
        let list = get_netclass_patterns(data);
        for pattern in patterns {
            let exists = list.iter().any(|p| {
                p.get("netclass").and_then(Value::as_str) == Some(netclass)
                    && p.get("pattern").and_then(Value::as_str) == Some(*pattern)
            });
            if !exists {
                list.push(json!({"netclass": netclass, "pattern": pattern}));
            }
        }
    }
}
