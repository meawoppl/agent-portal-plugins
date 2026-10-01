//! Predictive placement warnings (port of `kicad_tools.drc.predictive`):
//! routing-difficulty, congestion and intent-risk estimates for a proposed
//! component move. The placement session is abstracted as
//! [`PlacementView`] (upstream reads `session._optimizer` and
//! `session._drc_engine`).

use std::time::Instant;

use super::incremental::Rectangle;
use crate::pyjson::{py_round, Json};

/// A placement component as the predictive analyzer sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewComponent {
    pub reference: String,
    pub x: f64,
    pub y: f64,
    /// `(dx, dy, net_name)` pin offsets.
    pub pins: Vec<(f64, f64, String)>,
}

/// Read access to a placement session.
pub trait PlacementView {
    fn components(&self) -> &[ViewComponent];
    fn get_component(&self, reference: &str) -> Option<&ViewComponent> {
        self.components().iter().find(|c| c.reference == reference)
    }
    /// Component references whose bounds intersect `bounds` (empty when the
    /// DRC engine has no cached state).
    fn query(&self, bounds: &Rectangle) -> Vec<String>;
}

/// A declared design intent constraint.
#[derive(Debug, Clone, PartialEq)]
pub struct IntentDeclaration {
    pub interface_type: String,
    pub nets: Vec<String>,
    /// Constraint `type` values (e.g. `length_match`, `differential_pair`).
    pub constraints: Vec<String>,
}

/// A warning about potential future problems.
#[derive(Debug, Clone, PartialEq)]
pub struct PredictiveWarning {
    pub warning_type: String,
    pub message: String,
    pub confidence: f64,
    pub suggestion: Option<String>,
    pub affected_nets: Vec<String>,
    pub location: Option<(f64, f64)>,
}

impl PredictiveWarning {
    pub fn to_dict(&self) -> Json {
        let mut d = crate::jobj! {
            "type" => self.warning_type.as_str(),
            "message" => self.message.as_str(),
            "confidence" => Json::Float(py_round(self.confidence, 2)),
            "affected_nets" => &self.affected_nets,
        };
        if let Some(s) = self.suggestion.as_deref().filter(|s| !s.is_empty()) {
            d.set("suggestion", s);
        }
        if let Some((x, y)) = self.location {
            d.set(
                "location",
                crate::jobj! { "x" => Json::Float(py_round(x, 3)), "y" => Json::Float(py_round(y, 3)) },
            );
        }
        d
    }
}

pub const DIFFICULTY_INCREASE_THRESHOLD: f64 = 1.5;
pub const CONGESTION_UTILIZATION_THRESHOLD: f64 = 0.8;
pub const LENGTH_MATCH_TOLERANCE: f64 = 0.15;
pub const MIN_CONFIDENCE_THRESHOLD: f64 = 0.5;
pub const CONGESTION_AREA_SIZE: f64 = 10.0;
pub const PINS_PER_MM2_WARNING: f64 = 2.0;

/// Analyzes moves for potential future problems.
pub struct PredictiveAnalyzer<'a, V: PlacementView> {
    pub view: &'a V,
    pub intents: Vec<IntentDeclaration>,
    pub last_analysis_time_ms: f64,
}

fn uniq(v: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

impl<'a, V: PlacementView> PredictiveAnalyzer<'a, V> {
    pub fn new(view: &'a V, intents: Vec<IntentDeclaration>) -> Self {
        PredictiveAnalyzer {
            view,
            intents,
            last_analysis_time_ms: 0.0,
        }
    }

    /// Warnings for moving `reference` to `new_pos` (confidence-filtered).
    pub fn analyze_move(&mut self, reference: &str, new_pos: (f64, f64)) -> Vec<PredictiveWarning> {
        let t = Instant::now();
        let Some(comp) = self.view.get_component(reference) else {
            return vec![];
        };
        let current = (comp.x, comp.y);
        let mut w = self.routing_difficulty(reference, current, new_pos);
        w.extend(self.congestion(reference, new_pos));
        if !self.intents.is_empty() {
            w.extend(self.intent_risks(reference, new_pos));
        }
        w.retain(|x| x.confidence >= MIN_CONFIDENCE_THRESHOLD);
        self.last_analysis_time_ms = t.elapsed().as_secs_f64() * 1000.0;
        w
    }

    fn connected_nets(&self, reference: &str) -> Vec<String> {
        self.view
            .get_component(reference)
            .map(|c| uniq(c.pins.iter().filter(|p| !p.2.is_empty()).map(|p| p.2.clone())))
            .unwrap_or_default()
    }

    fn net_endpoints(&self, net: &str, exclude: Option<(f64, f64)>) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        for c in self.view.components() {
            for p in &c.pins {
                if p.2 == net {
                    let pos = (c.x + p.0, c.y + p.1);
                    if exclude.is_none_or(|e| (pos.0 - e.0).abs() > 0.1 || (pos.1 - e.1).abs() > 0.1) {
                        out.push(pos);
                    }
                }
            }
        }
        out
    }

    fn routing_bounds(a: (f64, f64), b: (f64, f64)) -> Rectangle {
        let m = 2.0;
        Rectangle::new(a.0.min(b.0) - m, a.1.min(b.1) - m, a.0.max(b.0) + m, a.1.max(b.1) + m)
    }

    fn pin_count(&self, refs: &[String]) -> usize {
        refs.iter()
            .filter_map(|r| self.view.get_component(r))
            .map(|c| c.pins.len())
            .sum()
    }

    fn estimate_congestion(&self, b: &Rectangle) -> f64 {
        let n = self.pin_count(&self.view.query(b));
        let area = b.width() * b.height();
        if area <= 0.0 {
            return 0.0;
        }
        (n as f64 / area).min(1.0)
    }

    fn route_difficulty(&self, net: &str, pos: (f64, f64)) -> f64 {
        let eps = self.net_endpoints(net, Some(pos));
        if eps.is_empty() {
            return 0.0;
        }
        let dist = crate::utils::pymath::py_sum(eps.iter().map(|e| (pos.0 - e.0).abs() + (pos.1 - e.1).abs()));
        let mut obstacle = 0.0;
        for e in &eps {
            obstacle += self.view.query(&Self::routing_bounds(pos, *e)).len() as f64 * 0.5;
        }
        let mut cong = 0.0;
        for e in &eps {
            cong += self.estimate_congestion(&Self::routing_bounds(pos, *e)) * 2.0;
        }
        dist + obstacle + cong
    }

    fn routing_difficulty(&self, reference: &str, current: (f64, f64), new_pos: (f64, f64)) -> Vec<PredictiveWarning> {
        let mut out = Vec::new();
        for net in self.connected_nets(reference) {
            let old = self.route_difficulty(&net, current);
            let new = self.route_difficulty(&net, new_pos);
            if old <= 0.0 {
                continue;
            }
            let ratio = new / old;
            if ratio > DIFFICULTY_INCREASE_THRESHOLD {
                let (dx, dy) = (new_pos.0 - current.0, new_pos.1 - current.1);
                let dir = if dx.abs() > dy.abs() {
                    if dx < 0.0 { "left" } else { "right" }
                } else if dy > 0.0 {
                    "up"
                } else {
                    "down"
                };
                out.push(PredictiveWarning {
                    warning_type: "routing_difficulty".into(),
                    message: format!("Routing {net} will be significantly harder from this position"),
                    confidence: (0.5 + (ratio - 1.5) * 0.2).min(0.9),
                    suggestion: Some(format!(
                        "Consider moving less far {dir} to maintain clear routing channel"
                    )),
                    affected_nets: vec![net],
                    location: Some(new_pos),
                });
            }
        }
        out
    }

    fn nets_in_area(&self, b: &Rectangle) -> Vec<String> {
        let mut nets = Vec::new();
        for r in self.view.query(b) {
            if let Some(c) = self.view.get_component(&r) {
                nets.extend(c.pins.iter().filter(|p| !p.2.is_empty()).map(|p| p.2.clone()));
            }
        }
        uniq(nets)
    }

    fn congestion(&self, reference: &str, new_pos: (f64, f64)) -> Vec<PredictiveWarning> {
        let h = CONGESTION_AREA_SIZE / 2.0;
        let area = Rectangle::new(new_pos.0 - h, new_pos.1 - h, new_pos.0 + h, new_pos.1 + h);
        let nearby: Vec<String> = self.view.query(&area).into_iter().filter(|r| r != reference).collect();
        let mut pins = self.pin_count(&nearby);
        if let Some(c) = self.view.get_component(reference) {
            pins += c.pins.len();
        }
        let density = pins as f64 / (CONGESTION_AREA_SIZE * CONGESTION_AREA_SIZE);
        let pitch = 0.2 + 0.15;
        let capacity = area.width() / pitch * (area.height() / pitch) * 0.5;
        let mut refs = nearby.clone();
        refs.push(reference.to_string());
        let required = uniq(
            refs.iter()
                .filter_map(|r| self.view.get_component(r))
                .flat_map(|c| c.pins.iter())
                .filter(|p| !p.2.is_empty() && !["GND", "VCC", "VDD"].contains(&p.2.as_str()))
                .map(|p| p.2.clone()),
        )
        .len() as f64;
        let mut out = Vec::new();
        if capacity > 0.0 {
            let util = required / capacity;
            if util > CONGESTION_UTILIZATION_THRESHOLD {
                out.push(PredictiveWarning {
                    warning_type: "congestion".into(),
                    message: format!(
                        "Area around ({:.1}, {:.1}) is becoming congested ({}% routing utilization)",
                        new_pos.0,
                        new_pos.1,
                        (util * 100.0) as i64
                    ),
                    confidence: (0.5 + (util - 0.8) * 1.5).min(0.85),
                    suggestion: Some("Consider spreading components or using inner layers".into()),
                    affected_nets: self.nets_in_area(&area),
                    location: Some(new_pos),
                });
            }
        } else if density > PINS_PER_MM2_WARNING {
            out.push(PredictiveWarning {
                warning_type: "congestion".into(),
                message: format!(
                    "High pin density ({density:.1} pins/mm²) around ({:.1}, {:.1})",
                    new_pos.0, new_pos.1
                ),
                confidence: 0.6,
                suggestion: Some("Consider spreading components for easier routing".into()),
                affected_nets: self.nets_in_area(&area),
                location: Some(new_pos),
            });
        }
        out
    }

    fn net_length(&self, net: &str) -> f64 {
        let eps = self.net_endpoints(net, None);
        if eps.len() < 2 {
            return 0.0;
        }
        let n = eps.len() as f64;
        let cx = crate::utils::pymath::py_sum(eps.iter().map(|p| p.0)) / n;
        let cy = crate::utils::pymath::py_sum(eps.iter().map(|p| p.1)) / n;
        let mut total = 0.0;
        for e in &eps {
            total += (e.0 - cx).abs() + (e.1 - cy).abs();
        }
        total
    }

    fn intent_risks(&self, reference: &str, new_pos: (f64, f64)) -> Vec<PredictiveWarning> {
        let Some(comp) = self.view.get_component(reference) else {
            return vec![];
        };
        let current = (comp.x, comp.y);
        let connected = self.connected_nets(reference);
        let mut out = Vec::new();
        for intent in &self.intents {
            let overlap: Vec<String> = connected.iter().filter(|n| intent.nets.contains(n)).cloned().collect();
            if overlap.is_empty() {
                continue;
            }
            for c in &intent.constraints {
                let w = match c.as_str() {
                    "length_match" => self.length_match_risk(intent, current, new_pos, &overlap),
                    "differential_pair" => self.diff_pair_risk(intent, new_pos),
                    _ => None,
                };
                out.extend(w);
            }
        }
        out
    }

    fn length_match_risk(
        &self,
        intent: &IntentDeclaration,
        current: (f64, f64),
        new_pos: (f64, f64),
        affected: &[String],
    ) -> Option<PredictiveWarning> {
        let mut lengths: Vec<(String, f64)> = Vec::new();
        for n in &intent.nets {
            let l = self.net_length(n);
            match lengths.iter_mut().find(|(k, _)| k == n) {
                Some(e) => e.1 = l,
                None => lengths.push((n.clone(), l)),
            }
        }
        if lengths.is_empty() {
            return None;
        }
        let (dx, dy) = (new_pos.0 - current.0, new_pos.1 - current.1);
        let mv = (dx * dx + dy * dy).sqrt();
        for n in affected {
            if let Some(e) = lengths.iter_mut().find(|(k, _)| k == n) {
                e.1 += mv;
            }
        }
        if lengths.len() < 2 {
            return None;
        }
        let max = lengths.iter().map(|e| e.1).fold(f64::NEG_INFINITY, f64::max);
        let min = lengths.iter().map(|e| e.1).fold(f64::INFINITY, f64::min);
        if min <= 0.0 {
            return None;
        }
        let var = (max - min) / min;
        (var > LENGTH_MATCH_TOLERANCE).then(|| PredictiveWarning {
            warning_type: "intent_risk".into(),
            message: format!(
                "This move may make {} length matching difficult (projected {:.0}% variation)",
                intent.interface_type,
                var * 100.0
            ),
            confidence: 0.6,
            suggestion: Some("Keep matched traces similar length".into()),
            affected_nets: intent.nets.clone(),
            location: Some(new_pos),
        })
    }

    fn diff_pair_risk(&self, intent: &IntentDeclaration, new_pos: (f64, f64)) -> Option<PredictiveWarning> {
        if intent.nets.len() < 2 {
            return None;
        }
        let e1 = self.net_endpoints(&intent.nets[0], None);
        let e2 = self.net_endpoints(&intent.nets[1], None);
        if e1.is_empty() || e2.is_empty() {
            return None;
        }
        let spread = e1
            .iter()
            .any(|a| e2.iter().any(|b| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() > 5.0));
        spread.then(|| PredictiveWarning {
            warning_type: "intent_risk".into(),
            message: format!(
                "Differential pair {} endpoints may spread further apart, affecting signal \
                 integrity",
                intent.interface_type
            ),
            confidence: 0.55,
            suggestion: Some("Keep differential pair endpoints close together".into()),
            affected_nets: intent.nets.clone(),
            location: Some(new_pos),
        })
    }
}
