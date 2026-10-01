//! Power trace width checks (port of `explain/checks/power.py`).

use std::collections::HashSet;

use super::mistake;
use crate::explain::mistakes::{
    is_ground_net, is_power_net, CheckIncomplete, Mistake, MistakeCategory, MistakeCheck,
};
use crate::schema::pcb::Pcb;

/// Current (A) -> minimum recommended width (mm), IPC-2221 1oz/10C rise.
pub const POWER_TRACE_WIDTHS: &[(f64, f64)] =
    &[(0.5, 0.25), (1.0, 0.5), (2.0, 1.0), (3.0, 1.5), (5.0, 2.5)];

/// Minimum power trace width to flag (mm).
pub const MIN_POWER_TRACE_WIDTH_MM: f64 = 0.3;

/// Check that power traces are wide enough.
#[derive(Debug, Default, Clone, Copy)]
pub struct PowerTraceWidthCheck;

impl MistakeCheck for PowerTraceWidthCheck {
    fn name(&self) -> &'static str {
        "PowerTraceWidthCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::PowerTrace
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let power_nets: HashSet<i64> = pcb
            .nets()
            .iter()
            .filter(|n| is_power_net(&n.name) || is_ground_net(&n.name))
            .map(|n| n.number)
            .collect();

        let mut mistakes = Vec::new();
        for seg in pcb.segments() {
            if !power_nets.contains(&seg.net_number) {
                continue;
            }
            let net_name = pcb
                .get_net(seg.net_number)
                .map_or_else(|| format!("Net {}", seg.net_number), |n| n.name.clone());
            if seg.width < MIN_POWER_TRACE_WIDTH_MM {
                mistakes.push(mistake(
                    MistakeCategory::PowerTrace,
                    "warning",
                    "Power trace too narrow",
                    vec![net_name.clone()],
                    Some(seg.start),
                    format!(
                        "Power trace on {net_name} is only {:.2}mm wide. Narrow power traces \
                         cause voltage drop and can overheat under load. For 1A at 1oz copper, \
                         traces should be at least 0.5mm wide.",
                        seg.width
                    ),
                    format!(
                        "Increase trace width on {net_name} to at least 0.3mm, or use a copper \
                         pour for power distribution. Consider the expected current draw."
                    ),
                    "docs/mistakes/power-trace-width.md",
                ));
            }
        }

        // Deduplicate: one mistake per component tuple (net).
        let mut seen: HashSet<Vec<String>> = HashSet::new();
        Ok(mistakes
            .into_iter()
            .filter(|m| seen.insert(m.components.clone()))
            .collect())
    }
}
