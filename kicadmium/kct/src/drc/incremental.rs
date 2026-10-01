//! Incremental DRC for interactive placement (port of
//! `kicad_tools.drc.incremental`): cached full check plus O(log n)-ish
//! re-checks of the area a moved component touches. The upstream C++
//! pair-clearance backend is replaced by the native implementation here.

use std::time::Instant;

use crate::geometry::strtree::StrTree;
use crate::manufacturers::DesignRules;
use crate::schema::pcb::{Footprint, Pcb};

/// 0.1 um tolerance on pad-pad clearance comparisons.
pub const CLEARANCE_EPSILON_MM: f64 = 1e-4;

/// Axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rectangle {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Rectangle {
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Rectangle {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    pub fn center_x(&self) -> f64 {
        (self.min_x + self.max_x) / 2.0
    }

    pub fn center_y(&self) -> f64 {
        (self.min_y + self.max_y) / 2.0
    }

    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    pub fn translate(&self, dx: f64, dy: f64) -> Self {
        Rectangle::new(
            self.min_x + dx,
            self.min_y + dy,
            self.max_x + dx,
            self.max_y + dy,
        )
    }

    pub fn union(&self, o: &Rectangle) -> Self {
        Rectangle::new(
            self.min_x.min(o.min_x),
            self.min_y.min(o.min_y),
            self.max_x.max(o.max_x),
            self.max_y.max(o.max_y),
        )
    }

    pub fn expand(&self, m: f64) -> Self {
        Rectangle::new(
            self.min_x - m,
            self.min_y - m,
            self.max_x + m,
            self.max_y + m,
        )
    }

    pub fn intersects(&self, o: &Rectangle) -> bool {
        !(self.max_x < o.min_x
            || self.min_x > o.max_x
            || self.max_y < o.min_y
            || self.min_y > o.max_y)
    }

    pub fn as_tuple(&self) -> (f64, f64, f64, f64) {
        (self.min_x, self.min_y, self.max_x, self.max_y)
    }

    pub fn from_center(cx: f64, cy: f64, w: f64, h: f64) -> Self {
        let (hw, hh) = (w / 2.0, h / 2.0);
        Rectangle::new(cx - hw, cy - hh, cx + hw, cy + hh)
    }
}

/// A DRC violation (identity = rule, location, items).
#[derive(Debug, Clone)]
pub struct Violation {
    pub rule_id: String,
    pub message: String,
    pub severity: String,
    pub location: (f64, f64),
    pub layer: String,
    pub items: Vec<String>,
    pub nets: Vec<String>,
    pub actual_value: Option<f64>,
    pub required_value: Option<f64>,
}

impl PartialEq for Violation {
    fn eq(&self, o: &Self) -> bool {
        self.rule_id == o.rule_id && self.location == o.location && self.items == o.items
    }
}

impl Violation {
    /// Whether any item starts with `reference`.
    pub fn involves(&self, reference: &str) -> bool {
        self.items.iter().any(|i| i.starts_with(reference))
    }
}

/// Spatial index over named rectangles (an STR tree rebuilt lazily after
/// edits; queries return insertion order like the linear fallback).
#[derive(Debug, Clone, Default)]
pub struct SpatialIndex {
    items: Vec<(String, Rectangle)>,
    tree: std::cell::OnceCell<StrTree>,
}

impl SpatialIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, reference: &str, bounds: Rectangle) {
        self.remove(reference);
        self.items.push((reference.to_string(), bounds));
        self.tree = std::cell::OnceCell::new();
    }

    pub fn remove(&mut self, reference: &str) {
        let before = self.items.len();
        self.items.retain(|(r, _)| r != reference);
        if self.items.len() != before {
            self.tree = std::cell::OnceCell::new();
        }
    }

    pub fn query(&self, b: &Rectangle) -> Vec<String> {
        let tree = self.tree.get_or_init(|| {
            StrTree::new(
                &self
                    .items
                    .iter()
                    .map(|(_, r)| Some(r.as_tuple()))
                    .collect::<Vec<_>>(),
            )
        });
        let mut hits = tree.query(b.as_tuple());
        hits.sort_unstable();
        hits.into_iter().map(|i| self.items[i].0.clone()).collect()
    }

    pub fn update(&mut self, reference: &str, bounds: Rectangle) {
        self.insert(reference, bounds);
    }

    pub fn get_bounds(&self, reference: &str) -> Option<Rectangle> {
        self.items
            .iter()
            .find(|(r, _)| r == reference)
            .map(|(_, b)| *b)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, reference: &str) -> bool {
        self.items.iter().any(|(r, _)| r == reference)
    }
}

/// Cached DRC state.
#[derive(Debug, Clone, Default)]
pub struct DRCState {
    pub violations: Vec<Violation>,
    pub spatial_index: SpatialIndex,
    pub component_bounds: Vec<(String, Rectangle)>,
    pub net_segments: Vec<(String, SegmentBounds)>,
}

/// Per-net segment envelopes `(min_x, min_y, max_x, max_y)`.
pub type SegmentBounds = Vec<(f64, f64, f64, f64)>;

impl DRCState {
    fn bounds(&self, r: &str) -> Option<Rectangle> {
        self.component_bounds
            .iter()
            .find(|(k, _)| k == r)
            .map(|(_, b)| *b)
    }
}

/// Change in violations after an operation.
#[derive(Debug, Clone, Default)]
pub struct DRCDelta {
    pub new_violations: Vec<Violation>,
    pub resolved_violations: Vec<Violation>,
    pub affected_components: Vec<String>,
    pub affected_nets: Vec<String>,
    pub check_time_ms: f64,
}

impl DRCDelta {
    pub fn net_change(&self) -> i64 {
        self.new_violations.len() as i64 - self.resolved_violations.len() as i64
    }

    pub fn is_improvement(&self) -> bool {
        self.net_change() < 0
    }

    pub fn summary(&self) -> String {
        let (n, r) = (self.new_violations.len(), self.resolved_violations.len());
        match self.net_change() {
            c if c > 0 => format!("+{c} violations ({n} new, {r} resolved)"),
            c if c < 0 => format!("{c} violations ({n} new, {r} resolved)"),
            _ => format!("No net change ({n} new, {r} resolved)"),
        }
    }
}

/// DRC engine with incremental update capability.
pub struct IncrementalDRC<'a> {
    pub pcb: &'a Pcb,
    pub rules: DesignRules,
    pub state: Option<DRCState>,
    component_nets: Vec<(String, Vec<String>)>,
    max_clearance: f64,
}

fn rotate(fp_rot: f64) -> (f64, f64) {
    let a = (-fp_rot).to_radians();
    (a.cos(), a.sin())
}

impl<'a> IncrementalDRC<'a> {
    pub fn new(pcb: &'a Pcb, rules: DesignRules) -> Self {
        let max_clearance = rules.min_clearance_mm;
        IncrementalDRC {
            pcb,
            rules,
            state: None,
            component_nets: vec![],
            max_clearance,
        }
    }

    /// Upstream never rewrites footprint positions: applied moves live only
    /// in the cached bounds.
    fn position(&self, fp: &Footprint) -> (f64, f64) {
        fp.position
    }

    /// Full DRC; caches state for incremental checks.
    pub fn full_check(&mut self) -> Vec<Violation> {
        let mut st = DRCState::default();
        for fp in self.pcb.footprints() {
            let b = Self::footprint_bounds(fp, self.position(fp));
            st.spatial_index.insert(&fp.reference, b);
            match st
                .component_bounds
                .iter_mut()
                .find(|(r, _)| *r == fp.reference)
            {
                Some(e) => e.1 = b,
                None => st.component_bounds.push((fp.reference.clone(), b)),
            }
        }
        self.component_nets = self
            .pcb
            .footprints()
            .iter()
            .map(|fp| {
                let mut nets: Vec<String> = Vec::new();
                for p in &fp.pads {
                    if !p.net_name.is_empty() && !nets.contains(&p.net_name) {
                        nets.push(p.net_name.clone());
                    }
                }
                (fp.reference.clone(), nets)
            })
            .collect();
        for s in self.pcb.segments() {
            if let Some(net) = self.pcb.get_net(s.net_number) {
                let seg = (s.start.0, s.start.1, s.end.0, s.end.1);
                match st.net_segments.iter_mut().find(|(n, _)| *n == net.name) {
                    Some(e) => e.1.push(seg),
                    None => st.net_segments.push((net.name.clone(), vec![seg])),
                }
            }
        }
        self.state = Some(st);
        let v = self.check_all_clearances();
        self.state.as_mut().unwrap().violations = v.clone();
        v
    }

    /// Preview the DRC impact of moving `reference` to `(new_x, new_y)`.
    pub fn check_move(&mut self, reference: &str, new_x: f64, new_y: f64) -> DRCDelta {
        let t = Instant::now();
        if self.state.is_none() {
            self.full_check();
        }
        let st = self.state.as_ref().unwrap();
        let Some(old) = st.bounds(reference) else {
            return DRCDelta {
                check_time_ms: t.elapsed().as_secs_f64() * 1000.0,
                ..Default::default()
            };
        };
        let new_bounds = old.translate(new_x - old.center_x(), new_y - old.center_y());
        let area = old.union(&new_bounds).expand(self.max_clearance);
        let mut nearby = st.spatial_index.query(&area);
        if !nearby.iter().any(|r| r == reference) {
            nearby.push(reference.to_string());
        }
        let nets = self
            .component_nets
            .iter()
            .find(|(r, _)| r == reference)
            .map(|(_, n)| n.clone())
            .unwrap_or_default();
        let new_v = self.component_clearances(reference, new_bounds, &nearby);
        let st = self.state.as_ref().unwrap();
        let resolved: Vec<Violation> = st
            .violations
            .iter()
            .filter(|v| v.involves(reference) && !new_v.contains(v))
            .cloned()
            .collect();
        let mut affected = vec![reference.to_string()];
        affected.extend(nearby.into_iter().filter(|r| r != reference));
        DRCDelta {
            new_violations: new_v,
            resolved_violations: resolved,
            affected_components: affected,
            affected_nets: nets,
            check_time_ms: t.elapsed().as_secs_f64() * 1000.0,
        }
    }

    /// Apply a move and update the cached state.
    pub fn apply_move(&mut self, reference: &str, new_x: f64, new_y: f64) -> DRCDelta {
        let delta = self.check_move(reference, new_x, new_y);
        let Some(st) = self.state.as_mut() else {
            return delta;
        };
        let Some(old) = st.bounds(reference) else {
            return delta;
        };
        let (dx, dy) = (new_x - old.center_x(), new_y - old.center_y());
        let nb = old.translate(dx, dy);
        st.violations
            .retain(|v| !delta.resolved_violations.contains(v));
        st.violations.extend(delta.new_violations.iter().cloned());
        if let Some(e) = st.component_bounds.iter_mut().find(|(r, _)| r == reference) {
            e.1 = nb;
        }
        st.spatial_index.update(reference, nb);
        delta
    }

    pub fn current_violations(&self) -> Vec<Violation> {
        self.state
            .as_ref()
            .map(|s| s.violations.clone())
            .unwrap_or_default()
    }

    /// Footprint bounds over its pads at `position`.
    pub fn footprint_bounds(fp: &Footprint, position: (f64, f64)) -> Rectangle {
        if fp.pads.is_empty() {
            return Rectangle::from_center(position.0, position.1, 1.0, 1.0);
        }
        let (c, s) = rotate(fp.rotation);
        let mut r = Rectangle::new(
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for p in &fp.pads {
            let (lx, ly) = p.position;
            let ax = position.0 + lx * c - ly * s;
            let ay = position.1 + lx * s + ly * c;
            let (hw, hh) = (p.size.0 / 2.0, p.size.1 / 2.0);
            r.min_x = r.min_x.min(ax - hw);
            r.min_y = r.min_y.min(ay - hh);
            r.max_x = r.max_x.max(ax + hw);
            r.max_y = r.max_y.max(ay + hh);
        }
        r
    }

    fn check_all_clearances(&self) -> Vec<Violation> {
        let st = self.state.as_ref().unwrap();
        let mut out = Vec::new();
        let mut checked: std::collections::HashSet<(String, String)> = Default::default();
        for (r, b) in &st.component_bounds {
            for other in st.spatial_index.query(&b.expand(self.max_clearance)) {
                if other == *r {
                    continue;
                }
                let pair = if *r <= other {
                    (r.clone(), other.clone())
                } else {
                    (other.clone(), r.clone())
                };
                if !checked.insert(pair) {
                    continue;
                }
                if let Some(v) = self.pair_clearance(r, &other, None) {
                    out.push(v);
                }
            }
        }
        out
    }

    /// Minimum pad-pad (circumscribed-circle) clearance between two
    /// footprints, as a violation when below the rule minimum.
    fn pair_clearance(&self, r1: &str, r2: &str, fp1_pos: Option<(f64, f64)>) -> Option<Violation> {
        let fp1 = self.pcb.get_footprint(r1)?;
        let fp2 = self.pcb.get_footprint(r2)?;
        let p1 = fp1_pos.unwrap_or_else(|| self.position(fp1));
        let p2 = self.position(fp2);
        let (c1, s1) = rotate(fp1.rotation);
        let (c2, s2) = rotate(fp2.rotation);
        let mut best = f64::INFINITY;
        let mut loc = (0.0, 0.0);
        let mut items: Vec<String> = vec![];
        let mut nets: Vec<String> = vec![];
        for a in &fp1.pads {
            let ax = p1.0 + a.position.0 * c1 - a.position.1 * s1;
            let ay = p1.1 + a.position.0 * s1 + a.position.1 * c1;
            let ra = a.size.0.max(a.size.1) / 2.0;
            for b in &fp2.pads {
                if a.net_number == b.net_number && a.net_number != 0 {
                    continue;
                }
                let bx = p2.0 + b.position.0 * c2 - b.position.1 * s2;
                let by = p2.1 + b.position.0 * s2 + b.position.1 * c2;
                let rb = b.size.0.max(b.size.1) / 2.0;
                let d = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
                let cl = d - ra - rb;
                if cl < best {
                    best = cl;
                    loc = ((ax + bx) / 2.0, (ay + by) / 2.0);
                    items = vec![format!("{r1}-{}", a.number), format!("{r2}-{}", b.number)];
                    nets = vec![a.net_name.clone(), b.net_name.clone()];
                }
            }
        }
        let min = self.rules.min_clearance_mm;
        (best < min - CLEARANCE_EPSILON_MM).then(|| Violation {
            rule_id: "clearance".into(),
            message: format!("Clearance {best:.3}mm < minimum {min:.3}mm"),
            severity: "error".into(),
            location: loc,
            layer: String::new(),
            items,
            nets,
            actual_value: Some(best),
            required_value: Some(min),
        })
    }

    fn component_clearances(
        &self,
        reference: &str,
        bounds: Rectangle,
        nearby: &[String],
    ) -> Vec<Violation> {
        let Some(fp) = self.pcb.get_footprint(reference) else {
            return vec![];
        };
        let Some(orig) = self.state.as_ref().and_then(|s| s.bounds(reference)) else {
            return vec![];
        };
        let (dx, dy) = (
            bounds.center_x() - orig.center_x(),
            bounds.center_y() - orig.center_y(),
        );
        let p = self.position(fp);
        let new_pos = (p.0 + dx, p.1 + dy);
        nearby
            .iter()
            .filter(|o| *o != reference)
            .filter_map(|o| self.pair_clearance(reference, o, Some(new_pos)))
            .collect()
    }
}
