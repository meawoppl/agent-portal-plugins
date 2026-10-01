//! Schematic/PCB consistency and four-pass LVS matching.
//!
//! This is the native counterpart of `kicad_tools.validate.consistency`.
//! The checker operates on deliberately small records so hierarchy-aware BOM
//! and netlist extraction remain separate from comparison policy.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub pad_nets: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsistencyIssue {
    pub issue_type: String,
    pub domain: String,
    pub schematic_value: Option<String>,
    pub pcb_value: Option<String>,
    pub reference: String,
    pub severity: String,
    pub suggestion: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ConsistencyResult {
    pub issues: Vec<ConsistencyIssue>,
}

impl ConsistencyResult {
    pub fn error_count(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == "error").count()
    }
    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == "warning")
            .count()
    }
    pub fn is_consistent(&self) -> bool {
        self.error_count() == 0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LvsMatch {
    pub pcb_ref: String,
    pub sch_ref: String,
    pub confidence: f64,
    pub match_reason: String,
    pub value_match: bool,
    pub footprint_match: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LvsResult {
    pub matches: Vec<LvsMatch>,
    pub unmatched_pcb: Vec<String>,
    #[serde(rename = "unmatched_schematic")]
    pub unmatched_sch: Vec<String>,
}

impl LvsResult {
    pub fn exact_match_count(&self) -> usize {
        self.matches.iter().filter(|m| m.confidence >= 1.0).count()
    }
    pub fn fuzzy_match_count(&self) -> usize {
        self.matches.iter().filter(|m| m.confidence < 1.0).count()
    }
    pub fn is_clean(&self) -> bool {
        self.unmatched_pcb.is_empty()
            && self.unmatched_sch.is_empty()
            && self.fuzzy_match_count() == 0
    }
}

fn normalize_footprint(value: &str) -> &str {
    value.rsplit(':').next().unwrap_or(value)
}
fn prefix(reference: &str) -> &str {
    let n = reference
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(reference.len());
    &reference[..n]
}
fn package_size(fp: &str) -> Option<&str> {
    let name = normalize_footprint(fp);
    for (start, _) in name.char_indices() {
        let Some(candidate) = name.get(start..start + 4) else {
            break;
        };
        if candidate.bytes().all(|c| c.is_ascii_digit()) {
            let before = start == 0 || matches!(name.as_bytes()[start - 1], b'_' | b'-');
            let after =
                start + 4 == name.len() || matches!(name.as_bytes()[start + 4], b'_' | b'-');
            if before && after {
                return Some(candidate);
            }
        }
    }
    None
}

/// Compare component presence, pin nets, values and footprints.
pub fn consistency(
    schematic: &[Component],
    pcb: &[Component],
    schematic_pin_nets: &BTreeMap<String, BTreeMap<String, String>>,
) -> ConsistencyResult {
    let sm: BTreeMap<_, _> = schematic
        .iter()
        .filter(|c| !c.reference.starts_with('#'))
        .map(|c| (c.reference.as_str(), c))
        .collect();
    let pm: BTreeMap<_, _> = pcb
        .iter()
        .filter(|c| !c.reference.starts_with('#'))
        .map(|c| (c.reference.as_str(), c))
        .collect();
    let mut issues = Vec::new();
    for reference in sm.keys().filter(|r| !pm.contains_key(**r)) {
        let fp = &sm[reference].footprint;
        let hint = if fp.is_empty() {
            String::new()
        } else {
            format!(" ({fp})")
        };
        issues.push(ConsistencyIssue {
            issue_type: "missing".into(),
            domain: "component".into(),
            schematic_value: Some((*reference).into()),
            pcb_value: None,
            reference: (*reference).into(),
            severity: "error".into(),
            suggestion: format!("Add footprint for {reference}{hint} to PCB"),
        });
    }
    for reference in pm.keys().filter(|r| !sm.contains_key(**r)) {
        issues.push(ConsistencyIssue {
            issue_type: "extra".into(),
            domain: "component".into(),
            schematic_value: None,
            pcb_value: Some((*reference).into()),
            reference: (*reference).into(),
            severity: "warning".into(),
            suggestion: format!("Remove {reference} from PCB or add to schematic"),
        });
    }
    for (reference, sch) in &sm {
        let Some(board) = pm.get(reference) else {
            continue;
        };
        if !sch.value.is_empty() && !board.value.is_empty() && sch.value != board.value {
            issues.push(ConsistencyIssue {
                issue_type: "mismatch".into(),
                domain: "property".into(),
                schematic_value: Some(sch.value.clone()),
                pcb_value: Some(board.value.clone()),
                reference: (*reference).into(),
                severity: "warning".into(),
                suggestion: format!(
                    "Update {reference} value: schematic has \"{}\", PCB has \"{}\"",
                    sch.value, board.value
                ),
            });
        }
        if !sch.footprint.is_empty()
            && !board.footprint.is_empty()
            && normalize_footprint(&sch.footprint) != normalize_footprint(&board.footprint)
        {
            issues.push(ConsistencyIssue {
                issue_type: "mismatch".into(),
                domain: "property".into(),
                schematic_value: Some(sch.footprint.clone()),
                pcb_value: Some(board.footprint.clone()),
                reference: (*reference).into(),
                severity: "error".into(),
                suggestion: format!(
                    "Update {reference} footprint: schematic has \"{}\", PCB has \"{}\"",
                    sch.footprint, board.footprint
                ),
            });
        }
        if let Some(pins) = schematic_pin_nets.get(*reference) {
            for (pin, sch_net) in pins {
                if let Some(pcb_net) = board.pad_nets.get(pin) {
                    if !sch_net.is_empty() && !pcb_net.is_empty() && sch_net != pcb_net {
                        issues.push(ConsistencyIssue { issue_type:"mismatch".into(), domain:"net".into(), schematic_value:Some(sch_net.clone()), pcb_value:Some(pcb_net.clone()), reference:format!("{reference}.{pin}"), severity:"error".into(), suggestion:format!("Update net {reference}.{pin}: schematic has \"{sch_net}\", PCB has \"{pcb_net}\"") });
                    }
                }
            }
        }
    }
    ConsistencyResult { issues }
}

/// Apply upstream's exact, unique value+footprint, unique value+prefix and
/// net-correlation passes, in that order.
pub fn lvs(schematic: &[Component], pcb: &[Component]) -> LvsResult {
    let sm: BTreeMap<_, _> = schematic
        .iter()
        .filter(|c| !c.reference.starts_with('#'))
        .map(|c| (c.reference.clone(), c))
        .collect();
    let pm: BTreeMap<_, _> = pcb
        .iter()
        .filter(|c| !c.reference.starts_with('#'))
        .map(|c| (c.reference.clone(), c))
        .collect();
    let mut us: BTreeSet<_> = sm.keys().cloned().collect();
    let mut up: BTreeSet<_> = pm.keys().cloned().collect();
    let mut matches = Vec::new();
    let exact: Vec<_> = us
        .intersection(&up)
        .filter(|r| {
            sm[*r].value == pm[*r].value
                && normalize_footprint(&sm[*r].footprint) == normalize_footprint(&pm[*r].footprint)
        })
        .cloned()
        .collect();
    for r in exact {
        add_match(
            &mut matches,
            &mut us,
            &mut up,
            &r,
            &r,
            1.0,
            "exact ref+value+footprint",
            true,
            true,
        );
    }

    let mut sv: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut pv: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for r in &us {
        sv.entry((
            sm[r].value.clone(),
            normalize_footprint(&sm[r].footprint).into(),
        ))
        .or_default()
        .push(r.clone());
    }
    for r in &up {
        pv.entry((
            pm[r].value.clone(),
            normalize_footprint(&pm[r].footprint).into(),
        ))
        .or_default()
        .push(r.clone());
    }
    let pairs: Vec<_> = sv
        .iter()
        .filter_map(|(k, s)| {
            let p = pv.get(k)?;
            (s.len() == 1 && p.len() == 1 && s[0] != p[0]).then(|| (s[0].clone(), p[0].clone()))
        })
        .collect();
    for (s, p) in pairs {
        add_match(
            &mut matches,
            &mut us,
            &mut up,
            &s,
            &p,
            0.8,
            "unique value+footprint pair",
            true,
            true,
        );
    }

    let mut sv: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut pv: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for r in &us {
        sv.entry((prefix(r).into(), sm[r].value.clone()))
            .or_default()
            .push(r.clone());
    }
    for r in &up {
        pv.entry((prefix(r).into(), pm[r].value.clone()))
            .or_default()
            .push(r.clone());
    }
    let pairs: Vec<_> = sv.iter().filter_map(|(k,s)| { let p=pv.get(k)?; if s.len()!=1||p.len()!=1{return None}; let (a,b)=(&sm[&s[0]],&pm[&p[0]]); if matches!((package_size(&a.footprint),package_size(&b.footprint)),(Some(x),Some(y)) if x!=y){return None}; Some((s[0].clone(),p[0].clone(),normalize_footprint(&a.footprint)==normalize_footprint(&b.footprint))) }).collect();
    for (s, p, fp) in pairs {
        add_match(
            &mut matches,
            &mut us,
            &mut up,
            &s,
            &p,
            0.6,
            "unique value within reference prefix",
            true,
            fp,
        );
    }

    let prefixes: BTreeSet<_> = us.iter().map(|r| prefix(r).to_string()).collect();
    for pre in prefixes {
        let ss: Vec<_> = us.iter().filter(|r| prefix(r) == pre).cloned().collect();
        let pp: Vec<_> = up.iter().filter(|r| prefix(r) == pre).cloned().collect();
        if ss.len() != 1 || pp.len() != 1 || pm[&pp[0]].pad_nets.is_empty() {
            continue;
        }
        let (a, b) = (&sm[&ss[0]], &pm[&pp[0]]);
        if matches!((package_size(&a.footprint),package_size(&b.footprint)),(Some(x),Some(y)) if x!=y)
        {
            continue;
        }
        let vm = a.value == b.value;
        let fm = normalize_footprint(&a.footprint) == normalize_footprint(&b.footprint);
        add_match(
            &mut matches,
            &mut us,
            &mut up,
            &ss[0],
            &pp[0],
            0.4,
            "net-based correlation within prefix",
            vm,
            fm,
        );
    }
    LvsResult {
        matches,
        unmatched_pcb: up.into_iter().collect(),
        unmatched_sch: us.into_iter().collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn add_match(
    out: &mut Vec<LvsMatch>,
    us: &mut BTreeSet<String>,
    up: &mut BTreeSet<String>,
    s: &str,
    p: &str,
    confidence: f64,
    reason: &str,
    value_match: bool,
    footprint_match: bool,
) {
    out.push(LvsMatch {
        pcb_ref: p.into(),
        sch_ref: s.into(),
        confidence,
        match_reason: reason.into(),
        value_match,
        footprint_match,
    });
    us.remove(s);
    up.remove(p);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(r: &str, v: &str, f: &str) -> Component {
        Component {
            reference: r.into(),
            value: v.into(),
            footprint: f.into(),
            pad_nets: BTreeMap::new(),
        }
    }
    #[test]
    fn all_lvs_passes_and_package_guard() {
        let s = vec![
            c("U1", "MCU", "QFP"),
            c("R1", "10k", "Resistor:R_0402_1005Metric"),
            c("C1", "1u", "C_0402_1005Metric"),
            c("L1", "2u", "L_0603_1608Metric"),
            c("R9", "22k", "R_0402_1005Metric"),
        ];
        let mut p = vec![
            c("U1", "MCU", "QFP"),
            c("R7", "10k", "R_0402_1005Metric"),
            c("C8", "1u", "C_0402_alt"),
            c("L8", "different", "L_unknown"),
            c("R8", "22k", "R_0603_1608Metric"),
        ];
        p[3].pad_nets.insert("1".into(), "GND".into());
        let x = lvs(&s, &p);
        assert_eq!(x.exact_match_count(), 1);
        assert_eq!(x.fuzzy_match_count(), 3);
        assert_eq!(x.unmatched_sch, vec!["R9"]);
        assert_eq!(x.unmatched_pcb, vec!["R8"]);
        assert_eq!(
            x.matches.iter().map(|m| m.confidence).collect::<Vec<_>>(),
            vec![1.0, 0.8, 0.6, 0.4]
        );
    }
    #[test]
    fn consistency_severities_match_upstream() {
        let s = vec![c("R1", "10k", "Lib:R_0402"), c("C1", "1u", "C_0402")];
        let p = vec![c("R1", "12k", "Other:R_0603"), c("U1", "MCU", "QFP")];
        let x = consistency(&s, &p, &BTreeMap::new());
        assert_eq!(x.error_count(), 2);
        assert_eq!(x.warning_count(), 2);
        assert!(!x.is_consistent());
    }
}
