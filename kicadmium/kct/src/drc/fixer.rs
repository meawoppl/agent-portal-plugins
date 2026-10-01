//! Port of `kicad_tools.drc.fixer`: destructive DRC repair (delete the
//! offending tracks/vias so the net can be re-routed).
//!
//! ```no_run
//! use kct::drc::{fixer::DRCFixer, DRCReport};
//! let report = DRCReport::load("board-drc.rpt").unwrap();
//! let mut fixer = DRCFixer::new("board.kicad_pcb").unwrap();
//! let fixed = fixer.fix_shorts(&report, false);
//! println!("Fixed {fixed} shorts");
//! fixer.save(Some("board-fixed.kicad_pcb".as_ref())).unwrap();
//! ```

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::net_compat::resolve_net_atom;
use super::report::DRCReport;
use super::violation::ViolationType;
use crate::sexp::{Document, SExp};

/// A track segment found near a point.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceInfo {
    pub start_x: f64,
    pub start_y: f64,
    pub end_x: f64,
    pub end_y: f64,
    pub width: f64,
    pub layer: String,
    pub net: i64,
    pub net_name: String,
    pub uuid: String,
    /// Copy of the s-expression node (identity for deletion).
    pub node: SExp,
}

/// A via found near a point.
#[derive(Debug, Clone, PartialEq)]
pub struct ViaInfo {
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub drill: f64,
    pub net: i64,
    pub uuid: String,
    pub node: SExp,
}

/// Automated DRC violation fixer.
#[derive(Debug, Clone)]
pub struct DRCFixer {
    pub path: PathBuf,
    pub doc: Document,
    pub deleted_count: usize,
    pub modified: bool,
    pub nets: HashMap<i64, String>,
    pub net_names: HashMap<String, i64>,
}

fn xy(node: Option<&SExp>) -> (f64, f64) {
    let Some(n) = node else {
        return (0.0, 0.0);
    };
    (n.float_at(0).unwrap_or(0.0), n.float_at(1).unwrap_or(0.0))
}

fn nonempty(node: Option<&SExp>) -> Option<&SExp> {
    node.filter(|n| !n.children.is_empty())
}

impl DRCFixer {
    /// Load a PCB file for fixing.
    pub fn new(pcb_path: impl AsRef<Path>) -> Result<Self> {
        let path = pcb_path.as_ref().to_path_buf();
        let doc = Document::load(&path)?;
        let mut fixer = DRCFixer {
            path,
            doc,
            deleted_count: 0,
            modified: false,
            nets: HashMap::new(),
            net_names: HashMap::new(),
        };
        fixer.parse_nets();
        Ok(fixer)
    }

    /// Top-level `(net <number> "<name>")` definitions only.
    fn parse_nets(&mut self) {
        for child in &self.doc.root.children {
            if !child.has_tag("net") {
                continue;
            }
            let atoms: Vec<_> = child.atoms().collect();
            if atoms.len() < 2 {
                continue;
            }
            let Some(num) = child.int_at(0) else {
                continue;
            };
            let name = atoms[1].to_string();
            self.nets.insert(num, name.clone());
            self.net_names.insert(name, num);
        }
    }

    fn resolve(&self, node: Option<&SExp>) -> (i64, String) {
        let atom = node.and_then(|n| n.text_at(0));
        resolve_net_atom(atom.as_deref(), Some(&self.nets), Some(&self.net_names))
    }

    /// Track segments within `radius` of a point.
    pub fn find_segments_near(
        &self,
        x: f64,
        y: f64,
        radius: f64,
        layer: Option<&str>,
        net_name: Option<&str>,
    ) -> Vec<TraceInfo> {
        let mut out = Vec::new();
        for seg in self.doc.root.find_all("segment") {
            let (Some(start), Some(end)) = (nonempty(seg.find("start")), nonempty(seg.find("end")))
            else {
                continue;
            };
            let (sx, sy) = xy(Some(start));
            let (ex, ey) = xy(Some(end));
            if !segment_near_point(sx, sy, ex, ey, x, y, radius) {
                continue;
            }
            let seg_layer = seg
                .find("layer")
                .and_then(|n| n.text_at(0))
                .unwrap_or_default();
            if layer.is_some_and(|l| !l.is_empty() && seg_layer != l) {
                continue;
            }
            let (net, name) = self.resolve(seg.find("net"));
            if net_name.is_some_and(|n| !n.is_empty() && name != n) {
                continue;
            }
            out.push(TraceInfo {
                start_x: sx,
                start_y: sy,
                end_x: ex,
                end_y: ey,
                width: seg.find("width").and_then(|n| n.float_at(0)).unwrap_or(0.0),
                layer: seg_layer,
                net,
                net_name: name,
                uuid: seg
                    .find("uuid")
                    .and_then(|n| n.text_at(0))
                    .unwrap_or_default(),
                node: seg.clone(),
            });
        }
        out
    }

    /// Vias within `radius` of a point.
    pub fn find_vias_near(
        &self,
        x: f64,
        y: f64,
        radius: f64,
        net_name: Option<&str>,
    ) -> Vec<ViaInfo> {
        let mut out = Vec::new();
        for via in self.doc.root.find_all("via") {
            let Some(at) = nonempty(via.find("at")) else {
                continue;
            };
            let (vx, vy) = xy(Some(at));
            if ((vx - x).powi(2) + (vy - y).powi(2)).sqrt() > radius {
                continue;
            }
            let (net, name) = self.resolve(via.find("net"));
            if net_name.is_some_and(|n| !n.is_empty() && name != n) {
                continue;
            }
            out.push(ViaInfo {
                x: vx,
                y: vy,
                size: via.find("size").and_then(|n| n.float_at(0)).unwrap_or(0.0),
                drill: via.find("drill").and_then(|n| n.float_at(0)).unwrap_or(0.0),
                net,
                uuid: via
                    .find("uuid")
                    .and_then(|n| n.text_at(0))
                    .unwrap_or_default(),
                node: via.clone(),
            });
        }
        out
    }

    fn delete_node(&mut self, node: &SExp) -> bool {
        if let Some(i) = self.doc.root.children.iter().position(|c| c == node) {
            self.doc.root.children.remove(i);
            self.deleted_count += 1;
            self.modified = true;
            return true;
        }
        false
    }

    /// Delete a track segment from the PCB.
    pub fn delete_segment(&mut self, segment: &TraceInfo) -> bool {
        self.delete_node(&segment.node)
    }

    /// Delete a via from the PCB.
    pub fn delete_via(&mut self, via: &ViaInfo) -> bool {
        self.delete_node(&via.node)
    }

    /// Delete all top-level traces and vias for a net.
    pub fn delete_net_traces(&mut self, net_name: &str) -> usize {
        let net_num = self.net_names.get(net_name).copied().unwrap_or(0);
        if net_num == 0 {
            return 0;
        }
        let before = self.doc.root.children.len();
        let doomed: Vec<bool> = self
            .doc
            .root
            .children
            .iter()
            .map(|c| {
                (c.has_tag("segment") || c.has_tag("via"))
                    && nonempty(c.find("net")).is_some_and(|n| self.resolve(Some(n)).0 == net_num)
            })
            .collect();
        let mut it = doomed.into_iter();
        self.doc
            .root
            .children
            .retain(|_| !it.next().unwrap_or(false));
        let deleted = before - self.doc.root.children.len();
        if deleted > 0 {
            self.modified = true;
            self.deleted_count += deleted;
        }
        deleted
    }

    /// Fix short-circuit violations by deleting offending traces/vias.
    pub fn fix_shorts(&mut self, report: &DRCReport, _delete_both_nets: bool) -> usize {
        let mut fixed = 0;
        for v in report.by_type(ViolationType::SHORTING_ITEMS) {
            let Some(loc) = v.primary_location() else {
                continue;
            };
            let layer = (!loc.layer.is_empty()).then_some(loc.layer.as_str());
            for seg in self.find_segments_near(loc.x_mm, loc.y_mm, 1.0, layer, None) {
                if v.nets.is_empty() || v.nets.contains(&seg.net_name) {
                    self.delete_segment(&seg);
                    fixed += 1;
                }
            }
            for via in self.find_vias_near(loc.x_mm, loc.y_mm, 1.0, None) {
                let name = self.nets.get(&via.net).cloned().unwrap_or_default();
                if v.nets.is_empty() || v.nets.contains(&name) {
                    self.delete_via(&via);
                    fixed += 1;
                }
            }
        }
        fixed
    }

    /// Fix clearance violations by deleting the first nearby segment.
    pub fn fix_clearance_violations(&mut self, report: &DRCReport) -> usize {
        let mut fixed = 0;
        for v in report.by_type(ViolationType::CLEARANCE) {
            let Some(loc) = v.primary_location() else {
                continue;
            };
            let layer = (!loc.layer.is_empty()).then_some(loc.layer.as_str());
            if let Some(seg) = self
                .find_segments_near(loc.x_mm, loc.y_mm, 0.5, layer, None)
                .into_iter()
                .next()
            {
                self.delete_segment(&seg);
                fixed += 1;
            }
        }
        fixed
    }

    /// Nets with unconnected items.
    pub fn get_unconnected_nets(&self, report: &DRCReport) -> BTreeSet<String> {
        report
            .by_type(ViolationType::UNCONNECTED_ITEMS)
            .into_iter()
            .flat_map(|v| v.nets.iter().cloned())
            .collect()
    }

    /// All nets affected by any DRC violation.
    pub fn get_affected_nets(&self, report: &DRCReport) -> BTreeSet<String> {
        report
            .violations
            .iter()
            .flat_map(|v| v.nets.iter().cloned())
            .collect()
    }

    /// Save the modified PCB (to `output_path`, or back to the source).
    pub fn save(&self, output_path: Option<&Path>) -> Result<()> {
        self.doc.save(Some(output_path.unwrap_or(&self.path)))
    }

    pub fn summary(&self) -> String {
        format!("DRC Fixer: deleted {} elements", self.deleted_count)
    }
}

/// True when segment `(x1,y1)-(x2,y2)` passes within `radius` of `(px,py)`.
pub fn segment_near_point(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    px: f64,
    py: f64,
    radius: f64,
) -> bool {
    let (dx, dy) = (x2 - x1, y2 - y1);
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-10 {
        return ((x1 - px).powi(2) + (y1 - py).powi(2)).sqrt() <= radius;
    }
    let t = (((px - x1) * dx + (py - y1) * dy) / len_sq).clamp(0.0, 1.0);
    let (cx, cy) = (x1 + t * dx, y1 + t * dy);
    ((cx - px).powi(2) + (cy - py).powi(2)).sqrt() <= radius
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_point() {
        assert!(segment_near_point(0.0, 0.0, 10.0, 0.0, 5.0, 0.4, 0.5));
        assert!(!segment_near_point(0.0, 0.0, 10.0, 0.0, 5.0, 0.6, 0.5));
        assert!(segment_near_point(1.0, 1.0, 1.0, 1.0, 1.3, 1.0, 0.5));
    }
}
