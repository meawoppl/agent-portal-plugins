//! Trace impedance control (port of
//! `kicad_tools.validate.rules.impedance`, issues #2650 / #2696 / #3157).
//!
//! Default (heuristic) specs only apply on boards that opt into controlled
//! impedance, and their single-ended name patterns are suppressed; explicit
//! specs (e.g. derived from a net-class map) always evaluate.

use regex::{Regex, RegexBuilder};

use crate::manufacturers::DesignRules;
use crate::physics::coupled_lines::CoupledLines;
use crate::physics::stackup::Stackup;
use crate::physics::transmission_line::TransmissionLine;
use crate::router::diffpair::DifferentialPair;
use crate::router::kelvin::{find_kelvin_root, is_sense_net_name, KelvinPad};
use crate::schema::pcb::Pcb;
use crate::utils::pymath::hypot;
use crate::validate::violations::{DRCResults, DRCViolation};

/// Impedance specification for a net or net class.
#[derive(Debug, Clone, PartialEq)]
pub struct NetImpedanceSpec {
    pub net_pattern: String,
    pub target_z0: Option<f64>,
    pub target_zdiff: Option<f64>,
    pub tolerance_percent: f64,
    pub exclude_current_sense: bool,
}

impl NetImpedanceSpec {
    pub fn new(net_pattern: impl Into<String>) -> Self {
        NetImpedanceSpec {
            net_pattern: net_pattern.into(),
            target_z0: None,
            target_zdiff: None,
            tolerance_percent: 10.0,
            exclude_current_sense: false,
        }
    }

    fn regex(&self) -> Option<Regex> {
        // `re.match`: anchored at the start only.
        RegexBuilder::new(&format!("^(?:{})", self.net_pattern))
            .case_insensitive(true)
            .build()
            .ok()
    }

    pub fn matches(&self, net_name: &str, has_kelvin_root: bool) -> bool {
        if self.exclude_current_sense && has_kelvin_root && is_sense_net_name(net_name) {
            return false;
        }
        self.regex().is_some_and(|r| r.is_match(net_name))
    }
}

/// Result of checking one trace.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpedanceCheckResult {
    pub net_name: String,
    pub layer: String,
    pub width_mm: f64,
    pub calculated_z0: f64,
    pub target_z0: f64,
    pub deviation_percent: f64,
    pub compliant: bool,
}

const SE_HEURISTIC_PATTERNS: [&str; 3] = [r".*CLK$", r".*MCLK$", r".*ETH.*"];

/// Upstream `ImpedanceRule`. `specs: None` selects the default specs.
#[derive(Debug, Clone, Default)]
pub struct ImpedanceRule {
    pub specs: Option<Vec<NetImpedanceSpec>>,
    pub stackup: Option<Stackup>,
    pub detected_pairs: Vec<DifferentialPair>,
}

pub const NECK_DOWN_EXEMPT_LENGTH_MM: f64 = 1.0;

pub fn default_specs() -> Vec<NetImpedanceSpec> {
    let diff = |p: &str, z: f64, ex: bool| NetImpedanceSpec {
        target_zdiff: Some(z),
        exclude_current_sense: ex,
        ..NetImpedanceSpec::new(p)
    };
    let se = |p: &str| NetImpedanceSpec {
        target_z0: Some(50.0),
        ..NetImpedanceSpec::new(p)
    };
    vec![
        diff(r"USB.*D[PM\+\-]?", 90.0, false),
        diff(r".*LVDS.*", 100.0, false),
        diff(r".*_[PN]$", 100.0, true),
        diff(r".*[+\-]$", 100.0, true),
        se(r".*CLK$"),
        se(r".*MCLK$"),
        se(r".*ETH.*"),
    ]
}

struct Trace {
    net_name: String,
    width_mm: f64,
    layer: String,
    start: (f64, f64),
    end: (f64, f64),
}

struct Physics {
    stackup: Stackup,
    tl: TransmissionLine,
    cl: CoupledLines,
}

impl ImpedanceRule {
    pub fn with_specs(specs: Vec<NetImpedanceSpec>) -> Self {
        ImpedanceRule {
            specs: Some(specs),
            ..Default::default()
        }
    }

    fn partner_map(&self) -> Vec<(String, String)> {
        let mut m: Vec<(String, String)> = Vec::new();
        let mut set = |k: &str, v: &str| match m.iter_mut().find(|(a, _)| a == k) {
            Some(e) => e.1 = v.to_string(),
            None => m.push((k.to_string(), v.to_string())),
        };
        for p in &self.detected_pairs {
            set(&p.positive.net_name, &p.negative.net_name);
            set(&p.negative.net_name, &p.positive.net_name);
        }
        m
    }

    fn board_has_controlled_impedance(stackup: &Stackup) -> bool {
        stackup.has_explicit_data || stackup.num_copper_layers() >= 4
    }

    fn collect_traces(pcb: &Pcb) -> Vec<(String, Vec<Trace>)> {
        let mut out: Vec<(String, Vec<Trace>)> = Vec::new();
        for s in pcb.segments() {
            if s.net_name.is_empty() {
                continue;
            }
            let t = Trace {
                net_name: s.net_name.clone(),
                width_mm: s.width,
                layer: s.layer.clone(),
                start: s.start,
                end: s.end,
            };
            match out.iter_mut().find(|(n, _)| *n == s.net_name) {
                Some(e) => e.1.push(t),
                None => out.push((s.net_name.clone(), vec![t])),
            }
        }
        out
    }

    fn collect_current_sense_nets(pcb: &Pcb) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for fp in pcb.footprints() {
            for pad in &fp.pads {
                if names.contains(&pad.net_name) || !is_sense_net_name(&pad.net_name) {
                    continue;
                }
                let cand = KelvinPad {
                    x: pad.position.0,
                    y: pad.position.1,
                    reference: fp.reference.clone(),
                    footprint_name: fp.name.clone(),
                    steiner_point: false,
                };
                if find_kelvin_root(&[cand]).is_some() {
                    names.push(pad.net_name.clone());
                }
            }
        }
        names
    }

    fn check_trace(
        &self,
        ph: &Physics,
        partners: &[(String, String)],
        t: &Trace,
        spec: &NetImpedanceSpec,
    ) -> ImpedanceCheckResult {
        let outer = ph.stackup.is_outer_layer(&t.layer);
        let is_diff = partners.iter().any(|(k, _)| *k == t.net_name) && spec.target_zdiff.is_some();
        let (calc, target) = if is_diff {
            let gap = 0.15;
            let r = if outer {
                ph.cl.edge_coupled_microstrip(t.width_mm, gap, &t.layer)
            } else {
                ph.cl.edge_coupled_stripline(t.width_mm, gap, &t.layer)
            };
            match r {
                Ok(r) => (r.zdiff, spec.target_zdiff.unwrap()),
                Err(_) => (100.0, spec.target_zdiff.unwrap_or(100.0)),
            }
        } else {
            let r = if outer {
                ph.tl.microstrip(t.width_mm, &t.layer, 1.0)
            } else {
                ph.tl.stripline(t.width_mm, &t.layer, 1.0)
            };
            let z = r.map(|r| r.z0).unwrap_or(50.0);
            (z, spec.target_z0.filter(|v| *v != 0.0).unwrap_or(50.0))
        };
        let dev = (calc - target).abs() / target * 100.0;
        ImpedanceCheckResult {
            net_name: t.net_name.clone(),
            layer: t.layer.clone(),
            width_mm: t.width_mm,
            calculated_z0: calc,
            target_z0: target,
            deviation_percent: dev,
            compliant: dev <= spec.tolerance_percent,
        }
    }

    pub fn check(&self, pcb: &Pcb, _design_rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        let stackup = self
            .stackup
            .clone()
            .unwrap_or_else(|| Stackup::from_pcb(pcb.sexp()));
        let ph = Physics {
            tl: TransmissionLine::new(stackup.clone()),
            cl: CoupledLines::new(stackup.clone()),
            stackup,
        };
        let using_default = self.specs.is_none();
        if using_default && !Self::board_has_controlled_impedance(&ph.stackup) {
            return results;
        }
        let specs = self.specs.clone().unwrap_or_else(default_specs);
        let partners = self.partner_map();
        let sense = Self::collect_current_sense_nets(pcb);
        for (net, traces) in Self::collect_traces(pcb) {
            let kelvin = sense.contains(&net);
            let Some(spec) = specs.iter().find(|s| s.matches(&net, kelvin)) else {
                continue;
            };
            if spec.target_z0.is_none()
                && spec.target_zdiff.is_some()
                && !partners.iter().any(|(k, _)| *k == net)
            {
                continue;
            }
            if using_default && SE_HEURISTIC_PATTERNS.contains(&spec.net_pattern.as_str()) {
                continue;
            }
            for t in &traces {
                if hypot(t.end.0 - t.start.0, t.end.1 - t.start.1) < NECK_DOWN_EXEMPT_LENGTH_MM {
                    continue;
                }
                let r = self.check_trace(&ph, &partners, t, spec);
                if r.compliant {
                    continue;
                }
                let hint = match ph.tl.width_for_impedance(r.target_z0, &r.layer, "auto") {
                    Ok(w) => format!(" (requires {w:.3}mm)"),
                    Err(_) => String::new(),
                };
                results.add(
                    DRCViolation::new(
                        "impedance",
                        if r.deviation_percent > 20.0 {
                            "error"
                        } else {
                            "warning"
                        },
                        format!(
                            "Trace impedance mismatch on {}: width {:.3}mm gives {:.1}Ω, target \
                             is {:.1}Ω ({:.1}% deviation){hint}",
                            r.layer, r.width_mm, r.calculated_z0, r.target_z0, r.deviation_percent
                        ),
                    )
                    .layer(r.layer.clone())
                    .actual(r.calculated_z0)
                    .required(r.target_z0)
                    .items([r.net_name.clone()]),
                );
            }
        }
        results
    }
}
