//! Acid-trap (acute trace angle) checks (port of
//! `explain/checks/acid_trap.py`).

use super::mistake;
use crate::explain::mistakes::{CheckIncomplete, Mistake, MistakeCategory, MistakeCheck};
use crate::schema::pcb::{Pcb, Segment};

/// Minimum acceptable trace angle (degrees).
pub const MIN_TRACE_ANGLE_DEG: f64 = 90.0;

const TOLERANCE: f64 = 0.01;

/// Check for acute angles in traces (acid traps).
#[derive(Debug, Default, Clone, Copy)]
pub struct AcidTrapCheck;

fn find_junction(a: &Segment, b: &Segment) -> Option<(f64, f64)> {
    for p1 in [a.start, a.end] {
        for p2 in [b.start, b.end] {
            if (p1.0 - p2.0).abs() < TOLERANCE && (p1.1 - p2.1).abs() < TOLERANCE {
                return Some(p1);
            }
        }
    }
    None
}

fn calculate_angle(a: &Segment, b: &Segment, j: (f64, f64)) -> Option<f64> {
    let other = |s: &Segment| {
        if (s.start.0 - j.0).abs() < TOLERANCE && (s.start.1 - j.1).abs() < TOLERANCE {
            s.end
        } else {
            s.start
        }
    };
    let (p1, p2) = (other(a), other(b));
    let v1 = (p1.0 - j.0, p1.1 - j.1);
    let v2 = (p2.0 - j.0, p2.1 - j.1);
    let m1 = (v1.0.powi(2) + v1.1.powi(2)).sqrt();
    let m2 = (v2.0.powi(2) + v2.1.powi(2)).sqrt();
    if m1 == 0.0 || m2 == 0.0 {
        return None;
    }
    let cos = ((v1.0 * v2.0 + v1.1 * v2.1) / (m1 * m2)).clamp(-1.0, 1.0);
    Some(cos.acos().to_degrees())
}

impl MistakeCheck for AcidTrapCheck {
    fn name(&self) -> &'static str {
        "AcidTrapCheck"
    }

    fn category(&self) -> MistakeCategory {
        MistakeCategory::Manufacturability
    }

    fn check(&self, pcb: &Pcb) -> Result<Vec<Mistake>, CheckIncomplete> {
        let mut groups: Vec<((i64, String), Vec<&Segment>)> = Vec::new();
        for s in pcb.segments() {
            let key = (s.net_number, s.layer.clone());
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some(g) => g.1.push(s),
                None => groups.push((key, vec![s])),
            }
        }
        let mut out = Vec::new();
        for ((net, layer), segs) in &groups {
            for (i, a) in segs.iter().enumerate() {
                for b in &segs[i + 1..] {
                    let Some(j) = find_junction(a, b) else {
                        continue;
                    };
                    let Some(angle) = calculate_angle(a, b, j).filter(|a| *a < MIN_TRACE_ANGLE_DEG)
                    else {
                        continue;
                    };
                    let net_name = pcb
                        .get_net(*net)
                        .map(|n| n.name.clone())
                        .unwrap_or_else(|| format!("Net {net}"));
                    out.push(mistake(
                        MistakeCategory::Manufacturability,
                        "warning",
                        "Acute angle in trace (acid trap)",
                        vec![net_name.clone(), layer.clone()],
                        Some(j),
                        format!(
                            "Trace on {net_name} ({layer}) has a {angle:.0}° angle. Angles less \
                             than 90° can trap etchant during PCB manufacturing, causing \
                             over-etching at the vertex. This can weaken the trace and cause \
                             reliability issues."
                        ),
                        "Reroute the trace to use 45° or 90° angles instead. If the acute angle \
                         is unavoidable, add a teardrop at the junction to eliminate the sharp \
                         corner."
                            .into(),
                        "docs/mistakes/acid-traps.md",
                    ));
                }
            }
        }
        Ok(out)
    }
}
