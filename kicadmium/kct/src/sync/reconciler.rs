//! Schematic-to-PCB reconciliation analysis (the `Reconciler.analyze` half
//! of `kicad_tools.sync.reconciler`; the `apply` mutation half belongs to the
//! `kct sync` command port).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::cost::suggest::parse_component_value;
use crate::schema::bom::extract_bom;
use crate::schema::pcb::Pcb;
use crate::schema::schematic::Schematic;

/// Relative tolerance for parsed passive magnitudes.
const VALUE_REL_TOLERANCE: f64 = 0.05;

/// Python `str.casefold()` (lowercase plus the common full foldings).
fn casefold(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            'ß' => "ss".chars().collect::<Vec<_>>(),
            'ς' => vec!['σ'],
            c => c.to_lowercase().collect(),
        })
        .collect()
}

/// `math.isclose(a, b, rel_tol=r)` (abs_tol 0).
fn isclose(a: f64, b: f64, rel: f64) -> bool {
    if a == b {
        return true;
    }
    if a.is_infinite() || b.is_infinite() {
        return false;
    }
    let d = (a - b).abs();
    d <= (rel * b).abs() || d <= (rel * a).abs()
}

/// True when two value strings denote the same component value.
pub fn values_equivalent(sch_value: &str, pcb_value: &str, reference: &str) -> bool {
    let s = sch_value.trim();
    let p = pcb_value.trim();
    if casefold(s) == casefold(p) {
        return true;
    }
    let ps = parse_component_value(s, reference);
    let pp = parse_component_value(p, reference);
    match (ps.numeric_value, pp.numeric_value) {
        (Some(a), Some(b)) if ps.component_type == pp.component_type => {
            isclose(a, b, VALUE_REL_TOLERANCE)
        }
        _ => false,
    }
}

static MOUNTING_HOLE_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:MH|MK|MP|MTG|H)\d+$").unwrap());

/// Mechanical mounting holes have no schematic symbol by convention.
pub fn is_mounting_hole(reference: &str, footprint_name: &str) -> bool {
    if !reference.is_empty() && MOUNTING_HOLE_REF_RE.is_match(reference) {
        return true;
    }
    let lower = footprint_name.to_lowercase();
    lower.contains("mountinghole") || lower.contains("mounting_hole")
}

/// One proposed action (`type`, `reference`, `old_value`, `new_value`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncAction {
    pub action_type: &'static str,
    pub reference: String,
    pub old_value: String,
    pub new_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncMatch {
    pub schematic_ref: String,
    pub pcb_ref: String,
    pub confidence: &'static str,
    pub match_type: &'static str,
    pub actions: Vec<SyncAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueMismatch {
    pub reference: String,
    pub schematic_value: String,
    pub pcb_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FootprintMismatch {
    pub reference: String,
    pub schematic_footprint: String,
    pub pcb_footprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddFootprintAction {
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub lib_id: String,
}

/// `SyncAnalysis`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncAnalysis {
    pub matches: Vec<SyncMatch>,
    pub schematic_orphans: Vec<String>,
    pub pcb_orphans: Vec<String>,
    pub value_mismatches: Vec<ValueMismatch>,
    pub footprint_mismatches: Vec<FootprintMismatch>,
    pub value_suffix_notes: Vec<ValueMismatch>,
    pub add_footprint_actions: Vec<AddFootprintAction>,
}

#[derive(Debug, Clone, Default)]
struct Comp {
    value: String,
    footprint: String,
    lib_id: String,
}

fn fp_name(fp: &str) -> &str {
    fp.rsplit(':').next().unwrap_or(fp)
}

pub struct Reconciler {
    pub schematic_path: PathBuf,
    pub pcb_path: PathBuf,
}

impl Reconciler {
    /// `Reconciler(schematic=..., pcb=...)` (both must exist).
    pub fn new(schematic: &Path, pcb: &Path) -> Result<Self, String> {
        if !schematic.exists() {
            return Err(format!("Schematic not found: {}", schematic.display()));
        }
        if !pcb.exists() {
            return Err(format!("PCB not found: {}", pcb.display()));
        }
        Ok(Reconciler {
            schematic_path: schematic.to_path_buf(),
            pcb_path: pcb.to_path_buf(),
        })
    }

    fn schematic_components(&self) -> Result<HashMap<String, Comp>, String> {
        let bom = extract_bom(&self.schematic_path.to_string_lossy(), true).map_err(|e| e.to_string())?;
        let mut out = HashMap::new();
        for item in bom.items {
            if item.is_power_symbol() || !item.on_board {
                continue;
            }
            if !item.reference.is_empty() && !item.reference.starts_with('#') {
                out.insert(
                    item.reference.clone(),
                    Comp {
                        value: item.value.clone(),
                        footprint: item.footprint.clone(),
                        lib_id: item.lib_id.clone(),
                    },
                );
            }
        }
        Ok(out)
    }

    fn pcb_components(pcb: &Pcb) -> HashMap<String, Comp> {
        let mut out = HashMap::new();
        for fp in pcb.footprints() {
            if fp.reference.is_empty() || fp.reference.starts_with('#') {
                continue;
            }
            if is_mounting_hole(&fp.reference, &fp.name) {
                continue;
            }
            out.insert(
                fp.reference.clone(),
                Comp {
                    value: fp.value.clone(),
                    footprint: fp.name.clone(),
                    lib_id: String::new(),
                },
            );
        }
        out
    }

    /// `Reconciler.analyze()`.
    pub fn analyze(&self) -> Result<SyncAnalysis, String> {
        // SchematicPCBChecker loads both documents up front.
        Schematic::load(&self.schematic_path).map_err(|e| e.to_string())?;
        let pcb = Pcb::load(&self.pcb_path).map_err(|e| e.to_string())?;
        let sch = self.schematic_components()?;
        let pcbc = Self::pcb_components(&pcb);
        let sch_refs: BTreeSet<&String> = sch.keys().collect();
        let pcb_refs: BTreeSet<&String> = pcbc.keys().collect();
        let mut a = SyncAnalysis::default();

        for r in sch_refs.intersection(&pcb_refs) {
            let (s, p) = (&sch[*r], &pcbc[*r]);
            let mut actions = Vec::new();
            if !s.value.is_empty() && !p.value.is_empty() && s.value != p.value {
                let m = ValueMismatch {
                    reference: (*r).clone(),
                    schematic_value: s.value.clone(),
                    pcb_value: p.value.clone(),
                };
                if values_equivalent(&s.value, &p.value, r) {
                    a.value_suffix_notes.push(m);
                } else {
                    actions.push(SyncAction {
                        action_type: "update_value",
                        reference: (*r).clone(),
                        old_value: p.value.clone(),
                        new_value: s.value.clone(),
                    });
                    a.value_mismatches.push(m);
                }
            }
            if !s.footprint.is_empty()
                && !p.footprint.is_empty()
                && fp_name(&s.footprint) != fp_name(&p.footprint)
            {
                actions.push(SyncAction {
                    action_type: "update_footprint",
                    reference: (*r).clone(),
                    old_value: p.footprint.clone(),
                    new_value: s.footprint.clone(),
                });
                a.footprint_mismatches.push(FootprintMismatch {
                    reference: (*r).clone(),
                    schematic_footprint: s.footprint.clone(),
                    pcb_footprint: p.footprint.clone(),
                });
            }
            a.matches.push(SyncMatch {
                schematic_ref: (*r).clone(),
                pcb_ref: (*r).clone(),
                confidence: "high",
                match_type: "exact",
                actions,
            });
        }
        let sch_orphans: Vec<String> = sch_refs.difference(&pcb_refs).map(|s| (*s).clone()).collect();
        let pcb_orphans: Vec<String> = pcb_refs.difference(&sch_refs).map(|s| (*s).clone()).collect();
        let mut matched_sch: BTreeSet<String> = BTreeSet::new();
        let mut matched_pcb: BTreeSet<String> = BTreeSet::new();

        for sr in &sch_orphans {
            let s = &sch[sr];
            if s.value.is_empty() || s.footprint.is_empty() {
                continue;
            }
            let cands: Vec<&String> = pcb_orphans
                .iter()
                .filter(|pr| !matched_pcb.contains(*pr))
                .filter(|pr| {
                    let p = &pcbc[*pr];
                    values_equivalent(&s.value, &p.value, sr) && fp_name(&s.footprint) == fp_name(&p.footprint)
                })
                .collect();
            if let [pr] = cands[..] {
                a.matches.push(SyncMatch {
                    schematic_ref: sr.clone(),
                    pcb_ref: pr.clone(),
                    confidence: "medium",
                    match_type: "value_footprint",
                    actions: vec![SyncAction {
                        action_type: "rename",
                        reference: pr.clone(),
                        old_value: pr.clone(),
                        new_value: sr.clone(),
                    }],
                });
                matched_sch.insert(sr.clone());
                matched_pcb.insert(pr.clone());
            }
        }
        for sr in &sch_orphans {
            if matched_sch.contains(sr) {
                continue;
            }
            let s = &sch[sr];
            if s.footprint.is_empty() {
                continue;
            }
            let cands: Vec<&String> = pcb_orphans
                .iter()
                .filter(|pr| !matched_pcb.contains(*pr))
                .filter(|pr| fp_name(&s.footprint) == fp_name(&pcbc[*pr].footprint))
                .collect();
            if let [pr] = cands[..] {
                let mut actions = vec![SyncAction {
                    action_type: "rename",
                    reference: pr.clone(),
                    old_value: pr.clone(),
                    new_value: sr.clone(),
                }];
                let p = &pcbc[pr];
                if !s.value.is_empty() && !p.value.is_empty() && !values_equivalent(&s.value, &p.value, sr) {
                    actions.push(SyncAction {
                        action_type: "update_value",
                        reference: sr.clone(),
                        old_value: p.value.clone(),
                        new_value: s.value.clone(),
                    });
                }
                a.matches.push(SyncMatch {
                    schematic_ref: sr.clone(),
                    pcb_ref: pr.clone(),
                    confidence: "low",
                    match_type: "footprint_only",
                    actions,
                });
                matched_sch.insert(sr.clone());
                matched_pcb.insert(pr.clone());
            }
        }
        a.schematic_orphans = sch_orphans.into_iter().filter(|r| !matched_sch.contains(r)).collect();
        a.pcb_orphans = pcb_orphans.into_iter().filter(|r| !matched_pcb.contains(r)).collect();
        for r in &a.schematic_orphans {
            let s = &sch[r];
            if !s.footprint.is_empty() {
                a.add_footprint_actions.push(AddFootprintAction {
                    reference: r.clone(),
                    value: s.value.clone(),
                    footprint: s.footprint.clone(),
                    lib_id: s.lib_id.clone(),
                });
            }
        }
        Ok(a)
    }
}
