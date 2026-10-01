//! Kelvin (current-sense) net recognition (the detection slice of
//! `kicad_tools.router.kelvin` used by DRC: sense-name matching and the
//! shunt-resistor root pad).

use std::sync::LazyLock;

use regex::Regex;

use super::net_names::net_name_suffix;

static SENSE_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(I_?SENSE|V_?SENSE|C_?SENSE|CURR(ENT)?_?SENSE|SENSE|SHUNT|ISNS|VSNS|CSNS|IMON|VMON|KELVIN)",
    )
    .unwrap()
});
static RESISTOR_REF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^R\d").unwrap());
static RESISTOR_FP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(resistor|_shunt|shunt_|current[_-]?sense)").unwrap());

/// The pad fields Kelvin root selection reads.
#[derive(Debug, Clone, PartialEq)]
pub struct KelvinPad {
    pub x: f64,
    pub y: f64,
    pub reference: String,
    pub footprint_name: String,
    pub steiner_point: bool,
}

/// `_is_shunt_pad`.
pub fn is_shunt_pad(pad: &KelvinPad) -> bool {
    if pad.steiner_point {
        return false;
    }
    let r = pad.reference.trim();
    if !r.is_empty() && RESISTOR_REF_RE.is_match(r) {
        return true;
    }
    !pad.footprint_name.is_empty() && RESISTOR_FP_RE.is_match(&pad.footprint_name)
}

/// `is_sense_net_name`.
pub fn is_sense_net_name(net_name: &str) -> bool {
    !net_name.is_empty() && SENSE_NAME_RE.is_match(net_name_suffix(net_name))
}

/// `find_kelvin_root`: the most central shunt pad (lowest index on ties).
pub fn find_kelvin_root(pads: &[KelvinPad]) -> Option<usize> {
    let cands: Vec<usize> = (0..pads.len()).filter(|&i| is_shunt_pad(&pads[i])).collect();
    match cands.len() {
        0 => None,
        1 => Some(cands[0]),
        _ => {
            let total = |i: usize| {
                crate::utils::pymath::py_sum(
                    (0..pads.len())
                        .filter(|&k| k != i)
                        .map(|k| (pads[i].x - pads[k].x).abs() + (pads[i].y - pads[k].y).abs()),
                )
            };
            cands
                .into_iter()
                .min_by(|&a, &b| total(a).total_cmp(&total(b)).then(a.cmp(&b)))
        }
    }
}
