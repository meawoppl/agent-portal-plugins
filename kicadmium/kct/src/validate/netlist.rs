//! Schematic-to-PCB netlist synchronization (port of
//! `kicad_tools.validate.netlist`, behind `kct validate --sync`).
//!
//! Upstream compares component references plus global-label name variants
//! and leaves pad-to-net checking as a placeholder. That reports boards with
//! hundreds of KiCad schematic-parity findings as in sync, so the native port
//! also checks, per matched component, every pad net against KiCad's own
//! schematic netlist (`kicad-cli sch export netlist`), pins without pads and
//! pads without pins, and footprint library IDs. The native schematic
//! connectivity extractor is not accurate enough to judge net names, so
//! without kicad-cli the net checks are reported as not evaluated (a
//! warning) instead of passing silently.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::operations::netlist::Netlist;
use crate::schema::pcb::Pcb;

/// One synchronization finding (upstream `SyncIssue.to_dict` keys).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncIssue {
    pub severity: String,
    pub category: String,
    pub message: String,
    pub suggestion: String,
    pub reference: String,
    pub net_schematic: String,
    pub net_pcb: String,
    pub pin: String,
}

impl SyncIssue {
    fn new(severity: &str, category: &str, message: String, suggestion: String) -> Self {
        SyncIssue {
            severity: severity.into(),
            category: category.into(),
            message,
            suggestion,
            reference: String::new(),
            net_schematic: String::new(),
            net_pcb: String::new(),
            pin: String::new(),
        }
    }
    fn reference(mut self, r: &str) -> Self {
        self.reference = r.into();
        self
    }
    fn pin(mut self, p: &str) -> Self {
        self.pin = p.into();
        self
    }
    fn nets(mut self, sch: &str, pcb: &str) -> Self {
        self.net_schematic = sch.into();
        self.net_pcb = pcb.into();
        self
    }
    pub fn is_error(&self) -> bool {
        self.severity == "error"
    }
}

#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    pub issues: Vec<SyncIssue>,
    /// Where the schematic netlist came from (`kicad-cli`, or `none`).
    pub netlist_source: String,
}

impl SyncResult {
    pub fn error_count(&self) -> usize {
        self.issues.iter().filter(|i| i.is_error()).count()
    }
    pub fn warning_count(&self) -> usize {
        self.issues.len() - self.error_count()
    }
    pub fn in_sync(&self) -> bool {
        self.error_count() == 0
    }
    pub fn count(&self, category: &str) -> usize {
        self.issues
            .iter()
            .filter(|i| i.category == category)
            .count()
    }
}

/// Schematic side of a component (from the hierarchical BOM).
#[derive(Debug, Clone)]
pub struct SchComponent {
    pub reference: String,
    pub footprint: String,
}

/// Collect on-board, non-power schematic components (upstream
/// `_collect_schematic_refs` with `hierarchical=True`).
pub fn schematic_components(sch: &Path) -> Result<Vec<SchComponent>> {
    let bom = crate::schema::bom::extract_bom(&sch.to_string_lossy(), true)?;
    Ok(bom
        .items
        .into_iter()
        .filter(|i| !i.is_power_symbol() && i.on_board && !i.reference.starts_with('#'))
        .filter(|i| !i.reference.is_empty())
        .map(|i| SchComponent {
            reference: i.reference,
            footprint: i.footprint,
        })
        .collect())
}

/// Export KiCad's own schematic netlist into a temp file.
pub fn kicad_netlist(sch: &Path) -> Result<Netlist> {
    let tmp = std::env::temp_dir().join(format!(
        "kct-sync-{}-{}.kicad_net",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let out =
        crate::operations::netlist::export_netlist(sch, Some(&tmp), None, "kicadsexpr", false);
    let _ = std::fs::remove_file(&tmp);
    out
}

fn split_lib_id(id: &str) -> (&str, &str) {
    match id.split_once(':') {
        Some((lib, name)) => (lib, name),
        None => ("", id),
    }
}

/// Compare schematic components (and, when given, KiCad's schematic
/// netlist) against the board.
pub fn validate_sync(sch: &[SchComponent], pcb: &Pcb, netlist: Option<&Netlist>) -> SyncResult {
    let mut result = SyncResult {
        netlist_source: if netlist.is_some() {
            "kicad-cli"
        } else {
            "none"
        }
        .into(),
        ..Default::default()
    };
    let sm: BTreeMap<&str, &SchComponent> = sch.iter().map(|c| (c.reference.as_str(), c)).collect();
    let pm: BTreeMap<&str, &crate::schema::pcb::Footprint> = pcb
        .footprints()
        .iter()
        .filter(|f| !f.reference.is_empty() && !f.reference.starts_with('#'))
        .map(|f| (f.reference.as_str(), f))
        .collect();

    for (r, c) in &sm {
        if !pm.contains_key(r) {
            let fp = if c.footprint.is_empty() {
                String::new()
            } else {
                format!(" ({})", c.footprint)
            };
            result.issues.push(
                SyncIssue::new(
                    "error",
                    "missing_on_pcb",
                    format!("{r} missing on PCB"),
                    format!("Add footprint for {r}{fp}"),
                )
                .reference(r),
            );
        }
    }
    for r in pm.keys().filter(|r| !sm.contains_key(*r)) {
        result.issues.push(
            SyncIssue::new(
                "warning",
                "orphaned_on_pcb",
                format!("{r} on PCB has no schematic symbol"),
                format!("Remove {r} from PCB or add to schematic"),
            )
            .reference(r),
        );
    }

    // Footprint library IDs (KiCad `footprint_symbol_mismatch`).
    for (r, c) in &sm {
        let Some(fp) = pm.get(r) else { continue };
        if c.footprint.is_empty() || fp.name.is_empty() || c.footprint == fp.name {
            continue;
        }
        // Same footprint name under a different (or missing) library
        // nickname is a warning, as in KiCad; a different footprint is not.
        let (severity, what) = if split_lib_id(&c.footprint).1 != split_lib_id(&fp.name).1 {
            ("error", "footprint")
        } else {
            ("warning", "footprint library")
        };
        result.issues.push(
            SyncIssue::new(
                severity,
                "footprint_mismatch",
                format!(
                    "{r} {what} differs: schematic \"{}\", PCB \"{}\"",
                    c.footprint, fp.name
                ),
                format!(
                    "Update {r} from the schematic so the footprint is \"{}\"",
                    c.footprint
                ),
            )
            .reference(r),
        );
    }

    let Some(nl) = netlist else {
        result.issues.push(SyncIssue::new(
            "warning",
            "coverage",
            "Pad-to-net synchronization not evaluated: kicad-cli could not export the \
             schematic netlist"
                .into(),
            "Install KiCad (kicad-cli) or set KICADMIUM_KICAD_CLI to compare pad nets".into(),
        ));
        return result;
    };

    // (ref, pin) -> schematic net name.
    let mut pin_net: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    let mut ref_pins: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for net in &nl.nets {
        for node in &net.nodes {
            pin_net.insert((&node.reference, &node.pin), &net.name);
            ref_pins
                .entry(&node.reference)
                .or_default()
                .insert(&node.pin);
        }
    }
    for (r, fp) in &pm {
        if !sm.contains_key(r) {
            continue;
        }
        let mut pads_seen: BTreeSet<&str> = BTreeSet::new();
        for pad in &fp.pads {
            if pad.number.is_empty() || pad.pad_type == "np_thru_hole" {
                continue;
            }
            pads_seen.insert(&pad.number);
            match pin_net.get(&(*r, pad.number.as_str())) {
                Some(sch_net) if *sch_net != pad.net_name => {
                    let shown = if pad.net_name.is_empty() {
                        "<no net>"
                    } else {
                        &pad.net_name
                    };
                    result.issues.push(
                        SyncIssue::new(
                            "error",
                            "net_mismatch",
                            format!(
                                "{r} pad {} is on \"{shown}\" but the schematic connects it to \
                                 \"{sch_net}\"",
                                pad.number
                            ),
                            "Update the PCB from the schematic (or fix the schematic)".into(),
                        )
                        .reference(r)
                        .pin(&pad.number)
                        .nets(sch_net, &pad.net_name),
                    );
                }
                Some(_) => {}
                None if !pad.net_name.is_empty() => {
                    result.issues.push(
                        SyncIssue::new(
                            "error",
                            "pin_mismatch",
                            format!(
                                "{r} pad {} is on \"{}\" but the schematic symbol has no pin {}",
                                pad.number, pad.net_name, pad.number
                            ),
                            "Fix the symbol pin numbering or the footprint pad numbering".into(),
                        )
                        .reference(r)
                        .pin(&pad.number)
                        .nets("", &pad.net_name),
                    );
                }
                None => {}
            }
        }
        for pin in ref_pins.get(r).into_iter().flatten() {
            if !pads_seen.contains(pin) {
                result.issues.push(
                    SyncIssue::new(
                        "error",
                        "pin_mismatch",
                        format!("{r} schematic pin {pin} has no pad on the PCB footprint"),
                        "Fix the symbol pin numbering or the footprint pad numbering".into(),
                    )
                    .reference(r)
                    .pin(pin)
                    .nets(pin_net.get(&(*r, *pin)).copied().unwrap_or(""), ""),
                );
            }
        }
    }
    result
}
