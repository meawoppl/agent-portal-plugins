//! Multi-pad nets not fully connected by copper (port of
//! `kicad_tools.validate.rules.connectivity`, Issue #3041).

use crate::analysis::net_status::NetStatusAnalyzer;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

#[derive(Debug, Clone)]
pub struct ConnectivityRule {
    pub strict: bool,
}

impl ConnectivityRule {
    pub fn new(strict: bool) -> Self {
        ConnectivityRule { strict }
    }

    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        results.set_rule("connectivity", 1);
        let analysis = NetStatusAnalyzer::new(pcb, self.strict).analyze();
        for ns in &analysis.nets {
            if ns.total_pads < 2 || ns.status() == "complete" {
                continue;
            }
            if ns.has_filled_zone && ns.is_advisory_incomplete() {
                continue;
            }
            let (loc, items) = match ns.unconnected_pads.first() {
                Some(p) => (Some(p.position), vec![p.full_name()]),
                None => (None, vec![]),
            };
            let message = if ns.status() == "unrouted" {
                format!(
                    "Net '{}' is unrouted: {} pads with no connecting copper (0/{} connected)",
                    ns.net_name, ns.total_pads, ns.total_pads
                )
            } else {
                format!(
                    "Net '{}' is partially routed: {} of {} pads stranded (connected island has {} \
                     pads)",
                    ns.net_name,
                    ns.unconnected_count(),
                    ns.total_pads,
                    ns.connected_count()
                )
            };
            results.add(
                DRCViolation::new("connectivity", "error", message)
                    .location_opt(loc)
                    .items(items)
                    .nets([ns.net_name.clone()]),
            );
        }
        results
    }
}
