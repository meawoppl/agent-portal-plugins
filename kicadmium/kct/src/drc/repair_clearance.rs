//! Port of `kicad_tools.drc.repair_clearance`: non-destructive clearance
//! repair by nudging traces/vias (and optionally footprints) by the DRC
//! deficit plus margin, with an optional local A* reroute phase.
//!
//! Objects are addressed by child-index paths into the raw tree (upstream
//! holds `SExp` node references); paths are always re-resolved after a
//! structural edit.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::local_rerouter::{first_f, first_text, xy_of, LocalRerouter};
use super::net_compat::resolve_net_atom;
use super::repair_silkscreen::{descendant_paths, footprint_paths, node_at, node_at_mut, NodePath};
use super::report::DRCReport;
use super::violation::{extract_component_refs, DRCViolation, ViolationType};
use crate::pyjson::py_round;
use crate::sexp::SExp;

#[derive(Debug, Clone, PartialEq)]
pub struct NudgeResult {
    pub object_type: String,
    pub x: f64,
    pub y: f64,
    pub net_name: String,
    pub layer: String,
    pub displacement_x: f64,
    pub displacement_y: f64,
    pub displacement_mm: f64,
    pub old_clearance_mm: f64,
    pub new_clearance_mm: f64,
    pub uuid: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FootprintNudgeResult {
    pub reference: String,
    pub x: f64,
    pub y: f64,
    pub displacement_x: f64,
    pub displacement_y: f64,
    pub displacement_mm: f64,
    pub old_clearance_mm: f64,
    pub new_clearance_mm: f64,
    pub other_reference: String,
}

/// Summary of a clearance repair operation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RepairResult {
    pub total_violations: i64,
    pub repaired: i64,
    pub skipped_no_location: i64,
    pub skipped_not_clearance: i64,
    pub skipped_no_delta: i64,
    pub skipped_exceeds_max: i64,
    pub skipped_infeasible: i64,
    pub relocated_vias: i64,
    pub endpoint_nudges: i64,
    pub local_rerouted: i64,
    pub cluster_rerouted: i64,
    pub skipped_no_local_route: i64,
    pub footprint_nudges: i64,
    pub footprint_skipped_locked: i64,
    pub footprint_skipped_connector: i64,
    pub footprint_skipped_same_component: i64,
    pub footprint_skipped_exceeds_max: i64,
    pub nudges: Vec<NudgeResult>,
    pub footprint_nudge_results: Vec<FootprintNudgeResult>,
}

impl RepairResult {
    pub fn success_rate(&self) -> f64 {
        if self.total_violations == 0 {
            return 1.0;
        }
        self.repaired as f64 / self.total_violations as f64
    }

    pub fn summary(&self) -> String {
        let mut lines = vec![format!(
            "Clearance Repair: {}/{} violations fixed",
            self.repaired, self.total_violations
        )];
        let mut add = |n: i64, label: &str| {
            if n > 0 {
                lines.push(format!("  {label}: {n}"));
            }
        };
        add(self.endpoint_nudges, "Endpoint nudges (via-preserving)");
        add(self.relocated_vias, "Via relocations");
        add(self.local_rerouted, "Local reroutes");
        add(self.cluster_rerouted, "Cluster reroutes");
        add(self.footprint_nudges, "Footprint nudges (pad-pad)");
        add(self.footprint_skipped_locked, "Skipped (footprint locked)");
        add(
            self.footprint_skipped_connector,
            "Skipped (connector footprint)",
        );
        add(
            self.footprint_skipped_same_component,
            "Skipped (same component pads)",
        );
        add(
            self.skipped_exceeds_max,
            "Skipped (exceeds max displacement)",
        );
        add(self.skipped_infeasible, "Skipped (infeasible)");
        add(self.skipped_no_local_route, "Skipped (no local route)");
        add(self.skipped_no_location, "Skipped (no location)");
        add(self.skipped_no_delta, "Skipped (no delta info)");
        lines.join("\n")
    }
}

/// A located copper object: `(node, type, x, y, layer, net_name)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Obj {
    pub path: NodePath,
    pub kind: &'static str,
    pub x: f64,
    pub y: f64,
    pub layer: String,
    pub net: String,
}

/// Footprint lookup: `(node, ref, x, y, locked, pad_count, is_connector)`.
#[derive(Debug, Clone)]
struct FpInfo {
    path: NodePath,
    reference: String,
    x: f64,
    y: f64,
    locked: bool,
    pad_count: usize,
    is_connector: bool,
}

fn closest_point_on_segment(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    px: f64,
    py: f64,
) -> (f64, f64, f64) {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-10 {
        return (x1, y1, ((x1 - px).powi(2) + (y1 - py).powi(2)).sqrt());
    }
    let t = (((px - x1) * dx + (py - y1) * dy) / len_sq).clamp(0.0, 1.0);
    let cx = x1 + t * dx;
    let cy = y1 + t * dy;
    (cx, cy, ((cx - px).powi(2) + (cy - py).powi(2)).sqrt())
}

/// Displacement moving `obj` away from `other` by `required`.
pub fn compute_nudge(ox: f64, oy: f64, tx: f64, ty: f64, required: f64) -> (f64, f64, f64) {
    let dx = ox - tx;
    let dy = oy - ty;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 1e-10 {
        return (required, 0.0, required);
    }
    let scale = required / dist;
    let nx = dx * scale;
    let ny = dy * scale;
    (nx, ny, (nx * nx + ny * ny).sqrt())
}

fn is_zone_fill_violation(v: &DRCViolation) -> bool {
    v.items.iter().any(|i| {
        let l = i.to_lowercase();
        l.starts_with("zone") || l.contains("zone ")
    })
}

fn set_xy(node: &mut SExp, x: f64, y: f64) {
    node.set_value(0, x);
    node.set_value(1, y);
}

fn child_mut<'a>(node: &'a mut SExp, tag: &str) -> Option<&'a mut SExp> {
    super::repair_silkscreen::find_mut(node, tag)
}

/// Clearance repairer over a parsed board.
pub struct ClearanceRepairer {
    pub path: PathBuf,
    pub doc: SExp,
    pub modified: bool,
    pub nets: HashMap<i64, String>,
    pub net_names: HashMap<String, i64>,
    nudge_history: HashMap<String, (f64, f64)>,
}

impl ClearanceRepairer {
    pub fn new(pcb_path: &Path) -> Result<Self> {
        let doc = crate::sexp::parse_file(pcb_path)?;
        Ok(Self::from_doc(doc, pcb_path))
    }

    pub fn from_doc(doc: SExp, path: &Path) -> Self {
        let mut r = ClearanceRepairer {
            path: path.to_path_buf(),
            doc,
            modified: false,
            nets: HashMap::new(),
            net_names: HashMap::new(),
            nudge_history: HashMap::new(),
        };
        r.parse_nets();
        r
    }

    fn parse_nets(&mut self) {
        for child in &self.doc.children {
            if !child.has_tag("net") {
                continue;
            }
            let atoms: Vec<String> = (0..child.children.len())
                .filter_map(|i| child.text_at(i))
                .collect();
            if atoms.len() < 2 {
                continue;
            }
            let Ok(num) = atoms[0].trim().parse::<i64>() else {
                continue;
            };
            self.nets.insert(num, atoms[1].clone());
            self.net_names.insert(atoms[1].clone(), num);
        }
    }

    fn net_name_of(&self, node: &SExp) -> String {
        let atom = first_text(node.find("net"));
        resolve_net_atom(atom.as_deref(), Some(&self.nets), Some(&self.net_names)).1
    }

    /// Repair clearance violations from `report`.
    #[allow(clippy::too_many_arguments)]
    pub fn repair_from_report(
        &mut self,
        report: &DRCReport,
        max_displacement: f64,
        margin: f64,
        prefer: &str,
        dry_run: bool,
        local_reroute: bool,
        local_grid_padding: f64,
        nudge_footprints: bool,
    ) -> RepairResult {
        let mut result = RepairResult::default();
        let pick = |t: ViolationType| -> Vec<DRCViolation> {
            report
                .by_type(t)
                .into_iter()
                .filter(|v| !is_zone_fill_violation(v))
                .cloned()
                .collect()
        };
        let groups = [
            (pick(ViolationType::CLEARANCE), prefer.to_string()),
            (
                pick(ViolationType::CLEARANCE_SEGMENT_VIA),
                "move-trace".into(),
            ),
            (
                pick(ViolationType::CLEARANCE_PAD_SEGMENT),
                "move-trace".into(),
            ),
            (pick(ViolationType::CLEARANCE_PAD_VIA), "move-via".into()),
        ];
        let all: Vec<DRCViolation> = groups.iter().flat_map(|(v, _)| v.iter().cloned()).collect();
        result.total_violations = all.len() as i64;
        let mut skipped: Vec<usize> = Vec::new();
        let mut idx = 0;
        for (vs, pref) in &groups {
            for v in vs {
                let before = result.skipped_infeasible;
                self.repair_single_violation(
                    v,
                    &mut result,
                    max_displacement,
                    margin,
                    pref,
                    dry_run,
                );
                if result.skipped_infeasible > before {
                    skipped.push(idx);
                }
                idx += 1;
            }
        }
        if local_reroute {
            let both = self.find_both_endpoints_at_vias(&all, max_displacement, margin);
            let mut cands: Vec<(DRCViolation, &'static str)> = skipped
                .iter()
                .map(|&i| (all[i].clone(), "skipped"))
                .collect();
            let skipped_set: HashSet<usize> = skipped.iter().copied().collect();
            for i in both {
                if !skipped_set.contains(&i) {
                    cands.push((all[i].clone(), "phantom_repair"));
                }
            }
            if !cands.is_empty() {
                self.run_local_reroute_phase(
                    &cands,
                    &mut result,
                    margin,
                    dry_run,
                    local_grid_padding,
                );
            }
        }
        if nudge_footprints {
            self.repair_pad_pad_violations(report, &mut result, max_displacement, margin, dry_run);
        }
        result
    }

    fn via_positions(&self) -> Vec<(f64, f64)> {
        self.doc
            .find_all("via")
            .filter_map(|v| xy_of(v.find("at")))
            .collect()
    }

    fn find_both_endpoints_at_vias(
        &self,
        violations: &[DRCViolation],
        max_d: f64,
        margin: f64,
    ) -> Vec<usize> {
        let vias = self.via_positions();
        let at_via = |x: f64, y: f64| {
            vias.iter()
                .any(|(vx, vy)| ((x - vx).powi(2) + (y - vy).powi(2)).sqrt() <= 0.001)
        };
        let mut out = Vec::new();
        for (i, v) in violations.iter().enumerate() {
            if v.locations.len() < 2 {
                continue;
            }
            let Some(delta) = v.delta_mm() else { continue };
            if delta + margin > max_d {
                continue;
            }
            let (l1, l2) = (&v.locations[0], &v.locations[1]);
            let o1 = self.find_object_at(l1.x_mm, l1.y_mm, &l1.layer, &v.nets);
            let o2 = self.find_object_at(l2.x_mm, l2.y_mm, &l2.layer, &v.nets);
            for o in [o1, o2].into_iter().flatten() {
                if o.kind != "segment" {
                    continue;
                }
                let seg = node_at(&self.doc, &o.path);
                let (Some((sx, sy)), Some((ex, ey))) =
                    (xy_of(seg.find("start")), xy_of(seg.find("end")))
                else {
                    continue;
                };
                if at_via(sx, sy) && at_via(ex, ey) {
                    out.push(i);
                    break;
                }
            }
        }
        out
    }

    fn run_local_reroute_phase(
        &mut self,
        tagged: &[(DRCViolation, &'static str)],
        result: &mut RepairResult,
        margin: f64,
        dry_run: bool,
        padding: f64,
    ) {
        let rerouter = LocalRerouter::new(&self.nets, 0.05, padding);
        for cluster in group_violations_by_proximity(tagged, None) {
            if cluster.len() == 1 {
                let (v, src) = &tagged[cluster[0]];
                self.attempt_local_reroute(v, result, &rerouter, margin, dry_run, src, &[], false);
            } else {
                let obstacles: Vec<Option<(f64, f64, f64)>> = cluster
                    .iter()
                    .map(|&i| self.extract_obstacle_info(&tagged[i].0))
                    .collect();
                for (k, &i) in cluster.iter().enumerate() {
                    let extra: Vec<(f64, f64, f64)> = obstacles
                        .iter()
                        .enumerate()
                        .filter(|(j, o)| *j != k && o.is_some())
                        .map(|(_, o)| o.expect("checked"))
                        .collect();
                    let (v, src) = &tagged[i];
                    if !self.attempt_local_reroute(
                        v, result, &rerouter, margin, dry_run, src, &extra, true,
                    ) {
                        self.attempt_local_reroute(
                            v,
                            result,
                            &rerouter,
                            margin,
                            dry_run,
                            src,
                            &[],
                            false,
                        );
                    }
                }
            }
        }
    }

    fn obstacle_radius(&self, obs: Option<&Obj>) -> f64 {
        match obs {
            Some(o) if o.kind == "via" => {
                let n = node_at(&self.doc, &o.path);
                match n.find("size") {
                    Some(s) => first_f(Some(s), 0.0) / 2.0,
                    None => 0.3,
                }
            }
            Some(o) if o.kind == "segment" => {
                let n = node_at(&self.doc, &o.path);
                match n.find("width") {
                    Some(w) => first_f(Some(w), 0.0) / 2.0,
                    None => 0.125,
                }
            }
            Some(o) if o.kind == "pad" => {
                let n = node_at(&self.doc, &o.path);
                match n.find("size") {
                    Some(s) => {
                        let a: Vec<f64> = s.atoms().map(|v| v.as_f64().unwrap_or(0.0)).collect();
                        let w = a.first().copied().unwrap_or(1.0);
                        let h = a.get(1).copied().unwrap_or(w);
                        w.max(h) / 2.0
                    }
                    None => 0.5,
                }
            }
            _ => 0.3,
        }
    }

    fn extract_obstacle_info(&self, v: &DRCViolation) -> Option<(f64, f64, f64)> {
        if v.locations.len() < 2 {
            return None;
        }
        let (l1, l2) = (&v.locations[0], &v.locations[1]);
        let o1 = self.find_object_at(l1.x_mm, l1.y_mm, &l1.layer, &v.nets);
        let o2 = self.find_object_at(l2.x_mm, l2.y_mm, &l2.layer, &v.nets);
        let (x, y, obs) = if o1.as_ref().is_some_and(|o| o.kind == "segment") {
            (l2.x_mm, l2.y_mm, o2)
        } else {
            (l1.x_mm, l1.y_mm, o1)
        };
        Some((x, y, self.obstacle_radius(obs.as_ref())))
    }

    /// One local reroute attempt; returns success. `with_extras` selects the
    /// cluster-variant counter semantics.
    #[allow(clippy::too_many_arguments)]
    fn attempt_local_reroute(
        &mut self,
        v: &DRCViolation,
        result: &mut RepairResult,
        rerouter: &LocalRerouter,
        margin: f64,
        dry_run: bool,
        source: &str,
        extra: &[(f64, f64, f64)],
        with_extras: bool,
    ) -> bool {
        let fail = |result: &mut RepairResult| {
            if !with_extras {
                result.skipped_no_local_route += 1;
                if source == "phantom_repair" {
                    result.repaired -= 1;
                }
            }
            false
        };
        if v.locations.len() < 2 {
            if !with_extras {
                result.skipped_no_local_route += 1;
            }
            return false;
        }
        let (l1, l2) = (&v.locations[0], &v.locations[1]);
        let o1 = self.find_object_at(l1.x_mm, l1.y_mm, &l1.layer, &v.nets);
        let o2 = self.find_object_at(l2.x_mm, l2.y_mm, &l2.layer, &v.nets);
        let (seg, ox, oy, obs) = if o1.as_ref().is_some_and(|o| o.kind == "segment") {
            (o1.clone().expect("checked"), l2.x_mm, l2.y_mm, o2)
        } else if o2.as_ref().is_some_and(|o| o.kind == "segment") {
            (o2.clone().expect("checked"), l1.x_mm, l1.y_mm, o1)
        } else {
            if !with_extras {
                result.skipped_no_local_route += 1;
            }
            return false;
        };
        let radius = self.obstacle_radius(obs.as_ref());
        let seg_node = node_at(&self.doc, &seg.path);
        let trace_width = first_f(seg_node.find("width"), 0.25);
        let trace_clearance = v.required_value_mm.filter(|r| *r != 0.0).unwrap_or(0.2) + margin;
        let same_net: Vec<NodePath> = match &obs {
            Some(o) if o.kind == "segment" => vec![o.path.clone()],
            _ => Vec::new(),
        };
        let rr = rerouter.reroute_segment(
            &mut self.doc,
            &seg.path,
            ox,
            oy,
            radius,
            trace_width,
            trace_clearance,
            dry_run,
            extra,
            &same_net,
        );
        if rr.success {
            result.local_rerouted += 1;
            if with_extras {
                result.cluster_rerouted += 1;
            }
            if source == "skipped" {
                result.repaired += 1;
                result.skipped_infeasible -= 1;
            }
            if !dry_run {
                self.modified = true;
            }
            return true;
        }
        fail(result)
    }

    fn would_oscillate(&self, uuid: &str, dx: f64, dy: f64) -> bool {
        match self.nudge_history.get(uuid) {
            Some((px, py)) if !uuid.is_empty() => px * dx + py * dy < 0.0,
            _ => false,
        }
    }

    fn record_nudge(&mut self, uuid: &str, dx: f64, dy: f64) {
        if uuid.is_empty() {
            return;
        }
        let e = self
            .nudge_history
            .entry(uuid.to_string())
            .or_insert((0.0, 0.0));
        e.0 += dx;
        e.1 += dy;
    }

    fn uuid_of(&self, path: &NodePath) -> String {
        first_text(node_at(&self.doc, path).find("uuid")).unwrap_or_default()
    }

    fn node_width(node: &SExp) -> f64 {
        if let Some(w) = node.find("width") {
            return first_f(Some(w), 0.0);
        }
        if let Some(s) = node.find("size") {
            return first_f(Some(s), 0.0);
        }
        0.25
    }

    fn repair_single_violation(
        &mut self,
        v: &DRCViolation,
        result: &mut RepairResult,
        max_d: f64,
        margin: f64,
        prefer: &str,
        dry_run: bool,
    ) {
        if v.locations.len() < 2 {
            let Some(loc) = v.primary_location().cloned() else {
                result.skipped_no_location += 1;
                return;
            };
            let Some(delta) = v.delta_mm() else {
                result.skipped_no_delta += 1;
                return;
            };
            self.repair_from_single_location(
                loc.x_mm, loc.y_mm, &loc.layer, delta, margin, v, result, max_d, prefer, dry_run,
            );
            return;
        }
        let (l1, l2) = (v.locations[0].clone(), v.locations[1].clone());
        let Some(delta) = v.delta_mm() else {
            result.skipped_no_delta += 1;
            return;
        };
        let mut required = delta + margin;
        if required > max_d {
            result.skipped_exceeds_max += 1;
            return;
        }
        let o1 = self.find_object_at(l1.x_mm, l1.y_mm, &l1.layer, &v.nets);
        let o2 = self.find_object_at(l2.x_mm, l2.y_mm, &l2.layer, &v.nets);
        if o1.is_none() && o2.is_none() {
            result.skipped_infeasible += 1;
            return;
        }
        let Some(target) = choose_target(o1.as_ref(), o2.as_ref(), prefer).cloned() else {
            result.skipped_infeasible += 1;
            return;
        };
        let (other, mut other_x, mut other_y) =
            if o1.as_ref().is_some_and(|o| o.path == target.path) {
                (o2.clone(), l2.x_mm, l2.y_mm)
            } else {
                (o1.clone(), l1.x_mm, l1.y_mm)
            };
        if let Some(other) = &other {
            other_x = other.x;
            other_y = other.y;
            if target.kind == "segment" && other.kind == "segment" {
                let tw = Self::node_width(node_at(&self.doc, &target.path));
                let ow = Self::node_width(node_at(&self.doc, &other.path));
                if (tw - ow).abs() > 1e-6 {
                    let req = v.required_value_mm.unwrap_or(0.0);
                    let need = tw / 2.0 + ow / 2.0 + req + margin;
                    let cd = ((target.x - other.x).powi(2) + (target.y - other.y).powi(2)).sqrt();
                    required = required.max((need - cd).max(0.0));
                    if required > max_d {
                        result.skipped_exceeds_max += 1;
                        return;
                    }
                }
            }
        }
        let (dx, dy, dist) = compute_nudge(target.x, target.y, other_x, other_y, required);
        let uuid = self.uuid_of(&target.path);
        if self.would_oscillate(&uuid, dx, dy) {
            result.skipped_infeasible += 1;
            return;
        }
        let actual = v.actual_value_mm.unwrap_or(0.0);
        result.nudges.push(NudgeResult {
            object_type: target.kind.to_string(),
            x: target.x,
            y: target.y,
            net_name: target.net.clone(),
            layer: target.layer.clone(),
            displacement_x: dx,
            displacement_y: dy,
            displacement_mm: dist,
            old_clearance_mm: actual,
            new_clearance_mm: actual + dist,
            uuid: uuid.clone(),
        });
        if !dry_run {
            self.apply_nudge(&target.path, target.kind, dx, dy, Some(result));
            self.modified = true;
        }
        self.record_nudge(&uuid, dx, dy);
        result.repaired += 1;
    }

    #[allow(clippy::too_many_arguments)]
    fn repair_from_single_location(
        &mut self,
        x: f64,
        y: f64,
        layer: &str,
        delta: f64,
        margin: f64,
        v: &DRCViolation,
        result: &mut RepairResult,
        max_d: f64,
        prefer: &str,
        dry_run: bool,
    ) {
        let required = delta + margin;
        if required > max_d {
            result.skipped_exceeds_max += 1;
            return;
        }
        let layer_opt = if layer.is_empty() { None } else { Some(layer) };
        let mut objs = self.find_segments_near(x, y, 1.0, layer_opt, &v.nets);
        objs.extend(self.find_vias_near(x, y, 1.0, &v.nets));
        if objs.len() < 2 {
            result.skipped_infeasible += 1;
            return;
        }
        let Some(target) = choose_target(Some(&objs[0]), Some(&objs[1]), prefer).cloned() else {
            result.skipped_infeasible += 1;
            return;
        };
        let other = if objs[0].path == target.path {
            &objs[1]
        } else {
            &objs[0]
        };
        let (dx, dy, dist) = compute_nudge(target.x, target.y, other.x, other.y, required);
        let uuid = self.uuid_of(&target.path);
        let actual = v.actual_value_mm.unwrap_or(0.0);
        result.nudges.push(NudgeResult {
            object_type: target.kind.to_string(),
            x: target.x,
            y: target.y,
            net_name: target.net.clone(),
            layer: target.layer.clone(),
            displacement_x: dx,
            displacement_y: dy,
            displacement_mm: dist,
            old_clearance_mm: actual,
            new_clearance_mm: actual + dist,
            uuid,
        });
        if !dry_run {
            self.apply_nudge(&target.path, target.kind, dx, dy, Some(result));
            self.modified = true;
        }
        result.repaired += 1;
    }

    /// Nearest segment, then via, then pad within 1.5 mm.
    pub fn find_object_at(&self, x: f64, y: f64, layer: &str, nets: &[String]) -> Option<Obj> {
        let layer_opt = if layer.is_empty() { None } else { Some(layer) };
        if let Some(o) = self
            .find_segments_near(x, y, 1.5, layer_opt, nets)
            .into_iter()
            .next()
        {
            return Some(o);
        }
        if let Some(o) = self.find_vias_near(x, y, 1.5, nets).into_iter().next() {
            return Some(o);
        }
        self.find_pads_near(x, y, 1.5, layer_opt, nets)
            .into_iter()
            .next()
    }

    pub fn find_segments_near(
        &self,
        x: f64,
        y: f64,
        radius: f64,
        layer: Option<&str>,
        nets: &[String],
    ) -> Vec<Obj> {
        let mut out = Vec::new();
        for path in descendant_paths(&self.doc, &|n| n.has_tag("segment")) {
            let seg = node_at(&self.doc, &path);
            let (Some((sx, sy)), Some((ex, ey))) =
                (xy_of(seg.find("start")), xy_of(seg.find("end")))
            else {
                continue;
            };
            let (cx, cy, d) = closest_point_on_segment(sx, sy, ex, ey, x, y);
            if d > radius {
                continue;
            }
            let seg_layer = first_text(seg.find("layer")).unwrap_or_default();
            if layer.is_some_and(|l| l != seg_layer) {
                continue;
            }
            let net = self.net_name_of(seg);
            if !nets.is_empty() && !nets.contains(&net) {
                continue;
            }
            out.push(Obj {
                path,
                kind: "segment",
                x: cx,
                y: cy,
                layer: seg_layer,
                net,
            });
        }
        out
    }

    pub fn find_vias_near(&self, x: f64, y: f64, radius: f64, nets: &[String]) -> Vec<Obj> {
        let mut out = Vec::new();
        for path in descendant_paths(&self.doc, &|n| n.has_tag("via")) {
            let via = node_at(&self.doc, &path);
            let Some((vx, vy)) = xy_of(via.find("at")) else {
                continue;
            };
            if ((vx - x).powi(2) + (vy - y).powi(2)).sqrt() > radius {
                continue;
            }
            let net = self.net_name_of(via);
            if !nets.is_empty() && !nets.contains(&net) && !net.is_empty() && net != "<no net>" {
                continue;
            }
            let layer = via
                .find("layers")
                .map(|l| {
                    l.atoms()
                        .map(|a| a.to_string())
                        .collect::<Vec<_>>()
                        .join(" - ")
                })
                .unwrap_or_default();
            out.push(Obj {
                path,
                kind: "via",
                x: vx,
                y: vy,
                layer,
                net,
            });
        }
        out
    }

    pub fn find_pads_near(
        &self,
        x: f64,
        y: f64,
        radius: f64,
        layer: Option<&str>,
        nets: &[String],
    ) -> Vec<Obj> {
        let mut out = Vec::new();
        for fp_path in footprint_paths(&self.doc) {
            let fp = node_at(&self.doc, &fp_path);
            let Some(at) = fp.find("at") else { continue };
            let a: Vec<f64> = at.atoms().map(|v| v.as_f64().unwrap_or(0.0)).collect();
            let (fx, fy) = (
                a.first().copied().unwrap_or(0.0),
                a.get(1).copied().unwrap_or(0.0),
            );
            let rot = a.get(2).copied().unwrap_or(0.0);
            let ang = (-rot).to_radians();
            let (c, s) = (ang.cos(), ang.sin());
            for p in descendant_paths(fp, &|n| n.has_tag("pad")) {
                let pad = node_at(fp, &p);
                let Some((lx, ly)) = xy_of(pad.find("at")) else {
                    continue;
                };
                let ax = fx + lx * c - ly * s;
                let ay = fy + lx * s + ly * c;
                if ((ax - x).powi(2) + (ay - y).powi(2)).sqrt() > radius {
                    continue;
                }
                let layers: Vec<String> = pad
                    .find("layers")
                    .map(|l| l.atoms().map(|a| a.to_string()).collect())
                    .unwrap_or_default();
                if let Some(l) = layer {
                    if !layers.iter().any(|x| x == l) && !layers.iter().any(|x| x == "*.Cu") {
                        continue;
                    }
                }
                let num = pad
                    .find("net")
                    .and_then(|n| n.first_atom())
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let net = self.nets.get(&num).cloned().unwrap_or_default();
                if !nets.is_empty() && !nets.contains(&net) {
                    continue;
                }
                let mut full = fp_path.clone();
                full.extend(p);
                out.push(Obj {
                    path: full,
                    kind: "pad",
                    x: ax,
                    y: ay,
                    layer: layers.first().cloned().unwrap_or_default(),
                    net,
                });
            }
        }
        out
    }

    fn connected_segments(&self, x: f64, y: f64, tol: f64) -> Vec<(NodePath, &'static str)> {
        let mut out = Vec::new();
        for path in descendant_paths(&self.doc, &|n| n.has_tag("segment")) {
            let seg = node_at(&self.doc, &path);
            let (Some((sx, sy)), Some((ex, ey))) =
                (xy_of(seg.find("start")), xy_of(seg.find("end")))
            else {
                continue;
            };
            if ((sx - x).powi(2) + (sy - y).powi(2)).sqrt() <= tol {
                out.push((path.clone(), "start"));
            }
            if ((ex - x).powi(2) + (ey - y).powi(2)).sqrt() <= tol {
                out.push((path, "end"));
            }
        }
        out
    }

    fn move_via(&mut self, path: &NodePath, dx: f64, dy: f64) -> bool {
        let Some((ox, oy)) = xy_of(node_at(&self.doc, path).find("at")) else {
            return false;
        };
        let (nx, ny) = (py_round(ox + dx, 4), py_round(oy + dy, 4));
        let connected = self.connected_segments(ox, oy, 0.001);
        if let Some(at) = child_mut(node_at_mut(&mut self.doc, path), "at") {
            set_xy(at, nx, ny);
        }
        for (p, ep) in connected {
            if let Some(n) = child_mut(node_at_mut(&mut self.doc, &p), ep) {
                set_xy(n, nx, ny);
            }
        }
        true
    }

    fn move_segment(&mut self, path: &NodePath, dx: f64, dy: f64) -> Option<bool> {
        let seg = node_at(&self.doc, path);
        let (Some((sx, sy)), Some((ex, ey))) = (xy_of(seg.find("start")), xy_of(seg.find("end")))
        else {
            return None;
        };
        let vias = self.via_positions();
        let at_via = |x: f64, y: f64| {
            vias.iter()
                .any(|(vx, vy)| ((x - vx).powi(2) + (y - vy).powi(2)).sqrt() <= 0.001)
        };
        let (sa, ea) = (at_via(sx, sy), at_via(ex, ey));
        if sa && ea {
            return Some(false);
        }
        let node = node_at_mut(&mut self.doc, path);
        if !sa {
            if let Some(n) = child_mut(node, "start") {
                set_xy(n, py_round(sx + dx, 4), py_round(sy + dy, 4));
            }
        }
        if !ea {
            if let Some(n) = child_mut(node, "end") {
                set_xy(n, py_round(ex + dx, 4), py_round(ey + dy, 4));
            }
        }
        Some(sa || ea)
    }

    fn apply_nudge(
        &mut self,
        path: &NodePath,
        kind: &str,
        dx: f64,
        dy: f64,
        result: Option<&mut RepairResult>,
    ) {
        match kind {
            "via" => {
                if self.move_via(path, dx, dy) {
                    if let Some(r) = result {
                        r.relocated_vias += 1;
                    }
                }
            }
            "segment" => {
                if let Some(true) = self.move_segment(path, dx, dy) {
                    if let Some(r) = result {
                        r.endpoint_nudges += 1;
                    }
                }
            }
            _ => {}
        }
    }

    fn find_node_by_uuid(&self, uuid: &str, kind: &str) -> Option<NodePath> {
        if uuid.is_empty() {
            return None;
        }
        let want = uuid.trim_matches('"');
        descendant_paths(&self.doc, &|n| n.has_tag(kind))
            .into_iter()
            .find(|p| {
                first_text(node_at(&self.doc, p).find("uuid"))
                    .is_some_and(|u| u.trim_matches('"') == want)
            })
    }

    /// Reverse a previously applied nudge (located by UUID).
    pub fn undo_nudge(&mut self, nudge: &NudgeResult) -> bool {
        let Some(path) = self.find_node_by_uuid(&nudge.uuid, &nudge.object_type) else {
            return false;
        };
        let (dx, dy) = (-nudge.displacement_x, -nudge.displacement_y);
        match nudge.object_type.as_str() {
            "via" => self.move_via(&path, dx, dy),
            "segment" => self.move_segment(&path, dx, dy).is_some(),
            _ => false,
        }
    }

    fn repair_pad_pad_violations(
        &mut self,
        report: &DRCReport,
        result: &mut RepairResult,
        max_d: f64,
        margin: f64,
        dry_run: bool,
    ) {
        let vs: Vec<DRCViolation> = report
            .by_type(ViolationType::CLEARANCE_PAD_PAD)
            .into_iter()
            .filter(|v| !is_zone_fill_violation(v))
            .cloned()
            .collect();
        for v in vs {
            if v.is_same_component_pad_clearance() {
                result.footprint_skipped_same_component += 1;
                continue;
            }
            let Some(delta) = v.delta_mm() else {
                result.skipped_no_delta += 1;
                result.total_violations += 1;
                continue;
            };
            let required = delta + margin;
            result.total_violations += 1;
            if required > max_d {
                result.footprint_skipped_exceeds_max += 1;
                continue;
            }
            if v.locations.len() < 2 {
                result.skipped_no_location += 1;
                continue;
            }
            let refs: Vec<String> = extract_component_refs(&v.items).into_iter().collect();
            if refs.len() < 2 {
                result.skipped_infeasible += 1;
                continue;
            }
            let (Some(f1), Some(f2)) = (
                self.find_footprint_by_ref(&refs[0]),
                self.find_footprint_by_ref(&refs[1]),
            ) else {
                result.skipped_infeasible += 1;
                continue;
            };
            if f1.locked && f2.locked {
                result.footprint_skipped_locked += 1;
                continue;
            }
            let Some((target, other)) = select_nudge_target(f1, f2, result) else {
                continue;
            };
            let (l1, l2) = (&v.locations[0], &v.locations[1]);
            let first_refs = v
                .items
                .first()
                .map(|i| extract_component_refs(std::slice::from_ref(i)));
            let (px, py, ox, oy) =
                if first_refs.is_some_and(|r| r.contains(&target.reference.to_uppercase())) {
                    (l1.x_mm, l1.y_mm, l2.x_mm, l2.y_mm)
                } else {
                    (l2.x_mm, l2.y_mm, l1.x_mm, l1.y_mm)
                };
            let (dx, dy, dist) = compute_nudge(px, py, ox, oy, required);
            let actual = v.actual_value_mm.unwrap_or(0.0);
            result.footprint_nudge_results.push(FootprintNudgeResult {
                reference: target.reference.clone(),
                x: target.x,
                y: target.y,
                displacement_x: dx,
                displacement_y: dy,
                displacement_mm: dist,
                old_clearance_mm: actual,
                new_clearance_mm: actual + dist,
                other_reference: other.reference.clone(),
            });
            if !dry_run {
                self.nudge_footprint(&target.path, dx, dy);
                self.modified = true;
            }
            result.footprint_nudges += 1;
            result.repaired += 1;
        }
    }

    fn find_footprint_by_ref(&self, reference: &str) -> Option<FpInfo> {
        let want = reference.to_uppercase();
        for path in footprint_paths(&self.doc) {
            let fp = node_at(&self.doc, &path);
            let mut fp_ref = String::new();
            for prop in fp.find_all("property") {
                if prop.text_at(0).as_deref() == Some("Reference") {
                    fp_ref = prop.text_at(1).unwrap_or_default();
                    break;
                }
            }
            if fp_ref.is_empty() {
                for t in fp.find_all("fp_text") {
                    if t.text_at(0).as_deref() == Some("reference") {
                        fp_ref = t.text_at(1).unwrap_or_default();
                        break;
                    }
                }
            }
            if fp_ref.to_uppercase() != want {
                continue;
            }
            let Some((x, y)) = xy_of(fp.find_child("at")) else {
                continue;
            };
            let mut locked = fp.find_children("locked").iter().any(|l| {
                matches!(
                    l.text_at(0).unwrap_or_else(|| "yes".into()).as_str(),
                    "yes" | "true"
                )
            });
            if !locked {
                if let Some(attr) = fp.find_child("attr") {
                    locked = (0..attr.children.len())
                        .any(|i| attr.text_at(i).as_deref() == Some("locked"));
                }
            }
            let pad_count = fp.find_all("pad").count();
            let is_connector = fp_ref.to_uppercase().starts_with('J');
            return Some(FpInfo {
                path,
                reference: fp_ref,
                x,
                y,
                locked,
                pad_count,
                is_connector,
            });
        }
        None
    }

    fn nudge_footprint(&mut self, path: &NodePath, dx: f64, dy: f64) {
        let fp = node_at(&self.doc, path);
        let Some(at) = fp.find("at") else { return };
        let a: Vec<f64> = at.atoms().map(|v| v.as_f64().unwrap_or(0.0)).collect();
        let (ox, oy) = (
            a.first().copied().unwrap_or(0.0),
            a.get(1).copied().unwrap_or(0.0),
        );
        let rot = a.get(2).copied().unwrap_or(0.0);
        let ang = (-rot).to_radians();
        let (c, s) = (ang.cos(), ang.sin());
        let pads: Vec<(f64, f64)> = fp
            .find_all("pad")
            .filter_map(|p| xy_of(p.find("at")))
            .map(|(lx, ly)| (ox + lx * c - ly * s, oy + lx * s + ly * c))
            .collect();
        if let Some(at) = child_mut(node_at_mut(&mut self.doc, path), "at") {
            set_xy(at, py_round(ox + dx, 4), py_round(oy + dy, 4));
        }
        for (px, py) in pads {
            let (nx, ny) = (py_round(px + dx, 4), py_round(py + dy, 4));
            for (p, ep) in self.connected_segments(px, py, 0.001) {
                if let Some(n) = child_mut(node_at_mut(&mut self.doc, &p), ep) {
                    set_xy(n, nx, ny);
                }
            }
        }
    }

    /// Write the tree (to `output_path` or the source path).
    pub fn save(&self, output_path: Option<&Path>) -> Result<()> {
        crate::core::sexp_file::save_pcb(&self.doc, output_path.unwrap_or(&self.path))
    }
}

fn choose_target<'a>(o1: Option<&'a Obj>, o2: Option<&'a Obj>, prefer: &str) -> Option<&'a Obj> {
    let (a, b) = match (o1, o2) {
        (None, None) => return None,
        (None, Some(b)) => return Some(b),
        (Some(a), None) => return Some(a),
        (Some(a), Some(b)) => (a, b),
    };
    if a.kind == "pad" && b.kind == "pad" {
        return None;
    }
    if a.kind == "pad" {
        return Some(b);
    }
    if b.kind == "pad" {
        return Some(a);
    }
    let want = match prefer {
        "move-trace" => "segment",
        "move-via" => "via",
        _ => return Some(a),
    };
    if a.kind == want {
        Some(a)
    } else if b.kind == want {
        Some(b)
    } else {
        Some(a)
    }
}

fn select_nudge_target(
    f1: FpInfo,
    f2: FpInfo,
    result: &mut RepairResult,
) -> Option<(FpInfo, FpInfo)> {
    let m1 = !f1.locked && !f1.is_connector;
    let m2 = !f2.locked && !f2.is_connector;
    if !m1 && !m2 {
        if f1.locked || f2.locked {
            result.footprint_skipped_locked += 1;
        } else if f1.is_connector || f2.is_connector {
            result.footprint_skipped_connector += 1;
        }
        return None;
    }
    if m1 && !m2 {
        return Some((f1, f2));
    }
    if m2 && !m1 {
        return Some((f2, f1));
    }
    if f1.pad_count <= f2.pad_count {
        Some((f1, f2))
    } else {
        Some((f2, f1))
    }
}

/// Greedy proximity clustering of tagged violations (indices into `tagged`).
pub fn group_violations_by_proximity(
    tagged: &[(DRCViolation, &'static str)],
    radius: Option<f64>,
) -> Vec<Vec<usize>> {
    if tagged.is_empty() {
        return Vec::new();
    }
    let radius = radius.unwrap_or_else(|| {
        2.0 * tagged
            .iter()
            .map(|(v, _)| v.required_value_mm.filter(|r| *r != 0.0).unwrap_or(0.2))
            .fold(f64::NEG_INFINITY, f64::max)
    });
    let pos: Vec<Option<(f64, f64)>> = tagged
        .iter()
        .map(|(v, _)| v.primary_location().map(|l| (l.x_mm, l.y_mm)))
        .collect();
    let mut assigned: HashSet<usize> = HashSet::new();
    let mut clusters = Vec::new();
    for i in 0..tagged.len() {
        if assigned.contains(&i) {
            continue;
        }
        let mut cluster = vec![i];
        assigned.insert(i);
        let Some(pi) = pos[i] else {
            clusters.push(cluster);
            continue;
        };
        let mut cps = vec![pi];
        let mut changed = true;
        while changed {
            changed = false;
            for (j, position) in pos.iter().enumerate() {
                if assigned.contains(&j) {
                    continue;
                }
                let Some(pj) = *position else { continue };
                if cps
                    .iter()
                    .any(|cp| ((pj.0 - cp.0).powi(2) + (pj.1 - cp.1).powi(2)).sqrt() <= radius)
                {
                    cluster.push(j);
                    assigned.insert(j);
                    cps.push(pj);
                    changed = true;
                }
            }
        }
        clusters.push(cluster);
    }
    clusters
}
