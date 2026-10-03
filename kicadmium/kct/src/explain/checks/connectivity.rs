//! Pull-up and LED series-resistor connectivity checks (port of
//! `explain/checks/connectivity.py`).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::explain::mistakes::{
    is_ground_net, is_power_net, CheckIncomplete, Mistake, MistakeCategory, MistakeCheck,
};
use crate::pyjson::py_repr_str;
use crate::schema::pcb::{Footprint, Pcb};

const I2C_NET_PATTERNS: [&str; 2] = ["SCL", "SDA"];
static RESET_NET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(^|[_-])(N?RST|RESET)($|[_-])").unwrap());

fn needs_pullup(net: &str) -> bool {
    let up = net.to_uppercase();
    I2C_NET_PATTERNS.iter().any(|p| up.contains(p)) || RESET_NET_RE.is_match(net)
}

fn pad_nets(fp: &Footprint) -> BTreeSet<&str> {
    fp.pads
        .iter()
        .filter(|p| !p.net_name.is_empty())
        .map(|p| p.net_name.as_str())
        .collect()
}

/// I2C bus lines and reset lines need a pull-up resistor.
#[derive(Debug, Default, Clone, Copy)]
pub struct PullUpResistorCheck;

impl MistakeCheck for PullUpResistorCheck {
    fn name(&self) -> &'static str {
        "PullUpResistorCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::Connectivity
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let mut nets: std::collections::BTreeMap<&str, BTreeSet<&str>> = Default::default();
        for fp in pcb.footprints() {
            for pad in &fp.pads {
                if !pad.net_name.is_empty() && needs_pullup(&pad.net_name) {
                    nets.entry(pad.net_name.as_str())
                        .or_default()
                        .insert(fp.reference.as_str());
                }
            }
        }
        let has_pullup = |net: &str| {
            pcb.footprints().iter().any(|fp| {
                if !fp.reference.to_uppercase().starts_with('R') {
                    return false;
                }
                let pn = pad_nets(fp);
                pn.contains(net) && pn.iter().any(|n| *n != net && is_power_net(n))
            })
        };
        let mut out = Vec::new();
        for (net, refs) in nets {
            if refs.len() < 2 || has_pullup(net) {
                continue;
            }
            out.push(Mistake {
                category: MistakeCategory::Connectivity,
                severity: "warning".into(),
                title: "Missing pull-up resistor".into(),
                components: refs.iter().map(|r| r.to_string()).collect(),
                explanation: format!(
                    "Net {} looks like an I2C bus line or reset line, but no resistor connects \
                     it to a supply rail. I2C is open-drain and needs an external pull-up to \
                     function; an unpulled reset line can float into an indeterminate state.",
                    py_repr_str(net)
                ),
                fix_suggestion: format!(
                    "Add a pull-up resistor (typically 2.2k-10k for I2C) from {net} to the \
                     bus/logic supply rail."
                ),
                location: None,
                learn_more_url: Some("docs/mistakes/pull-up-resistors.md".into()),
            });
        }
        Ok(out)
    }
}

fn is_led(fp: &Footprint) -> bool {
    let r = fp.reference.to_uppercase();
    if r.starts_with("LED") {
        return true;
    }
    if !r.starts_with('D') {
        return false;
    }
    format!("{} {}", fp.value, fp.name)
        .to_uppercase()
        .contains("LED")
}

/// LEDs need a series current-limiting resistor.
#[derive(Debug, Default, Clone, Copy)]
pub struct LedSeriesResistorCheck;

fn has_series_resistor(pcb: &Pcb, led: &Footprint, led_nets: &BTreeSet<&str>) -> bool {
    if led.pads.len() != 2 || led_nets.len() != 2 {
        return false;
    }
    for fp in pcb.footprints() {
        if std::ptr::eq(fp, led)
            || !fp.reference.to_uppercase().starts_with('R')
            || fp.pads.len() != 2
        {
            continue;
        }
        let pn = pad_nets(fp);
        let shared: Vec<&&str> = pn.intersection(led_nets).collect();
        if pn.len() != 2 || shared.len() != 1 {
            continue;
        }
        let junction = *shared[0];
        if is_power_net(junction) || is_ground_net(junction) {
            continue;
        }
        // The junction must carry only this resistor and LEDs: one LED in
        // series (2 terminals), or LEDs sharing one series resistor, e.g.
        // an anti-parallel bicolour pair (kicadmium extension; upstream
        // required exactly 2 terminals).
        let mut terminals = 0usize;
        let mut only_resistor_and_leds = true;
        for c in pcb.footprints() {
            let n = c.pads.iter().filter(|p| p.net_name == junction).count();
            terminals += n;
            if n > 0 && !std::ptr::eq(c, fp) && !is_led(c) {
                only_resistor_and_leds = false;
            }
        }
        if terminals == 2 || (terminals > 2 && only_resistor_and_leds) {
            return true;
        }
    }
    false
}

impl MistakeCheck for LedSeriesResistorCheck {
    fn name(&self) -> &'static str {
        "LedSeriesResistorCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::Connectivity
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let mut out = Vec::new();
        for fp in pcb.footprints() {
            if !is_led(fp) {
                continue;
            }
            let nets = pad_nets(fp);
            if nets.is_empty() || has_series_resistor(pcb, fp, &nets) {
                continue;
            }
            out.push(Mistake {
                category: MistakeCategory::Connectivity,
                severity: "warning".into(),
                title: "LED missing series resistor".into(),
                components: vec![fp.reference.clone()],
                explanation: format!(
                    "{} has no confirmed series resistor in its net topology. Driving an LED \
                     without a series current-limiting resistor risks over-current damage or a \
                     shortened lifespan, since the LED's own forward resistance is not a \
                     reliable current limit.",
                    fp.reference
                ),
                fix_suggestion: format!(
                    "Add a series resistor in-line with {}, sized for the LED's rated forward \
                     current at the supply voltage in use (unless current limiting is already \
                     provided by a dedicated driver IC).",
                    fp.reference
                ),
                location: Some(fp.position),
                learn_more_url: Some("docs/mistakes/led-series-resistor.md".into()),
            });
        }
        Ok(out)
    }
}
