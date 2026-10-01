//! Central `.kct_waivers.json` application for `kct check` (port of
//! `kicad_tools.validate.rules.waivers.apply_waivers`, Issue #4417). The
//! loader lives in [`crate::drc::waivers`] (shared with `kct drc`).

pub use crate::drc::waivers::{
    discover_waivers_sidecar, load_waivers, Waiver, Waivers, WAIVER_UNUSED_RULE_ID,
};
use crate::pyjson::{py_repr_str, py_repr_str_list};
use crate::validate::violations::{DRCResults, DRCViolation};

/// Mark matching findings waived and append `waiver_unused` advisories.
pub fn apply_waivers(results: &mut DRCResults, waivers: &Waivers) {
    if waivers.entries.is_empty() {
        return;
    }
    let mut used = vec![false; waivers.entries.len()];
    for v in &mut results.violations {
        if v.waived {
            continue;
        }
        let Some(idx) = waivers
            .entries
            .iter()
            .position(|e| e.matches(&v.rule_id, &v.items, &v.nets))
        else {
            continue;
        };
        used[idx] = true;
        let entry = &waivers.entries[idx];
        v.waived = true;
        v.waiver_reason = Some(entry.reason.clone());
        v.waiver_issue = Some(entry.issue.clone());
    }
    for (idx, entry) in waivers.entries.iter().enumerate() {
        if used[idx] {
            continue;
        }
        let items: Vec<&str> = entry.items.iter().map(String::as_str).collect();
        let nets: Vec<&str> = entry.nets.iter().map(String::as_str).collect();
        let mut scope = Vec::new();
        if !items.is_empty() {
            scope.push(format!("items={}", py_repr_str_list(&items)));
        }
        if !nets.is_empty() {
            scope.push(format!("nets={}", py_repr_str_list(&nets)));
        }
        results.violations.push(
            DRCViolation::new(
                WAIVER_UNUSED_RULE_ID,
                "info",
                format!(
                    "Waiver for rule {} ({}) matched no finding (tracking {}); the underlying \
                     defect may already be resolved, or the rule/refs may have changed.",
                    py_repr_str(&entry.rule),
                    scope.join(", "),
                    entry.issue
                ),
            )
            .items(items)
            .nets(nets),
        );
    }
}
