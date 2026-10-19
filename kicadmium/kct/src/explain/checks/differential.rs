//! Differential pair routing checks (port of
//! `explain/checks/differential.py`).

use super::mistake;
use crate::explain::mistakes::{
    is_differential_pair_net, trace_length, CheckIncomplete, Mistake, MistakeCategory, MistakeCheck,
};
use crate::pyjson::{py_float_repr, py_title};
use crate::schema::pcb::{Pcb, Segment};

/// Maximum length mismatch for USB 2.0 high-speed (mm).
pub const MAX_USB_SKEW_MM: f64 = 2.0;
/// Maximum length mismatch for general differential pairs (mm).
pub const MAX_GENERAL_SKEW_MM: f64 = 5.0;

/// Check differential pair length matching.
#[derive(Debug, Default, Clone, Copy)]
pub struct DifferentialPairSkewCheck;

impl DifferentialPairSkewCheck {
    pub fn check_with_limits(
        pcb: &Pcb,
        limits: &[(String, String, f64)],
    ) -> Result<Vec<Mistake>, CheckIncomplete> {
        Self::check_pairs(pcb, limits)
    }
    /// Pair base name -> (positive net, negative net), in first-seen order.
    pub fn find_differential_pairs(pcb: &Pcb) -> Vec<(String, (String, String))> {
        let mut pairs: Vec<(String, Vec<String>)> = Vec::new();
        for net in pcb.nets() {
            let (is_diff, base) = is_differential_pair_net(&net.name);
            let Some(base) = base.filter(|b| is_diff && !b.is_empty()) else {
                continue;
            };
            match pairs.iter_mut().find(|(b, _)| *b == base) {
                Some((_, nets)) => nets.push(net.name.clone()),
                None => pairs.push((base, vec![net.name.clone()])),
            }
        }
        let mut out = Vec::new();
        for (base, mut nets) in pairs {
            if nets.len() != 2 {
                continue;
            }
            let key = |n: &String| {
                let up = n.to_uppercase();
                up.contains("_P") || n.contains('+') || up.ends_with('P') || up.contains("DP")
            };
            // `sorted(..., reverse=True)` on a bool key is stable: True first.
            nets.sort_by_key(|n| !key(n));
            let neg = nets.pop().unwrap_or_default();
            let pos = nets.pop().unwrap_or_default();
            out.push((base, (pos, neg)));
        }
        out
    }

    fn net_segments<'a>(pcb: &'a Pcb, net_name: &str) -> Vec<&'a Segment> {
        let Some(net) = pcb.get_net_by_name(net_name) else {
            return Vec::new();
        };
        pcb.segments()
            .iter()
            .filter(|s| s.net_number == net.number)
            .collect()
    }
}

impl MistakeCheck for DifferentialPairSkewCheck {
    fn name(&self) -> &'static str {
        "DifferentialPairSkewCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::DifferentialPair
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        Self::check_pairs(pcb, &[])
    }
}

impl DifferentialPairSkewCheck {
    fn check_pairs(
        pcb: &Pcb,
        limits: &[(String, String, f64)],
    ) -> Result<Vec<Mistake>, CheckIncomplete> {
        let mut mistakes = Vec::new();
        let mut pairs = Self::find_differential_pairs(pcb);
        for (p, n, mm) in limits {
            if !mm.is_finite()
                || *mm <= 0.0
                || p == n
                || pcb.get_net_by_name(p).is_none()
                || pcb.get_net_by_name(n).is_none()
            {
                return Err(CheckIncomplete::new(
                    "pair skew requires two existing distinct nets and a finite positive budget",
                ));
            }
            if !pairs
                .iter()
                .any(|(_, (a, b))| (a == p && b == n) || (a == n && b == p))
            {
                pairs.push((p.clone(), (p.clone(), n.clone())));
            }
        }
        for (base, (pos_net, neg_net)) in pairs {
            let pos_segs = Self::net_segments(pcb, &pos_net);
            let neg_segs = Self::net_segments(pcb, &neg_net);
            if pos_segs.is_empty() || neg_segs.is_empty() {
                continue;
            }
            let pos_len = trace_length(pos_segs);
            let neg_len = trace_length(neg_segs);
            let skew = (pos_len - neg_len).abs();

            let is_usb = base.to_uppercase().contains("USB");
            let declared = limits.iter().find(|(p, n, _)| {
                (p == &pos_net && n == &neg_net) || (p == &neg_net && n == &pos_net)
            });
            if is_usb && declared.is_none() {
                mistakes.push(mistake(
                    MistakeCategory::DifferentialPair, "info", "USB pair speed is undeclared",
                    vec![pos_net.clone(), neg_net.clone()], None,
                    format!("{pos_net} is {pos_len:.1}mm, {neg_net} is {neg_len:.1}mm ({skew:.1}mm difference). USB speed cannot be inferred from net names; no High-Speed limit was applied."),
                    "Declare the reviewed pair skew budget with --pair-skew POSITIVE,NEGATIVE,MM. Choose the budget for the actual interface speed and device requirements.".into(),
                    "docs/mistakes/differential-pair-matching.md",
                ));
                continue;
            }
            let max_skew = declared
                .map(|(_, _, mm)| *mm)
                .unwrap_or(MAX_GENERAL_SKEW_MM);
            let interface = if declared.is_some() {
                "Declared interface budget"
            } else {
                "differential"
            };
            let max_skew_s = py_float_repr(max_skew);

            if skew > max_skew {
                let (longer, shorter) = if pos_len > neg_len {
                    (&pos_net, &neg_net)
                } else {
                    (&neg_net, &pos_net)
                };
                let target = if neg_len > pos_len { neg_len } else { pos_len };
                mistakes.push(mistake(
                    MistakeCategory::DifferentialPair,
                    if declared.is_some() {
                        "error"
                    } else {
                        "warning"
                    },
                    "Differential pair length mismatch",
                    vec![pos_net.clone(), neg_net.clone()],
                    None,
                    format!(
                        "{pos_net} is {pos_len:.1}mm, {neg_net} is {neg_len:.1}mm ({skew:.1}mm \
                         difference). {} requires < {max_skew_s}mm length difference to maintain \
                         signal timing. Mismatched lengths cause timing skew that degrades signal \
                         quality.",
                        py_title(interface)
                    ),
                    format!(
                        "Add serpentine to {shorter} to match {longer} length, or shorten the \
                         {longer} routing path. Target: {target:.1}mm for both traces."
                    ),
                    "docs/mistakes/differential-pair-matching.md",
                ));
            } else if skew > max_skew * 0.5 {
                mistakes.push(mistake(
                    MistakeCategory::DifferentialPair,
                    "info",
                    "Differential pair length matching could be improved",
                    vec![pos_net.clone(), neg_net.clone()],
                    None,
                    format!(
                        "{pos_net}/{neg_net} have {skew:.1}mm length difference. While within \
                         spec ({max_skew_s}mm max), better matching improves signal integrity \
                         margin."
                    ),
                    format!(
                        "Consider adding serpentine to improve length matching. Current: \
                         {pos_len:.1}mm / {neg_len:.1}mm."
                    ),
                    "docs/mistakes/differential-pair-matching.md",
                ));
            }
        }
        Ok(mistakes)
    }
}
