//! Read-only, evidence-aware KiCad layout lint engine behind `kct lint`.
//!
//! Parses a `.kicad_pcb` into a normalized geometry model ([`model`]), runs the
//! rule catalog ([`rules`], `catalog.json`) and returns a versioned [`Report`]
//! whose findings carry stable review keys and rule-scoped evidence hashes
//! ([`evidence`]). Per-board policy and exceptions live in `<board>.lint.json`
//! ([`board_file`]); [`contact_sheet`] and [`ci`] render reviewable artifacts.
//! Findings are advisory heuristics, not DRC, and nothing here edits a design.
//! Formerly the standalone `pcb-lint` crate; see `docs/lint.md`.

pub mod advanced;
pub mod board_file;
pub mod ci;
pub mod contact_sheet;
pub mod copper;
pub mod design;
pub mod evidence;
pub mod intent;
pub mod manufacturing;
pub mod model;
pub mod review;
pub mod rules;
use anyhow::{bail, Result};
use model::{Board, Point};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
pub fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub intent: intent::Intent,
    pub disabled: BTreeSet<String>,
    pub enabled_only: BTreeSet<String>,
    pub min_trace_mm: f64,
    pub min_drill_mm: f64,
    pub min_annulus_mm: f64,
    pub short_segment_mm: f64,
    pub angle_tolerance_deg: f64,
    pub join_tolerance_mm: f64,
    pub via_cluster_mm: f64,
    pub detour_ratio: f64,
    pub detour_excess_mm: f64,
    pub narrow_run_mm: f64,
    pub bend_threshold: usize,
    pub net_rules: Vec<NetRule>,
    pub pin_nets: Vec<PinNet>,
    pub proximity: Vec<Proximity>,
    pub groups: Vec<Group>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            intent: intent::Intent::default(),
            disabled: BTreeSet::new(),
            enabled_only: BTreeSet::new(),
            min_trace_mm: 0.15,
            min_drill_mm: 0.2,
            min_annulus_mm: 0.1,
            short_segment_mm: 0.2,
            angle_tolerance_deg: 1.,
            join_tolerance_mm: 0.001,
            via_cluster_mm: 0.7,
            detour_ratio: 1.5,
            detour_excess_mm: 2.,
            narrow_run_mm: 1.,
            bend_threshold: 6,
            net_rules: vec![],
            pin_nets: vec![],
            proximity: vec![],
            groups: vec![],
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetRule {
    pub net: String,
    pub min_width_mm: Option<f64>,
    pub max_vias: Option<usize>,
    pub allowed_layers: Option<Vec<String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinNet {
    pub reference: String,
    pub pad: String,
    pub net: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proximity {
    pub reference: String,
    pub to: String,
    pub max_distance_mm: f64,
    #[serde(default)]
    pub same_side: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    pub references: Vec<String>,
    pub axis: String,
    pub tolerance_mm: f64,
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        for (n, v) in [
            ("min_trace", self.min_trace_mm),
            ("min_drill", self.min_drill_mm),
            ("min_annulus", self.min_annulus_mm),
            ("short_segment", self.short_segment_mm),
            ("angle_tolerance", self.angle_tolerance_deg),
            ("join_tolerance", self.join_tolerance_mm),
            ("via_cluster", self.via_cluster_mm),
            ("detour_ratio", self.detour_ratio),
            ("detour_excess", self.detour_excess_mm),
            ("narrow_run", self.narrow_run_mm),
        ] {
            if !v.is_finite() || v <= 0. {
                bail!("{n} must be finite and positive")
            }
        }
        if self.join_tolerance_mm > 0.01
            || self.angle_tolerance_deg >= 22.5
            || self.detour_ratio <= 1.
            || self.bend_threshold < 2
        {
            bail!("invalid geometric threshold range")
        }
        let catalog = rules::catalog();
        let ids: BTreeSet<_> = catalog.iter().map(|r| r.id.as_str()).collect();
        for id in self.disabled.iter().chain(&self.enabled_only) {
            if !ids.contains(id.as_str()) {
                bail!("unknown rule {id}")
            }
        }
        for n in &self.net_rules {
            if n.net.is_empty()
                || n.min_width_mm.is_some_and(|w| !w.is_finite() || w <= 0.)
                || n.allowed_layers
                    .as_ref()
                    .is_some_and(|v| v.is_empty() || v.iter().any(|x| !x.ends_with(".Cu")))
            {
                bail!("invalid net contract")
            }
        }
        let mut pins = BTreeSet::new();
        for p in &self.pin_nets {
            if p.reference.is_empty() || p.pad.is_empty() || !pins.insert((&p.reference, &p.pad)) {
                bail!("empty or repeated pin contract")
            }
        }
        for p in &self.proximity {
            if p.reference.is_empty()
                || p.to.is_empty()
                || !p.max_distance_mm.is_finite()
                || p.max_distance_mm <= 0.
            {
                bail!("invalid proximity contract")
            }
        }
        for g in &self.groups {
            if !["row", "column"].contains(&g.axis.as_str())
                || g.references.len() < 2
                || !g.tolerance_mm.is_finite()
                || g.tolerance_mm <= 0.
            {
                bail!("invalid alignment group")
            }
        }
        self.intent.validate()?;
        Ok(())
    }
    pub fn enabled(&self, id: &str) -> bool {
        !self.disabled.contains(id)
            && (self.enabled_only.is_empty() || self.enabled_only.contains(id))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub key: String,
    pub evidence: String,
    pub rule: String,
    pub severity: String,
    pub confidence: String,
    pub message: String,
    pub suggestion: String,
    pub subjects: Vec<String>,
    pub nets: Vec<String>,
    pub at: Point,
    pub metrics: BTreeMap<String, f64>,
    pub stable_identity: bool,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<review::Decision>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema: u32,
    pub tool_version: String,
    pub board_id: String,
    pub source_sha256: String,
    pub config: Config,
    pub limitations: Vec<String>,
    pub objects: BTreeMap<String, usize>,
    pub findings: Vec<Finding>,
    pub review_audit: Vec<review::Audit>,
    pub coverage: Vec<Coverage>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Coverage {
    pub rule: String,
    pub status: String,
}
pub struct Emitter<'a> {
    pub board: &'a Board,
    pub config: &'a Config,
    pub board_id: &'a str,
    pub findings: Vec<Finding>,
    pub coverage: BTreeMap<String, String>,
    pub source_sha256: String,
}
impl Emitter<'_> {
    #[allow(clippy::too_many_arguments)]
    pub fn emit(
        &mut self,
        rule: &str,
        subjects: Vec<String>,
        nets: Vec<String>,
        at: Point,
        slot: &str,
        message: String,
        metrics: &[(&str, f64)],
    ) {
        let mut subjects = subjects;
        subjects.sort();
        subjects.dedup();
        let mut nets = nets;
        nets.sort();
        nets.dedup();
        let stable = !subjects.is_empty()
            && subjects
                .iter()
                .all(|s| !s.starts_with("fallback:") && !s.starts_with("contract:"));
        let key = hash(&serde_json::to_string(&(self.board_id, rule, &subjects, slot)).unwrap());
        // Evidence is assigned after all rules run; see `evidence`.
        let r = rules::catalog_cached()
            .iter()
            .find(|r| r.id == rule)
            .expect("registered rule");
        self.findings.push(Finding {
            key,
            evidence: String::new(),
            rule: rule.into(),
            severity: r.severity.clone(),
            confidence: r.confidence.clone(),
            message,
            suggestion: r.action.clone(),
            subjects,
            nets,
            at,
            metrics: metrics.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            stable_identity: stable,
            state: "open".into(),
            review: None,
        });
    }
}
pub fn lint(input: &str, board_id: &str, config: Config) -> Result<Report> {
    if board_id.trim().is_empty() {
        bail!("board ID must not be empty")
    }
    config.validate()?;
    let board = Board::read(input)?;
    let mut e = Emitter {
        board: &board,
        config: &config,
        board_id,
        findings: vec![],
        coverage: BTreeMap::new(),
        source_sha256: hash(input),
    };
    rules::run(&mut e);
    advanced::run(&mut e, input)?;
    e.findings.retain(|f| config.enabled(&f.rule));
    e.findings.sort_by(|a, b| a.key.cmp(&b.key));
    e.findings.dedup_by(|a, b| a.key == b.key);
    let mut findings = e.findings;
    let extra = copper::Extra::read(input, &board)?;
    evidence::assign(&board, &extra, &config, &mut findings);
    let rule_coverage = e.coverage;
    let mut limitations=vec!["Heuristics, not KiCad DRC/ERC or fabrication sign-off. No edits are made.".into(),"Core trace endpoint checks ignore planes and arcs. Via attachment and layer-excursion screening use saved polygon fills and tessellated arcs; unsupported geometry withholds uncertain conclusions. Saved fills must be refreshed after layout edits. See coverage per rule.".into(),"Detour ratios use unobstructed octilinear distance as a lower bound, not a proven legal replacement route. Explicit thermal and reference-plane contracts provide screening only, not impedance or thermal proof.".into(),"Missing UUIDs use geometry fallback identities and cannot be ignored persistently. Board identity is caller-supplied; do not reuse across independent designs.".into()];
    for (k, v) in &board.unsupported {
        limitations.push(format!("{k}: {v}"));
    }
    if !board.zones.is_empty() {
        limitations.push(format!(
            "{} zones present; isolated-copper candidates may be plane-connected.",
            board.zones.len()
        ));
    }
    let objects = BTreeMap::from([
        ("tracks".into(), board.tracks.len()),
        ("vias".into(), board.vias.len()),
        ("pads".into(), board.pads.len()),
        ("footprints".into(), board.parts.len()),
    ]);
    let coverage = rules::catalog()
        .into_iter()
        .map(|r| {
            let status = if !config.enabled(&r.id) {
                "disabled"
            } else if let Some(s) = rule_coverage.get(&r.id) {
                s.as_str()
            } else if r.status != "implemented" {
                "planned"
            } else if !config.enabled(&r.id) {
                "disabled"
            } else if r.id.starts_with("contract.") && !rules::has_contract(&r.id, &config) {
                "needs_contract"
            } else {
                "evaluated"
            };
            Coverage {
                rule: r.id,
                status: status.into(),
            }
        })
        .collect();
    Ok(Report {
        schema: 1,
        tool_version: env!("CARGO_PKG_VERSION").into(),
        board_id: board_id.into(),
        source_sha256: hash(input),
        config,
        limitations,
        objects,
        findings,
        coverage,
        review_audit: vec![],
    })
}
pub mod cli;
