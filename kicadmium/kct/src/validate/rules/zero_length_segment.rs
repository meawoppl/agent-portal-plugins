//! Zero-length copper segment artifacts (port of
//! `kicad_tools.validate.rules.zero_length_segment`, issue #4651).

use crate::analysis::routing_quality::COORD_EPSILON_MM;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::Pcb;
use crate::validate::violations::{DRCResults, DRCViolation};

#[derive(Debug, Clone, Default)]
pub struct ZeroLengthSegmentRule;

impl ZeroLengthSegmentRule {
    pub fn check(&self, pcb: &Pcb, _rules: &DesignRules) -> DRCResults {
        let mut results = DRCResults::with_rules_checked(1);
        let (ox, oy) = pcb.board_origin();
        for s in pcb.segments() {
            let dx = s.end.0 - s.start.0;
            let dy = s.end.1 - s.start.1;
            if dx.hypot(dy) > COORD_EPSILON_MM {
                continue;
            }
            let (x, y) = s.start;
            let label = if s.net_name.is_empty() {
                format!("net {}", s.net_number)
            } else {
                s.net_name.clone()
            };
            let mut v = DRCViolation::new(
                "zero_length_segment",
                "warning",
                format!(
                    "Zero-length segment on '{label}' ({}) -- routing artifact carrying no \
                     copper; remove it (width {:.3} mm)",
                    s.layer, s.width
                ),
            )
            .at(x, y)
            .layer(s.layer.clone())
            .actual(0.0)
            .items([format!("{}@({:.4},{:.4})", s.layer, x + ox, y + oy)]);
            if !s.net_name.is_empty() {
                v = v.nets([s.net_name.clone()]);
            }
            results.add(v);
        }
        results
    }
}
