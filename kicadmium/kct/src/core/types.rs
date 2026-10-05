//! Canonical enum definitions (port of `kicad_tools.core.types`).
//!
//! String-valued enums (`Severity`, `ErcSeverity`, `RiskLevel`, `Layer`,
//! `LayoutStyle`) serialize to their upstream string values; `CopperLayer`
//! carries upstream's integer stack index.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::severity::SeverityMixin;
use crate::exceptions::ValueError;

macro_rules! str_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $value:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            /// All members in declaration order (`list(Enum)`).
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// Upstream string value (`.value`).
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $value),+
                }
            }

            /// Exact value lookup (`Enum(value)`).
            pub fn from_value(value: &str) -> Option<Self> {
                match value {
                    $($value => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.as_str() == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                $name::from_value(&s).ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "{s:?} is not a valid {}",
                        stringify!($name)
                    ))
                })
            }
        }
    };
}

str_enum! {
    /// Validation severity (DRC, validation, general reporting).
    Severity { Error = "error", Warning = "warning", Info = "info" }
}

impl SeverityMixin for Severity {
    const ERROR: Self = Severity::Error;
    const WARNING: Self = Severity::Warning;
    const INFO: Option<Self> = Some(Severity::Info);
    const MEMBERS: &'static [Self] = Severity::ALL;
}

impl FromStr for Severity {
    type Err = std::convert::Infallible;
    /// Lenient `Severity.from_string` parsing (never fails).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(<Self as SeverityMixin>::from_string(s))
    }
}

/// Alias kept for code that says "ViolationSeverity".
pub type ViolationSeverity = Severity;

str_enum! {
    /// ERC severity with exclusion support (`ERCSeverity`).
    ErcSeverity { Error = "error", Warning = "warning", Exclusion = "exclusion" }
}

/// Upstream spelling.
pub type ERCSeverity = ErcSeverity;

impl SeverityMixin for ErcSeverity {
    const ERROR: Self = ErcSeverity::Error;
    const WARNING: Self = ErcSeverity::Warning;
    const EXCLUSION: Option<Self> = Some(ErcSeverity::Exclusion);
    const MEMBERS: &'static [Self] = ErcSeverity::ALL;
}

impl FromStr for ErcSeverity {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(<Self as SeverityMixin>::from_string(s))
    }
}

str_enum! {
    /// Risk level classification for analysis results.
    RiskLevel { Low = "low", Medium = "medium", High = "high", Critical = "critical" }
}

impl RiskLevel {
    /// Case-insensitive exact match; `default` (else `Low`) otherwise.
    pub fn from_string(s: &str, default: Option<RiskLevel>) -> RiskLevel {
        let lower = s.trim().to_lowercase();
        RiskLevel::from_value(&lower)
            .or(default)
            .unwrap_or(RiskLevel::Low)
    }
}

str_enum! {
    /// PCB layer names (KiCad-compatible string values).
    Layer {
        FCu = "F.Cu",
        BCu = "B.Cu",
        In1Cu = "In1.Cu",
        In2Cu = "In2.Cu",
        In3Cu = "In3.Cu",
        In4Cu = "In4.Cu",
        FMask = "F.Mask",
        BMask = "B.Mask",
        FPaste = "F.Paste",
        BPaste = "B.Paste",
        FSilkS = "F.SilkS",
        BSilkS = "B.SilkS",
        FCrtYd = "F.CrtYd",
        BCrtYd = "B.CrtYd",
        FFab = "F.Fab",
        BFab = "B.Fab",
        EdgeCuts = "Edge.Cuts",
    }
}

impl Layer {
    pub fn is_copper(self) -> bool {
        self.as_str().ends_with(".Cu")
    }

    pub fn is_outer(self) -> bool {
        matches!(self, Layer::FCu | Layer::BCu)
    }

    pub fn is_front(self) -> bool {
        self.as_str().starts_with("F.")
    }

    pub fn is_back(self) -> bool {
        self.as_str().starts_with("B.")
    }

    /// KiCad name to `Layer` (`Layer.from_string`).
    pub fn from_string(name: &str) -> Result<Layer, ValueError> {
        Layer::from_value(name)
            .ok_or_else(|| ValueError::new(format!("Unknown KiCad layer name: {name}")))
    }

    /// Copper layers in stack order (top to bottom).
    pub fn copper_layers() -> [Layer; 6] {
        [
            Layer::FCu,
            Layer::In1Cu,
            Layer::In2Cu,
            Layer::In3Cu,
            Layer::In4Cu,
            Layer::BCu,
        ]
    }
}

impl FromStr for Layer {
    type Err = ValueError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Layer::from_string(s)
    }
}

/// Copper layer indices for routing (0 = top, 5 = bottom of a 6-layer stack).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum CopperLayer {
    FCu = 0,
    In1Cu = 1,
    In2Cu = 2,
    In3Cu = 3,
    In4Cu = 4,
    BCu = 5,
}

impl CopperLayer {
    pub const ALL: &'static [CopperLayer] = &[
        CopperLayer::FCu,
        CopperLayer::In1Cu,
        CopperLayer::In2Cu,
        CopperLayer::In3Cu,
        CopperLayer::In4Cu,
        CopperLayer::BCu,
    ];

    /// Integer value (`.value`).
    pub fn value(self) -> u8 {
        self as u8
    }

    pub fn from_value(value: u8) -> Option<CopperLayer> {
        CopperLayer::ALL.get(value as usize).copied()
    }

    pub fn kicad_name(self) -> &'static str {
        match self {
            CopperLayer::FCu => "F.Cu",
            CopperLayer::In1Cu => "In1.Cu",
            CopperLayer::In2Cu => "In2.Cu",
            CopperLayer::In3Cu => "In3.Cu",
            CopperLayer::In4Cu => "In4.Cu",
            CopperLayer::BCu => "B.Cu",
        }
    }

    pub fn is_outer(self) -> bool {
        matches!(self, CopperLayer::FCu | CopperLayer::BCu)
    }

    pub fn from_kicad_name(name: &str) -> Result<CopperLayer, ValueError> {
        CopperLayer::ALL
            .iter()
            .copied()
            .find(|l| l.kicad_name() == name)
            .ok_or_else(|| ValueError::new(format!("Unknown KiCad copper layer name: {name}")))
    }

    pub fn to_layer(self) -> Layer {
        Layer::from_value(self.kicad_name()).expect("copper layer names are Layer members")
    }
}

impl From<CopperLayer> for u8 {
    fn from(l: CopperLayer) -> u8 {
        l as u8
    }
}

impl TryFrom<u8> for CopperLayer {
    type Error = ValueError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        CopperLayer::from_value(v)
            .ok_or_else(|| ValueError::new(format!("{v} is not a valid CopperLayer")))
    }
}

str_enum! {
    /// Pin layout styles for symbol generation.
    LayoutStyle { Functional = "functional", Physical = "physical", Simple = "simple" }
}
