//! Differential-pair detection from net names (partial port of
//! `kicad_tools.router.diffpair`: signal parsing, pairing and the
//! engagement-layer gate).

use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use super::rules::NetClassRouting;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifferentialPairType {
    Usb2,
    Usb3,
    Ethernet,
    Hdmi,
    Lvds,
    Custom,
}

impl DifferentialPairType {
    pub fn value(self) -> &'static str {
        match self {
            Self::Usb2 => "usb2",
            Self::Usb3 => "usb3",
            Self::Ethernet => "ethernet",
            Self::Hdmi => "hdmi",
            Self::Lvds => "lvds",
            Self::Custom => "custom",
        }
    }

    /// `DifferentialPairRules.for_type` -> `(spacing, max_length_delta,
    /// trace_width, impedance)`.
    pub fn rules(self) -> (f64, f64, f64, f64) {
        match self {
            Self::Usb2 => (0.2, 2.5, 0.2, 90.0),
            Self::Usb3 => (0.15, 0.5, 0.2, 90.0),
            Self::Ethernet => (0.2, 2.0, 0.2, 100.0),
            Self::Hdmi => (0.15, 0.5, 0.2, 100.0),
            Self::Lvds => (0.15, 0.5, 0.15, 100.0),
            Self::Custom => (0.2, 1.0, 0.2, 90.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DifferentialSignal {
    pub net_name: String,
    pub net_id: i64,
    pub base_name: String,
    /// `"P"` or `"N"`.
    pub polarity: &'static str,
    pub notation: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DifferentialPair {
    pub name: String,
    pub positive: DifferentialSignal,
    pub negative: DifferentialSignal,
    pub pair_type: DifferentialPairType,
}

fn ci(p: &str) -> Regex {
    RegexBuilder::new(p).case_insensitive(true).build().unwrap()
}

static PLUS_MINUS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+)([+-])$").unwrap());
static PN_SUFFIX: LazyLock<Regex> = LazyLock::new(|| ci(r"^(.+)_([PN])$"));
static DP_DN: LazyLock<Regex> = LazyLock::new(|| ci(r"^(.+)_(D[PN])$"));
static POS_NEG: LazyLock<Regex> = LazyLock::new(|| ci(r"^(.+)_(POS|NEG)$"));
static POWER_RAIL: LazyLock<Regex> = LazyLock::new(|| {
    ci(
        r"^(VCC|VDD|VBUS|VIN|VOUT|PWR|POWER|AVDD|DVDD|PVDD|PVCC|VBAT|VCORE|VCAP|VIO|VMOTOR|VMOT|VMAIN|VPWR|VDRIVE|VACT|VSRV)(_.*)?$",
    )
});

pub use super::net_class::is_single_ended_refused;

/// `parse_differential_signal` -> `(base, polarity, notation)`.
pub fn parse_differential_signal(net: &str) -> Option<(String, &'static str, &'static str)> {
    if is_single_ended_refused(net) {
        return None;
    }
    if let Some(c) = PLUS_MINUS.captures(net) {
        let pol = if &c[2] == "+" { "P" } else { "N" };
        return Some((c[1].to_string(), pol, "plus_minus"));
    }
    if let Some(c) = DP_DN.captures(net) {
        let pol = if c[2].to_uppercase() == "DP" {
            "P"
        } else {
            "N"
        };
        return Some((format!("{}_D", &c[1]), pol, "pn_suffix"));
    }
    if let Some(c) = PN_SUFFIX.captures(net) {
        let pol = if c[2].to_uppercase() == "P" { "P" } else { "N" };
        return Some((c[1].to_string(), pol, "pn_suffix"));
    }
    if let Some(c) = POS_NEG.captures(net) {
        if POWER_RAIL.is_match(&c[1]) {
            return None;
        }
        let pol = if c[2].to_uppercase() == "POS" {
            "P"
        } else {
            "N"
        };
        return Some((c[1].to_string(), pol, "pos_neg"));
    }
    None
}

pub fn detect_pair_type(base: &str) -> DifferentialPairType {
    let u = base.to_uppercase();
    if u.contains("USB") {
        if u.contains("USB3") || u.contains("SS") {
            return DifferentialPairType::Usb3;
        }
        return DifferentialPairType::Usb2;
    }
    if ["ETH", "ETHERNET", "RGMII", "SGMII", "MDI"]
        .iter()
        .any(|e| u.contains(e))
    {
        return DifferentialPairType::Ethernet;
    }
    if u.contains("HDMI") || u.contains("TMDS") {
        return DifferentialPairType::Hdmi;
    }
    if u.contains("LVDS") {
        return DifferentialPairType::Lvds;
    }
    DifferentialPairType::Custom
}

/// `detect_differential_pairs(net_names)`; `net_names` is the ordered
/// `{net_id: name}` dict.
type SignalGroups = Vec<(
    (String, &'static str),
    Vec<(&'static str, DifferentialSignal)>,
)>;

pub fn detect_differential_pairs(net_names: &[(i64, String)]) -> Vec<DifferentialPair> {
    let mut by_key: SignalGroups = Vec::new();
    for (id, name) in net_names {
        let Some((base, pol, notation)) = parse_differential_signal(name) else {
            continue;
        };
        let sig = DifferentialSignal {
            net_name: name.clone(),
            net_id: *id,
            base_name: base.clone(),
            polarity: pol,
            notation,
        };
        let key = (base, notation);
        let i = match by_key.iter().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                by_key.push((key, Vec::new()));
                by_key.len() - 1
            }
        };
        let m = &mut by_key[i].1;
        match m.iter_mut().find(|(p, _)| *p == pol) {
            Some(e) => e.1 = sig,
            None => m.push((pol, sig)),
        }
    }
    by_key.sort_by(|a, b| a.0.cmp(&b.0));
    let mut pairs = Vec::new();
    for ((base, _), m) in by_key {
        let p = m.iter().find(|(k, _)| *k == "P").map(|(_, s)| s.clone());
        let n = m.iter().find(|(k, _)| *k == "N").map(|(_, s)| s.clone());
        if let (Some(p), Some(n)) = (p, n) {
            pairs.push(DifferentialPair {
                pair_type: detect_pair_type(&base),
                name: base,
                positive: p,
                negative: n,
            });
        }
    }
    pairs
}

/// `should_engage_coupled(pair, net_class_routing, net_to_class)`, with the
/// two-convention `_lookup_net_class` supplied as `lookup`.
pub fn should_engage_coupled<'a>(
    pair: &DifferentialPair,
    lookup: impl Fn(&str) -> Option<&'a NetClassRouting>,
) -> (bool, &'static str) {
    let (p, n) = (&pair.positive.net_name, &pair.negative.net_name);
    if is_single_ended_refused(p) && is_single_ended_refused(n) {
        return (false, "single_ended_refusal");
    }
    let (pc, nc) = (lookup(p), lookup(n));
    if pc.is_none() && nc.is_none() {
        return (false, "no_class_match");
    }
    if pc.is_some_and(|c| c.coupled_routing) || nc.is_some_and(|c| c.coupled_routing) {
        return (true, "engaged");
    }
    (false, "opt_in_disabled")
}
