//! BOM part-number health checks (port of `explain/checks/bom_health.py`).

use crate::explain::mistakes::{CheckIncomplete, Mistake, MistakeCategory, MistakeCheck};
use crate::schema::pcb::{Footprint, Pcb};

/// Recognised part-number property names, sorted (as `sorted(frozenset)`).
const PART_NUMBER_PROPERTY_NAMES: [&str; 9] = [
    "jlc",
    "jlcpcb",
    "lcsc",
    "lcsc part",
    "lcsc_pn",
    "manufacturer_pn",
    "mfr_pn",
    "mpn",
    "pn",
];

/// `fp.properties` as a Python dict: later duplicates overwrite the value
/// but keep the first key position.
fn properties(fp: &Footprint) -> Vec<(&str, &str)> {
    let mut out: Vec<(&str, &str)> = Vec::new();
    for (k, v) in &fp.properties {
        match out.iter_mut().find(|(n, _)| *n == k) {
            Some(e) => e.1 = v,
            None => out.push((k, v)),
        }
    }
    out
}

fn part_number(fp: &Footprint) -> String {
    for (name, value) in properties(fp) {
        if value.is_empty() {
            continue;
        }
        if PART_NUMBER_PROPERTY_NAMES.contains(&name.trim().to_lowercase().as_str()) {
            return value.to_string();
        }
    }
    String::new()
}

/// BOM-eligible components must carry a part-number property.
#[derive(Debug, Default, Clone, Copy)]
pub struct BomFieldHealthCheck;

impl MistakeCheck for BomFieldHealthCheck {
    fn name(&self) -> &'static str {
        "BomFieldHealthCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::BomHealth
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let candidates: Vec<&Footprint> = pcb
            .footprints()
            .iter()
            .filter(|fp| !fp.exclude_from_bom && !fp.dnp)
            .collect();
        if candidates.is_empty() {
            return Ok(vec![]);
        }
        if !candidates.iter().any(|fp| !part_number(fp).is_empty()) {
            let names: Vec<String> = PART_NUMBER_PROPERTY_NAMES
                .iter()
                .map(|n| crate::pyjson::py_repr_str(n))
                .collect();
            return Err(CheckIncomplete::new(format!(
                "no BOM-eligible component on this board defines an MPN/LCSC-style property \
                 (checked [{}]); this board may not track manufacturer part numbers in footprint \
                 properties, so BOM part-number coverage cannot be assessed",
                names.join(", ")
            )));
        }
        let mut out = Vec::new();
        for fp in candidates {
            if !part_number(fp).is_empty() {
                continue;
            }
            out.push(Mistake {
                category: MistakeCategory::BomHealth,
                severity: "warning".into(),
                title: "Component missing manufacturer part number".into(),
                components: vec![fp.reference.clone()],
                explanation: format!(
                    "{} ({}) has no MPN/LCSC part-number property, while other components on \
                     this board do. Without a specific part number, this component cannot be \
                     reliably re-sourced or ordered for assembly.",
                    fp.reference, fp.value
                ),
                fix_suggestion: format!(
                    "Add an 'MPN' (or distributor-specific, e.g. 'LCSC') property to {} with the \
                     manufacturer part number used elsewhere on this board.",
                    fp.reference
                ),
                location: Some(fp.position),
                learn_more_url: Some("docs/mistakes/bom-health.md".into()),
            });
        }
        Ok(out)
    }
}
