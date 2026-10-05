//! Grid-independent different-net copper short detection (port of the
//! detection half of `kicad_tools.drc.different_net_short`).
//!
//! The repair pass (`repair_different_net_shorts`) relocates vias through
//! the shared `relocate_drill_clearance` / `relocate_in_pad_vias` engine.

use crate::core::geometry::{point_to_segment_distance, segment_to_segment_distance};
use crate::pyjson::py_round;
use crate::schema::pcb::{Pcb, Segment, Via};

const EPS: f64 = 1e-4;
const BOTTOM_ORDINAL: i64 = 10_000;

/// Top-to-bottom ordinal of a copper layer name (`F.Cu` 0, `In{k}.Cu` k,
/// `B.Cu` last); `None` for non-copper names.
fn copper_ordinal(name: &str) -> Option<i64> {
    match name {
        "F.Cu" => Some(0),
        "B.Cu" => Some(BOTTOM_ORDINAL),
        n if n.starts_with("In") && n.ends_with(".Cu") && n.len() >= 5 => {
            n[2..n.len() - 3].trim().parse().ok()
        }
        _ => None,
    }
}

fn via_span(v: &Via) -> (i64, i64) {
    let ords: Vec<i64> = v.layers.iter().filter_map(|l| copper_ordinal(l)).collect();
    match (ords.iter().min(), ords.iter().max()) {
        (Some(lo), Some(hi)) => (*lo, *hi),
        _ => (0, BOTTOM_ORDINAL),
    }
}

fn via_spans_layer(v: &Via, layer: &str) -> bool {
    let Some(o) = copper_ordinal(layer) else {
        return false;
    };
    let (lo, hi) = via_span(v);
    lo <= o && o <= hi
}

fn spans_overlap(a: &Via, b: &Via) -> bool {
    let (alo, ahi) = via_span(a);
    let (blo, bhi) = via_span(b);
    alo.max(blo) <= ahi.min(bhi)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NetKey {
    Id(i64),
    Name(String),
}

fn net_identity(pcb: &Pcb, number: i64, name: &str) -> Option<(NetKey, String)> {
    if number != 0 {
        let display = if name.is_empty() {
            pcb.get_net(number)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| number.to_string())
        } else {
            name.to_string()
        };
        return Some((NetKey::Id(number), display));
    }
    (!name.is_empty()).then(|| (NetKey::Name(name.to_string()), name.to_string()))
}

/// A geometric different-net copper overlap the grid model missed.
#[derive(Debug, Clone, PartialEq)]
pub struct ShortItem {
    /// `"via-via"`, `"via-segment"` or `"segment-segment"`.
    pub kind: &'static str,
    pub net_a_name: String,
    pub net_b_name: String,
    pub layer: String,
    pub x: f64,
    pub y: f64,
    /// Edge-to-edge gap (negative = overlap depth).
    pub gap: f64,
    /// Indices into `pcb.vias()` of the participating vias.
    pub via_a: Option<usize>,
    pub via_b: Option<usize>,
}

impl ShortItem {
    pub fn describe(&self) -> String {
        format!(
            "{} short {}/{} on {} at ({:.3}, {:.3}); gap {:.3}mm",
            self.kind, self.net_a_name, self.net_b_name, self.layer, self.x, self.y, self.gap
        )
    }
}

fn seg_bbox(s: &Segment) -> (f64, f64, f64, f64) {
    (
        s.start.0.min(s.end.0),
        s.start.1.min(s.end.1),
        s.start.0.max(s.end.0),
        s.start.1.max(s.end.1),
    )
}

fn ordinal_layer_name(pcb: &Pcb, ordinal: i64) -> String {
    for l in pcb.copper_layers() {
        if copper_ordinal(&l.name) == Some(ordinal) {
            return l.name.clone();
        }
    }
    if ordinal == 0 {
        "F.Cu".into()
    } else if ordinal >= BOTTOM_ORDINAL {
        "B.Cu".into()
    } else {
        format!("In{ordinal}.Cu")
    }
}

/// Flag different-net via/segment pairs whose copper gap is below
/// `clearance` (default `0.0`: actual overlaps only), layer-aware.
pub fn find_different_net_shorts(pcb: &Pcb, clearance: f64) -> Vec<ShortItem> {
    let threshold = clearance - EPS;
    let vias = pcb.vias();
    let segs = pcb.segments();
    let vid: Vec<_> = vias
        .iter()
        .map(|v| net_identity(pcb, v.net_number, &v.net_name))
        .collect();
    let sid: Vec<_> = segs
        .iter()
        .map(|s| net_identity(pcb, s.net_number, &s.net_name))
        .collect();
    let mut out = Vec::new();
    for i in 0..vias.len() {
        let Some(a) = &vid[i] else {
            continue;
        };
        for j in i + 1..vias.len() {
            let Some(b) = &vid[j] else {
                continue;
            };
            if a.0 == b.0 || !spans_overlap(&vias[i], &vias[j]) {
                continue;
            }
            let (p, q) = (vias[i].position, vias[j].position);
            let d = crate::utils::pymath::hypot(p.0 - q.0, p.1 - q.1);
            let gap = d - vias[i].size / 2.0 - vias[j].size / 2.0;
            if gap < threshold {
                let lo = via_span(&vias[i]).0.max(via_span(&vias[j]).0);
                out.push(ShortItem {
                    kind: "via-via",
                    net_a_name: a.1.clone(),
                    net_b_name: b.1.clone(),
                    layer: ordinal_layer_name(pcb, lo),
                    x: (p.0 + q.0) / 2.0,
                    y: (p.1 + q.1) / 2.0,
                    gap,
                    via_a: Some(i),
                    via_b: Some(j),
                });
            }
        }
    }
    for (i, v) in vias.iter().enumerate() {
        let Some(a) = &vid[i] else {
            continue;
        };
        for (k, s) in segs.iter().enumerate() {
            let Some(b) = &sid[k] else {
                continue;
            };
            if a.0 == b.0 || !via_spans_layer(v, &s.layer) {
                continue;
            }
            let d = point_to_segment_distance(
                v.position.0,
                v.position.1,
                s.start.0,
                s.start.1,
                s.end.0,
                s.end.1,
            );
            let gap = d - v.size / 2.0 - s.width / 2.0;
            if gap < threshold {
                out.push(ShortItem {
                    kind: "via-segment",
                    net_a_name: a.1.clone(),
                    net_b_name: b.1.clone(),
                    layer: s.layer.clone(),
                    x: v.position.0,
                    y: v.position.1,
                    gap,
                    via_a: Some(i),
                    via_b: None,
                });
            }
        }
    }
    for ai in 0..segs.len() {
        let Some(ida) = &sid[ai] else {
            continue;
        };
        let sa = &segs[ai];
        let (ax0, ay0, ax1, ay1) = seg_bbox(sa);
        let pad_a = sa.width / 2.0 + clearance;
        for bi in ai + 1..segs.len() {
            let sb = &segs[bi];
            let Some(idb) = &sid[bi] else {
                continue;
            };
            if ida.0 == idb.0 || sa.layer != sb.layer {
                continue;
            }
            let (bx0, by0, bx1, by1) = seg_bbox(sb);
            let pad = pad_a + sb.width / 2.0;
            if ax1 + pad < bx0 || bx1 + pad < ax0 || ay1 + pad < by0 || by1 + pad < ay0 {
                continue;
            }
            let d = segment_to_segment_distance(
                sa.start.0, sa.start.1, sa.end.0, sa.end.1, sb.start.0, sb.start.1, sb.end.0,
                sb.end.1,
            );
            let gap = d - sa.width / 2.0 - sb.width / 2.0;
            if gap < threshold {
                out.push(ShortItem {
                    kind: "segment-segment",
                    net_a_name: ida.1.clone(),
                    net_b_name: idb.1.clone(),
                    layer: sa.layer.clone(),
                    x: (sa.start.0 + sa.end.0 + sb.start.0 + sb.end.0) / 4.0,
                    y: (sa.start.1 + sa.end.1 + sb.start.1 + sb.end.1) / 4.0,
                    gap,
                    via_a: None,
                    via_b: None,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        (a.kind, &a.net_a_name, &a.net_b_name)
            .cmp(&(b.kind, &b.net_a_name, &b.net_b_name))
            .then(py_round(a.x, 3).total_cmp(&py_round(b.x, 3)))
            .then(py_round(a.y, 3).total_cmp(&py_round(b.y, 3)))
    });
    out
}

/// Record of a via relocated to clear a different-net short.
#[derive(Debug, Clone, PartialEq)]
pub struct ShortRepairMove {
    pub old_x: f64,
    pub old_y: f64,
    pub new_x: f64,
    pub new_y: f64,
    pub net_name: String,
    pub uuid: String,
    pub stub_layers: Vec<String>,
}

/// Record of a short left in place.
#[derive(Debug, Clone, PartialEq)]
pub struct ShortRepairUnresolved {
    pub kind: String,
    pub net_a_name: String,
    pub net_b_name: String,
    pub layer: String,
    pub x: f64,
    pub y: f64,
    pub reason: String,
}

/// Aggregate outcome of a different-net short repair pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShortRepairResult {
    pub moved: Vec<ShortRepairMove>,
    pub unresolved: Vec<ShortRepairUnresolved>,
}

impl ShortRepairResult {
    pub fn changed(&self) -> bool {
        !self.moved.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut lines = vec![format!(
            "Different-net short repair: moved {} via(s), {} unresolved",
            self.moved.len(),
            self.unresolved.len()
        )];
        for m in &self.moved {
            let id: String = m.uuid.chars().take(8).collect();
            let stubs = if m.stub_layers.is_empty() {
                "(none)".to_string()
            } else {
                m.stub_layers.join(", ")
            };
            lines.push(format!(
                "  Via {} (net '{}'): ({:.3}, {:.3}) -> ({:.3}, {:.3}); stubs on {stubs}",
                if id.is_empty() { "?".to_string() } else { id },
                m.net_name,
                m.old_x,
                m.old_y,
                m.new_x,
                m.new_y
            ));
        }
        for u in &self.unresolved {
            lines.push(format!(
                "  UNRESOLVED {} {}/{} on {} at ({:.3}, {:.3}): {}",
                u.kind, u.net_a_name, u.net_b_name, u.layer, u.x, u.y, u.reason
            ));
        }
        lines.join("\n")
    }
}

/// `repair_different_net_shorts`: relocate offending vias through the shared
/// clearance-safe candidate ladder
/// ([`crate::drc::relocate_drill_clearance::try_relocate`]). Pure
/// segment-vs-segment shorts and boxed-in vias are reported as unresolved.
pub fn repair_different_net_shorts(
    pcb: &mut Pcb,
    rules: &crate::manufacturers::DesignRules,
    detect_clearance: f64,
    dry_run: bool,
) -> anyhow::Result<ShortRepairResult> {
    use crate::cli::relocate_in_pad_vias::{collect_smd_pads_by_net, collect_tht_pads};
    use crate::drc::relocate_drill_clearance::try_relocate;
    use std::collections::{BTreeMap, BTreeSet};

    let mut result = ShortRepairResult::default();
    let (min_clearance, min_h2h) = (rules.min_clearance_mm, rules.min_hole_to_hole_mm);
    let pads = collect_smd_pads_by_net(pcb);
    let tht = collect_tht_pads(pcb);
    let mut failed: BTreeSet<usize> = BTreeSet::new();

    let max_iterations = 4 * pcb.vias().len().max(1);
    for _ in 0..max_iterations {
        let shorts = find_different_net_shorts(pcb, detect_clearance);
        let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
        let mut order: Vec<usize> = Vec::new();
        for s in shorts
            .iter()
            .filter(|s| s.via_a.is_some() || s.via_b.is_some())
        {
            for v in [s.via_a, s.via_b].into_iter().flatten() {
                *counts.entry(v).or_default() += 1;
                if !order.contains(&v) {
                    order.push(v);
                }
            }
        }
        if order.is_empty() {
            break;
        }
        let vias = pcb.vias();
        order.sort_by(|&x, &y| {
            counts[&y]
                .cmp(&counts[&x])
                .then(py_round(vias[x].position.0, 4).total_cmp(&py_round(vias[y].position.0, 4)))
                .then(py_round(vias[x].position.1, 4).total_cmp(&py_round(vias[y].position.1, 4)))
        });
        let mut moved_this_pass = false;
        for vi in order {
            if failed.contains(&vi) || pcb.vias()[vi].net_number == 0 {
                continue;
            }
            match try_relocate(pcb, vi, &pads, &tht, min_clearance, min_h2h, dry_run)? {
                None => {
                    failed.insert(vi);
                }
                Some(o) => {
                    result.moved.push(ShortRepairMove {
                        old_x: o.old_x,
                        old_y: o.old_y,
                        new_x: o.new_x,
                        new_y: o.new_y,
                        net_name: o.net_name,
                        uuid: o.uuid,
                        stub_layers: o.stub_layers,
                    });
                    moved_this_pass = true;
                    if dry_run {
                        failed.insert(vi);
                    }
                    break;
                }
            }
        }
        if !moved_this_pass {
            break;
        }
    }

    for s in find_different_net_shorts(pcb, detect_clearance) {
        let reason = if s.via_a.is_none() && s.via_b.is_none() {
            "segment-vs-segment overlap (no via to relocate)"
        } else {
            "no clearance-legal location for the offending via (boxed in)"
        };
        result.unresolved.push(ShortRepairUnresolved {
            kind: s.kind.to_string(),
            net_a_name: s.net_a_name,
            net_b_name: s.net_b_name,
            layer: s.layer,
            x: s.x,
            y: s.y,
            reason: reason.into(),
        });
    }
    Ok(result)
}
