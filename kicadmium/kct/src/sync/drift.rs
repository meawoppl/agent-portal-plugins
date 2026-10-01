//! Shared schematic/PCB drift reporting for `kct check` (port of
//! `kicad_tools.sync.drift`): the advisory banner and the blocking
//! `--netlist-sync` gate, both driven by [`Reconciler::analyze`].

use std::path::{Path, PathBuf};

use super::discover::resolve_schematic_for_pcb;
use super::reconciler::{Reconciler, SyncAnalysis};
use crate::pyjson::py_repr_str;

/// Run the reconciler; `None` when no schematic resolves or analysis fails.
pub fn analyze_drift(pcb_path: &Path, schematic: Option<&str>) -> Option<(SyncAnalysis, PathBuf)> {
    let resolved = match schematic.filter(|s| !s.is_empty()) {
        Some(s) => PathBuf::from(s),
        None => resolve_schematic_for_pcb(pcb_path)?,
    };
    if !resolved.exists() {
        return None;
    }
    let analysis = Reconciler::new(&resolved, pcb_path).ok()?.analyze().ok()?;
    Some((analysis, resolved))
}

fn summary_parts(a: &SyncAnalysis) -> Vec<String> {
    let mut parts = Vec::new();
    if !a.schematic_orphans.is_empty() {
        parts.push(format!("{} schematic-only", a.schematic_orphans.len()));
    }
    if !a.pcb_orphans.is_empty() {
        parts.push(format!("{} PCB-only", a.pcb_orphans.len()));
    }
    if !a.value_mismatches.is_empty() {
        parts.push(format!("{} value mismatch(es)", a.value_mismatches.len()));
    }
    if !a.footprint_mismatches.is_empty() {
        parts.push(format!("{} footprint mismatch(es)", a.footprint_mismatches.len()));
    }
    parts
}

/// Any of the four drift axes non-empty (suffix notes excluded).
pub fn has_drift(a: &SyncAnalysis) -> bool {
    !(a.schematic_orphans.is_empty()
        && a.pcb_orphans.is_empty()
        && a.value_mismatches.is_empty()
        && a.footprint_mismatches.is_empty())
}

fn render_suffix_notes(a: &SyncAnalysis) -> Vec<String> {
    if a.value_suffix_notes.is_empty() {
        return vec![];
    }
    let mut lines = vec![
        String::new(),
        format!(
            "Informational: same value, PCB adds rating suffix [{}]:",
            a.value_suffix_notes.len()
        ),
    ];
    for m in &a.value_suffix_notes {
        lines.push(format!(
            "  - {}: schematic={} pcb={}",
            m.reference,
            py_repr_str(&m.schematic_value),
            py_repr_str(&m.pcb_value)
        ));
    }
    lines
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One-line advisory banner, or `None` when in sync.
pub fn format_drift_banner(a: &SyncAnalysis, pcb_path: &Path) -> Option<String> {
    let parts = summary_parts(a);
    if parts.is_empty() {
        return None;
    }
    let name = file_name(pcb_path);
    Some(format!(
        "  WARNING: PCB out of sync with schematic -- {}. Run 'kct sync --analyze {name}' to \
         inspect (apply with 'kct pcb sync-netlist').",
        parts.join(", ")
    ))
}

/// Full add/drop/orphan report for `--netlist-sync`.
pub fn render_drift_report(a: &SyncAnalysis, pcb_path: &Path, schematic_path: &Path) -> String {
    let rule = "=".repeat(60);
    let pcb_name = file_name(pcb_path);
    let mut lines = vec![
        rule.clone(),
        "NETLIST SYNC CHECK".to_string(),
        rule,
        format!("PCB:       {pcb_name}"),
        format!("Schematic: {}", file_name(schematic_path)),
    ];
    if !has_drift(a) {
        lines.push(String::new());
        lines.push("IN SYNC - schematic and PCB component sets match.".into());
        lines.extend(render_suffix_notes(a));
        return lines.join("\n");
    }
    lines.push(String::new());
    lines.push(format!("OUT OF SYNC - {}.", summary_parts(a).join(", ")));
    if !a.schematic_orphans.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "Schematic-only (in schematic, missing from PCB) [{}]:",
            a.schematic_orphans.len()
        ));
        lines.extend(a.schematic_orphans.iter().map(|r| format!("  - {r}")));
    }
    if !a.pcb_orphans.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "PCB-only (on PCB, missing from schematic) [{}]:",
            a.pcb_orphans.len()
        ));
        lines.extend(a.pcb_orphans.iter().map(|r| format!("  - {r}")));
    }
    if !a.value_mismatches.is_empty() {
        lines.push(String::new());
        lines.push(format!("Value mismatches [{}]:", a.value_mismatches.len()));
        for m in &a.value_mismatches {
            lines.push(format!(
                "  - {}: schematic={} pcb={}",
                m.reference,
                py_repr_str(&m.schematic_value),
                py_repr_str(&m.pcb_value)
            ));
        }
    }
    if !a.footprint_mismatches.is_empty() {
        lines.push(String::new());
        lines.push(format!("Footprint mismatches [{}]:", a.footprint_mismatches.len()));
        for m in &a.footprint_mismatches {
            lines.push(format!(
                "  - {}: schematic={} pcb={}",
                m.reference,
                py_repr_str(&m.schematic_footprint),
                py_repr_str(&m.pcb_footprint)
            ));
        }
    }
    lines.extend(render_suffix_notes(a));
    lines.push(String::new());
    lines.push("Remediation:".into());
    lines.push(format!(
        "  kct sync --analyze {pcb_name}      # inspect proposed changes"
    ));
    lines.push(format!(
        "  kct pcb sync-netlist {pcb_name}    # apply changes to the PCB"
    ));
    lines.join("\n")
}

/// `_emit_drift_banner`: advisory, stderr, never affects the exit code.
pub fn emit_drift_banner(pcb_path: &Path, schematic: Option<&str>) {
    if let Some((a, _)) = analyze_drift(pcb_path, schematic) {
        if let Some(b) = format_drift_banner(&a, pcb_path) {
            eprintln!("{b}");
        }
    }
}

/// `run_netlist_sync_gate`: 0 in sync / advisory, 1 no schematic, 2 drift.
pub fn run_netlist_sync_gate(pcb_path: &Path, schematic: Option<&str>, strict: bool) -> anyhow::Result<i32> {
    let Some((a, resolved)) = analyze_drift(pcb_path, schematic) else {
        eprintln!(
            "Error: --netlist-sync requires a schematic, but none was found for {}.",
            file_name(pcb_path)
        );
        eprintln!(
            "Hint: pass --schematic <path>.kicad_sch, or place a sibling <basename>.kicad_sch \
             next to the PCB."
        );
        return Ok(1);
    };
    println!("{}", render_drift_report(&a, pcb_path, &resolved));
    if !a.schematic_orphans.is_empty() || !a.value_mismatches.is_empty() || !a.footprint_mismatches.is_empty() {
        return Ok(2);
    }
    if strict && has_drift(&a) {
        return Ok(2);
    }
    Ok(0)
}
