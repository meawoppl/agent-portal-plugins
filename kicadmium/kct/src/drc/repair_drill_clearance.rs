//! Port of `kicad_tools.drc.repair_drill_clearance`: fix via-to-via drill
//! clearance by de-duplicating coincident same-net vias or sliding one via
//! along its connecting trace, optionally gated by the shared clearance
//! engine (`relocate_in_pad_vias::check_clearance`, issue #4408).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::local_rerouter::{first_text, xy_of};
use super::net_compat::resolve_net_atom;
use super::repair_silkscreen::{descendant_paths, find_mut, node_at, node_at_mut, NodePath};
use super::violation::{DRCViolation, ViolationType};
use crate::manufacturers::DesignRules;
use crate::pyjson::py_round;
use crate::sexp::SExp;

/// One drill-clearance repair action.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DrillRepairAction {
    /// `deduplicate` or `slide`.
    pub action: String,
    pub via_x: f64,
    pub via_y: f64,
    pub net_name: String,
    pub detail: String,
    pub displacement_mm: f64,
    pub displacement_x: f64,
    pub displacement_y: f64,
    pub uuid: String,
}

/// Summary of a drill clearance repair.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrillRepairResult {
    pub total_violations: i64,
    pub repaired: i64,
    pub deduplicated: i64,
    pub slid: i64,
    pub skipped_no_location: i64,
    pub skipped_no_delta: i64,
    pub skipped_exceeds_max: i64,
    pub skipped_infeasible: i64,
    pub skipped_unsafe: i64,
    pub actions: Vec<DrillRepairAction>,
}

impl DrillRepairResult {
    pub fn success_rate(&self) -> f64 {
        if self.total_violations == 0 {
            return 1.0;
        }
        self.repaired as f64 / self.total_violations as f64
    }

    pub fn summary(&self) -> String {
        let mut lines = vec![format!(
            "Drill Clearance Repair: {}/{} violations fixed",
            self.repaired, self.total_violations
        )];
        let mut add = |n: i64, label: &str| {
            if n > 0 {
                lines.push(format!("  {label}: {n}"));
            }
        };
        add(self.deduplicated, "De-duplicated same-net vias");
        add(self.slid, "Slid vias apart");
        add(
            self.skipped_exceeds_max,
            "Skipped (exceeds max displacement)",
        );
        add(self.skipped_infeasible, "Skipped (infeasible)");
        add(
            self.skipped_unsafe,
            "Skipped (slide would introduce a new violation)",
        );
        add(self.skipped_no_location, "Skipped (no location)");
        add(self.skipped_no_delta, "Skipped (no delta info)");
        lines.join("\n")
    }
}

struct ViaHit {
    path: NodePath,
    net: String,
    x: f64,
    y: f64,
}

/// Drill clearance repairer over a parsed board.
pub struct DrillClearanceRepairer {
    pub path: PathBuf,
    pub doc: SExp,
    pub modified: bool,
    pub nets: HashMap<i64, String>,
    pub net_names: HashMap<String, i64>,
}

pub const COINCIDENT_THRESHOLD: f64 = 0.01;

impl DrillClearanceRepairer {
    pub fn new(pcb_path: &Path) -> Result<Self> {
        let doc = crate::sexp::parse_file(pcb_path)?;
        let mut r = DrillClearanceRepairer {
            path: pcb_path.to_path_buf(),
            doc,
            modified: false,
            nets: HashMap::new(),
            net_names: HashMap::new(),
        };
        for child in &r.doc.children {
            if !child.has_tag("net") {
                continue;
            }
            let atoms: Vec<String> = (0..child.children.len())
                .filter_map(|i| child.text_at(i))
                .collect();
            if atoms.len() < 2 {
                continue;
            }
            if let Ok(n) = atoms[0].trim().parse::<i64>() {
                r.nets.insert(n, atoms[1].clone());
                r.net_names.insert(atoms[1].clone(), n);
            }
        }
        Ok(r)
    }

    fn resolve(&self, node: &SExp) -> (i64, String) {
        let atom = first_text(node.find("net"));
        resolve_net_atom(atom.as_deref(), Some(&self.nets), Some(&self.net_names))
    }

    /// Repair the drill-clearance members of `violations`.
    pub fn repair(
        &mut self,
        violations: &[DRCViolation],
        max_displacement: f64,
        margin: f64,
        dry_run: bool,
        design_rules: Option<&DesignRules>,
    ) -> DrillRepairResult {
        let mut result = DrillRepairResult::default();
        let drill: Vec<&DRCViolation> = violations
            .iter()
            .filter(|v| {
                matches!(
                    v.vtype,
                    ViolationType::DRILL_CLEARANCE | ViolationType::HOLE_NEAR_HOLE
                )
            })
            .collect();
        result.total_violations = drill.len() as i64;
        for v in drill {
            self.repair_single(
                v,
                &mut result,
                max_displacement,
                margin,
                dry_run,
                design_rules,
            );
        }
        result
    }

    fn repair_single(
        &mut self,
        v: &DRCViolation,
        result: &mut DrillRepairResult,
        max_d: f64,
        margin: f64,
        dry_run: bool,
        rules: Option<&DesignRules>,
    ) {
        if v.locations.is_empty() {
            result.skipped_no_location += 1;
            return;
        }
        let Some(delta) = v.delta_mm() else {
            result.skipped_no_delta += 1;
            return;
        };
        let loc = v.locations[0].clone();
        let mut vias = self.find_vias_near(loc.x_mm, loc.y_mm, 2.0);
        if vias.len() < 2 && v.locations.len() >= 2 {
            let l2 = &v.locations[1];
            for h in self.find_vias_near(l2.x_mm, l2.y_mm, 2.0) {
                if !vias.iter().any(|x| x.path == h.path) {
                    vias.push(h);
                }
            }
        }
        if vias.len() < 2 {
            result.skipped_infeasible += 1;
            return;
        }
        let d = |h: &ViaHit| ((h.x - loc.x_mm).powi(2) + (h.y - loc.y_mm).powi(2)).sqrt();
        vias.sort_by(|a, b| d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal));
        let (v1, v2) = (&vias[0], &vias[1]);
        let dist = ((v2.x - v1.x).powi(2) + (v2.y - v1.y).powi(2)).sqrt();
        if v1.net == v2.net && dist < COINCIDENT_THRESHOLD {
            let (p, x, y, n) = (v2.path.clone(), v2.x, v2.y, v2.net.clone());
            self.deduplicate(&p, x, y, &n, result, dry_run);
            return;
        }
        let required = delta + margin;
        if required > max_d {
            result.skipped_exceeds_max += 1;
            return;
        }
        let (p, x, y, n) = (v2.path.clone(), v2.x, v2.y, v2.net.clone());
        let (ox, oy) = (v1.x, v1.y);
        self.slide_via(&p, x, y, &n, ox, oy, required, result, dry_run, rules);
    }

    fn find_vias_near(&self, x: f64, y: f64, radius: f64) -> Vec<ViaHit> {
        let mut out = Vec::new();
        for path in descendant_paths(&self.doc, &|n| n.has_tag("via")) {
            let via = node_at(&self.doc, &path);
            let Some((vx, vy)) = xy_of(via.find("at")) else {
                continue;
            };
            if ((vx - x).powi(2) + (vy - y).powi(2)).sqrt() > radius {
                continue;
            }
            let net = self.resolve(via).1;
            out.push(ViaHit {
                path,
                net,
                x: vx,
                y: vy,
            });
        }
        out
    }

    fn uuid_at(&self, path: &NodePath) -> String {
        first_text(node_at(&self.doc, path).find("uuid")).unwrap_or_default()
    }

    fn deduplicate(
        &mut self,
        path: &NodePath,
        x: f64,
        y: f64,
        net: &str,
        result: &mut DrillRepairResult,
        dry_run: bool,
    ) {
        result.actions.push(DrillRepairAction {
            action: "deduplicate".into(),
            via_x: x,
            via_y: y,
            net_name: net.to_string(),
            detail: "removed duplicate same-net via".into(),
            uuid: self.uuid_at(path),
            ..Default::default()
        });
        if !dry_run && path.len() == 1 && path[0] < self.doc.children.len() {
            self.doc.children.remove(path[0]);
            self.modified = true;
        }
        result.repaired += 1;
        result.deduplicated += 1;
    }

    #[allow(clippy::too_many_arguments)]
    fn slide_via(
        &mut self,
        path: &NodePath,
        vx: f64,
        vy: f64,
        net: &str,
        ox: f64,
        oy: f64,
        required: f64,
        result: &mut DrillRepairResult,
        dry_run: bool,
        rules: Option<&DesignRules>,
    ) {
        let (dx, dy, dist) = match self.find_connected_segment(vx, vy, net) {
            Some((seg_path, sx, sy, ex, ey)) => {
                let ds = ((sx - vx).powi(2) + (sy - vy).powi(2)).sqrt();
                let de = ((ex - vx).powi(2) + (ey - vy).powi(2)).sqrt();
                let (dirx, diry) = if ds < de {
                    (ex - sx, ey - sy)
                } else {
                    (sx - ex, sy - ey)
                };
                let len = (dirx * dirx + diry * diry).sqrt();
                if len < 1e-10 {
                    push_away(vx, vy, ox, oy, required)
                } else {
                    let (mut nx, mut ny) = (dirx / len, diry / len);
                    let tx = vx + nx * required;
                    let ty = vy + ny * required;
                    let new_d = ((tx - ox).powi(2) + (ty - oy).powi(2)).sqrt();
                    let old_d = ((vx - ox).powi(2) + (vy - oy).powi(2)).sqrt();
                    if new_d <= old_d {
                        nx = -nx;
                        ny = -ny;
                    }
                    let (dx, dy) = (nx * required, ny * required);
                    if !dry_run {
                        self.update_segment_endpoint(&seg_path, vx, vy, vx + dx, vy + dy);
                    }
                    (dx, dy, required)
                }
            }
            None => push_away(vx, vy, ox, oy, required),
        };
        if let Some(rules) = rules {
            if self
                .target_clearance_reason(vx, vy, vx + dx, vy + dy, rules)
                .is_some()
            {
                result.skipped_unsafe += 1;
                return;
            }
        }
        result.actions.push(DrillRepairAction {
            action: "slide".into(),
            via_x: vx,
            via_y: vy,
            net_name: net.to_string(),
            detail: format!("slid {dist:.4}mm to increase clearance"),
            displacement_mm: dist,
            displacement_x: dx,
            displacement_y: dy,
            uuid: self.uuid_at(path),
        });
        if !dry_run {
            if let Some(at) = find_mut(node_at_mut(&mut self.doc, path), "at") {
                at.set_value(0, py_round(vx + dx, 4));
                at.set_value(1, py_round(vy + dy, 4));
                self.modified = true;
            }
        }
        result.repaired += 1;
        result.slid += 1;
    }

    /// Clearance-engine gate on the post-move position (board frame).
    fn target_clearance_reason(
        &self,
        vx: f64,
        vy: f64,
        nx: f64,
        ny: f64,
        rules: &DesignRules,
    ) -> Option<String> {
        use crate::cli::relocate_in_pad_vias as rel;
        let pcb = crate::schema::pcb::Pcb::from_sexp(self.doc.clone()).ok()?;
        let (ox, oy) = pcb.board_origin();
        let (rvx, rvy) = (vx - ox, vy - oy);
        let mut best = 1e-3;
        let mut target = None;
        for (i, v) in pcb.vias().iter().enumerate() {
            let d = (v.position.0 - rvx).hypot(v.position.1 - rvy);
            if d < best {
                best = d;
                target = Some(i);
            }
        }
        let idx = target?;
        let via = pcb.vias()[idx].clone();
        let pads = rel::collect_smd_pads_by_net(&pcb);
        let tht = rel::collect_tht_pads(&pcb);
        match rel::check_clearance(
            &pcb,
            Some(idx),
            &via,
            nx - ox,
            ny - oy,
            &pads,
            &tht,
            rules.min_clearance_mm,
            rules.min_hole_to_hole_mm,
            None,
        ) {
            Ok(r) => r,
            Err(e) => Some(e.to_string()),
        }
    }

    /// Reverse a `slide` action (dedup actions cannot be undone).
    pub fn undo_action(&mut self, action: &DrillRepairAction) -> bool {
        if action.action != "slide" || action.uuid.is_empty() {
            return false;
        }
        let want = action.uuid.trim_matches('"').to_string();
        let Some(path) = descendant_paths(&self.doc, &|n| n.has_tag("via"))
            .into_iter()
            .find(|p| {
                first_text(node_at(&self.doc, p).find("uuid"))
                    .is_some_and(|u| u.trim_matches('"') == want)
            })
        else {
            return false;
        };
        let Some((cx, cy)) = xy_of(node_at(&self.doc, &path).find("at")) else {
            return false;
        };
        let (nx, ny) = (
            py_round(cx - action.displacement_x, 4),
            py_round(cy - action.displacement_y, 4),
        );
        let segs: Vec<NodePath> = descendant_paths(&self.doc, &|n| n.has_tag("segment"))
            .into_iter()
            .filter(|p| {
                let s = node_at(&self.doc, p);
                match (xy_of(s.find("start")), xy_of(s.find("end"))) {
                    (Some((sx, sy)), Some((ex, ey))) => {
                        ((sx - cx).powi(2) + (sy - cy).powi(2)).sqrt() < 0.01
                            || ((ex - cx).powi(2) + (ey - cy).powi(2)).sqrt() < 0.01
                    }
                    _ => false,
                }
            })
            .collect();
        if let Some(at) = find_mut(node_at_mut(&mut self.doc, &path), "at") {
            at.set_value(0, nx);
            at.set_value(1, ny);
        }
        for p in segs {
            self.update_segment_endpoint(&p, cx, cy, nx, ny);
        }
        true
    }

    fn find_connected_segment(
        &self,
        vx: f64,
        vy: f64,
        net: &str,
    ) -> Option<(NodePath, f64, f64, f64, f64)> {
        let net_num = self.net_names.get(net).copied().unwrap_or(-1);
        for path in descendant_paths(&self.doc, &|n| n.has_tag("segment")) {
            let seg = node_at(&self.doc, &path);
            if seg.find("net").is_none() {
                continue;
            }
            if self.resolve(seg).0 != net_num {
                continue;
            }
            let (Some((sx, sy)), Some((ex, ey))) =
                (xy_of(seg.find("start")), xy_of(seg.find("end")))
            else {
                continue;
            };
            let ds = ((sx - vx).powi(2) + (sy - vy).powi(2)).sqrt();
            let de = ((ex - vx).powi(2) + (ey - vy).powi(2)).sqrt();
            if ds < 0.01 || de < 0.01 {
                return Some((path, sx, sy, ex, ey));
            }
        }
        None
    }

    fn update_segment_endpoint(&mut self, path: &NodePath, ox: f64, oy: f64, nx: f64, ny: f64) {
        let seg = node_at(&self.doc, path);
        let start = xy_of(seg.find("start"));
        let end = xy_of(seg.find("end"));
        let node = node_at_mut(&mut self.doc, path);
        if let Some((sx, sy)) = start {
            if ((sx - ox).powi(2) + (sy - oy).powi(2)).sqrt() < 0.01 {
                if let Some(n) = find_mut(node, "start") {
                    n.set_value(0, py_round(nx, 4));
                    n.set_value(1, py_round(ny, 4));
                }
                return;
            }
        }
        if let Some((ex, ey)) = end {
            if ((ex - ox).powi(2) + (ey - oy).powi(2)).sqrt() < 0.01 {
                if let Some(n) = find_mut(node, "end") {
                    n.set_value(0, py_round(nx, 4));
                    n.set_value(1, py_round(ny, 4));
                }
            }
        }
    }

    pub fn save(&self, output_path: Option<&Path>) -> Result<()> {
        crate::core::sexp_file::save_pcb(&self.doc, output_path.unwrap_or(&self.path))
    }
}

fn push_away(x: f64, y: f64, ox: f64, oy: f64, d: f64) -> (f64, f64, f64) {
    let (dx, dy) = (x - ox, y - oy);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-10 {
        return (d, 0.0, d);
    }
    let s = d / len;
    (dx * s, dy * s, d)
}
