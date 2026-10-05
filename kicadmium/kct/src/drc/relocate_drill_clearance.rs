//! Port of `kicad_tools.drc.relocate_drill_clearance`: the generic,
//! manufacturer-floor-driven hole-to-hole via relocation post-pass.
//!
//! Driven by via/via drill pairs closer than `min_hole_to_hole_mm`, validated
//! by the clearance-safe engine in [`crate::cli::relocate_in_pad_vias`]. The
//! via participating in the most violations moves first; a via with no
//! clearance-legal location is left in place and reported (the pass never
//! introduces a new violation). Connectivity is preserved with short
//! same-net stubs on every connected copper layer.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use crate::cli::relocate_in_pad_vias::{
    check_clearance, check_stub_clearance, collect_smd_pads_by_net, collect_tht_pads, endpoint_at,
    pad_copper_layer, persist_via_with_stubs, PadsByNet, ThtPads, PLANE_DIRECTIONS,
};
use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::schema::pcb::{Pcb, Via};
use crate::validate::rules::via_in_pad::via_inside_pad;

/// Coincidence tolerance (mm) for "a stub already runs old->new on this layer".
const SEG_COINCIDENT_TOL: f64 = 1e-3;
/// Floating-point slack for the hole-to-hole comparison (mm).
const EPS: f64 = 1e-6;

/// Record of a via moved to satisfy the hole-to-hole floor.
#[derive(Debug, Clone, PartialEq)]
pub struct DrillClearanceRelocation {
    pub old_x: f64,
    pub old_y: f64,
    pub new_x: f64,
    pub new_y: f64,
    pub net: i64,
    pub net_name: String,
    pub uuid: String,
    pub stub_layers: Vec<String>,
}

/// Record of a hole-to-hole violation left in place (boxed in).
#[derive(Debug, Clone, PartialEq)]
pub struct DrillClearanceUnresolved {
    pub x: f64,
    pub y: f64,
    pub net: i64,
    pub net_name: String,
    pub uuid: String,
    pub reason: String,
}

/// Aggregate outcome of a hole-to-hole relocation pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrillClearanceRelocationResult {
    pub moved: Vec<DrillClearanceRelocation>,
    pub unresolved: Vec<DrillClearanceUnresolved>,
}

impl DrillClearanceRelocationResult {
    /// True when at least one via was moved.
    pub fn changed(&self) -> bool {
        !self.moved.is_empty()
    }

    /// Human-readable one-line-per-section summary.
    pub fn summary(&self) -> String {
        let mut lines = vec![format!(
            "Hole-to-hole relocation: moved {} via(s)",
            self.moved.len()
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
        if !self.unresolved.is_empty() {
            lines.push(format!(
                "  {} unresolved (boxed in, left in place):",
                self.unresolved.len()
            ));
            for u in &self.unresolved {
                lines.push(format!(
                    "    Via at ({:.3}, {:.3}) net '{}': {}",
                    u.x, u.y, u.net_name, u.reason
                ));
            }
        }
        lines.join("\n")
    }
}

/// `_resolve_net_name`: the via's net name, falling back to the net table.
pub fn resolve_net_name(pcb: &Pcb, via: &Via) -> String {
    if !via.net_name.is_empty() {
        return via.net_name.clone();
    }
    pcb.get_net(via.net_number)
        .map(|n| n.name.clone())
        .unwrap_or_default()
}

/// Edge-to-edge drill gap (mm) between two vias.
pub fn hole_gap(a: &Via, b: &Via) -> f64 {
    let d = (a.position.0 - b.position.0).hypot(a.position.1 - b.position.1);
    d - a.drill / 2.0 - b.drill / 2.0
}

/// All via index pairs whose drill gap is below the floor.
pub fn violating_pairs(vias: &[Via], min_hole_to_hole: f64) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for i in 0..vias.len() {
        for j in i + 1..vias.len() {
            if hole_gap(&vias[i], &vias[j]) < min_hole_to_hole - EPS {
                pairs.push((i, j));
            }
        }
    }
    pairs
}

fn near(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < SEG_COINCIDENT_TOL && (a.1 - b.1).abs() < SEG_COINCIDENT_TOL
}

/// True when a same-net segment already runs `a`->`b` on `layer`.
fn segment_exists(pcb: &Pcb, net: i64, layer: &str, a: (f64, f64), b: (f64, f64)) -> bool {
    pcb.segments_in_net(net).any(|s| {
        s.layer == layer
            && ((near(s.start, a) && near(s.end, b)) || (near(s.start, b) && near(s.end, a)))
    })
}

/// `_find_target`: first clearance-safe location clearing the floor.
#[allow(clippy::too_many_arguments)]
fn find_target(
    pcb: &Pcb,
    via_index: usize,
    escape_far: Option<(f64, f64)>,
    pads_by_net: &PadsByNet,
    tht_pads: &ThtPads,
    min_clearance: f64,
    min_hole_to_hole: f64,
    stub_layers: &[String],
    stub_width: f64,
) -> Result<Option<(f64, f64)>> {
    let via = &pcb.vias()[via_index];
    let (vx, vy) = via.position;
    let mut candidates = Vec::new();
    if let Some((ex, ey)) = escape_far {
        let dist = (ex - vx).hypot(ey - vy);
        if dist > EPS {
            candidates.push((ex, ey));
            let (ux, uy) = ((ex - vx) / dist, (ey - vy) / dist);
            for mult in [1.0, 1.25, 1.5, 2.0] {
                candidates.push((
                    vx + ux * min_hole_to_hole * mult,
                    vy + uy * min_hole_to_hole * mult,
                ));
            }
        }
    }
    for mult in [1.0, 1.25, 1.5, 2.0] {
        let reach = min_hole_to_hole * mult;
        for (dx, dy) in PLANE_DIRECTIONS {
            candidates.push((vx + dx * reach, vy + dy * reach));
        }
    }
    for (nx, ny) in candidates {
        if check_clearance(
            pcb,
            Some(via_index),
            via,
            nx,
            ny,
            pads_by_net,
            tht_pads,
            min_clearance,
            min_hole_to_hole,
            None,
        )?
        .is_none()
            && check_stub_clearance(
                pcb,
                via_index,
                via,
                (nx, ny),
                stub_layers,
                stub_width,
                min_clearance,
                None,
            )?
            .is_none()
        {
            return Ok(Some((nx, ny)));
        }
    }
    Ok(None)
}

/// `_try_relocate`: one clearance-safe relocation of `pcb.vias()[via_index]`.
///
/// Returns `None` when the via is boxed in or the move could not be
/// persisted (the via is then left untouched).
pub fn try_relocate(
    pcb: &mut Pcb,
    via_index: usize,
    pads_by_net: &PadsByNet,
    tht_pads: &ThtPads,
    min_clearance: f64,
    min_hole_to_hole: f64,
    dry_run: bool,
) -> Result<Option<DrillClearanceRelocation>> {
    let via = pcb.vias()[via_index].clone();
    let (vx, vy) = via.position;
    let net_name = resolve_net_name(pcb, &via);

    let fps = pcb.footprints();
    let containing_pad = pads_by_net
        .get(&via.net_number)
        .into_iter()
        .flatten()
        .find(|(_, _, b)| via_inside_pad(&via, *b, None))
        .map(|&(fi, pi, _)| fps[fi].pads[pi].clone());

    let connected: Vec<(String, f64, (f64, f64))> = pcb
        .segments_in_net(via.net_number)
        .filter_map(|s| endpoint_at(s, vx, vy).map(|far| (s.layer.clone(), s.width, far)))
        .collect();

    // Python `max` keeps the first maximal element.
    let mut escape_far: Option<(f64, f64)> = None;
    let mut best = f64::NEG_INFINITY;
    for (_, _, far) in &connected {
        let d = (far.0 - vx).hypot(far.1 - vy);
        if d > best {
            best = d;
            escape_far = Some(*far);
        }
    }

    let mut stub_layers: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();
    if let Some(pad) = &containing_pad {
        sources.push(pad_copper_layer(pad));
    }
    sources.extend(connected.iter().map(|(l, _, _)| l.clone()));
    for layer in sources {
        if !layer.is_empty() && !stub_layers.contains(&layer) {
            stub_layers.push(layer);
        }
    }
    let widths: Vec<f64> = connected.iter().map(|c| c.1).filter(|w| *w > 0.0).collect();
    let stub_width = if widths.is_empty() {
        0.2
    } else {
        widths.iter().cloned().fold(f64::INFINITY, f64::min)
    };

    let Some(target) = find_target(
        pcb,
        via_index,
        escape_far,
        pads_by_net,
        tht_pads,
        min_clearance,
        min_hole_to_hole,
        &stub_layers,
        stub_width,
    )?
    else {
        return Ok(None);
    };

    if !dry_run {
        let additions: Vec<String> = stub_layers
            .iter()
            .filter(|l| !segment_exists(pcb, via.net_number, l, (vx, vy), target))
            .cloned()
            .collect();
        if !persist_via_with_stubs(pcb, via_index, target, &additions, stub_width, &net_name) {
            return Ok(None);
        }
    }

    Ok(Some(DrillClearanceRelocation {
        old_x: vx,
        old_y: vy,
        new_x: target.0,
        new_y: target.1,
        net: via.net_number,
        net_name,
        uuid: via.uuid,
        stub_layers,
    }))
}

/// `relocate_drill_clearance`: relocate vias to satisfy the hole-to-hole floor.
///
/// Not gated on `via_in_pad_supported`. `nets` restricts the pass to the
/// given net names; `dry_run` simulates on a copy.
pub fn relocate_drill_clearance(
    pcb: &mut Pcb,
    rules: &DesignRules,
    nets: Option<&BTreeSet<String>>,
    dry_run: bool,
) -> Result<DrillClearanceRelocationResult> {
    if dry_run {
        let mut copy = pcb.clone();
        return relocate_drill_clearance(&mut copy, rules, nets, false);
    }
    let mut result = DrillClearanceRelocationResult::default();
    let min_clearance = rules.min_clearance_mm;
    let min_h2h = rules.min_hole_to_hole_mm;
    let pads = collect_smd_pads_by_net(pcb);
    let tht = collect_tht_pads(pcb);
    let mut failed: BTreeSet<String> = BTreeSet::new();

    let max_iterations = 4 * pcb.vias().len().max(1);
    for _ in 0..max_iterations {
        let pairs = violating_pairs(pcb.vias(), min_h2h);
        if pairs.is_empty() {
            break;
        }
        let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
        for (a, b) in &pairs {
            *counts.entry(*a).or_default() += 1;
            *counts.entry(*b).or_default() += 1;
        }
        // Insertion order (first appearance) for stable tie-breaks.
        let mut order: Vec<usize> = Vec::new();
        for (a, b) in &pairs {
            for v in [*a, *b] {
                if !order.contains(&v) {
                    order.push(v);
                }
            }
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
            let via = &pcb.vias()[vi];
            if failed.contains(&via.uuid) || via.net_number == 0 {
                continue;
            }
            let name = resolve_net_name(pcb, via);
            if nets.is_some_and(|n| !n.contains(&name)) {
                continue;
            }
            let uuid = via.uuid.clone();
            match try_relocate(pcb, vi, &pads, &tht, min_clearance, min_h2h, false)? {
                None => {
                    failed.insert(uuid);
                }
                Some(outcome) => {
                    result.moved.push(outcome);
                    moved_this_pass = true;
                    break;
                }
            }
        }
        if !moved_this_pass {
            break;
        }
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (a, b) in violating_pairs(pcb.vias(), min_h2h) {
        for vi in [a, b] {
            let via = &pcb.vias()[vi];
            if !seen.insert(via.uuid.clone()) {
                continue;
            }
            result.unresolved.push(DrillClearanceUnresolved {
                x: via.position.0,
                y: via.position.1,
                net: via.net_number,
                net_name: resolve_net_name(pcb, via),
                uuid: via.uuid.clone(),
                reason: "no clearance-legal location satisfies the hole-to-hole floor (boxed in)"
                    .into(),
            });
        }
    }
    Ok(result)
}
