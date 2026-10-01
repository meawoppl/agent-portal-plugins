//! DRC fix suggestions with actionable remediation guidance (port of
//! `kicad_tools.drc.suggestions`).

use std::collections::BTreeMap;

use super::violation::{DRCViolation, ViolationType};
use crate::jobj;
use crate::pyjson::{py_round, Json};

/// Types of fix actions that can be suggested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FixAction {
    Move,
    Reroute,
    Resize,
    Delete,
    Connect,
    AdjustRule,
}

impl FixAction {
    pub fn value(self) -> &'static str {
        match self {
            FixAction::Move => "move",
            FixAction::Reroute => "reroute",
            FixAction::Resize => "resize",
            FixAction::Delete => "delete",
            FixAction::Connect => "connect",
            FixAction::AdjustRule => "adjust_rule",
        }
    }
}

/// Specific fix suggestion for a DRC violation.
#[derive(Debug, Clone, PartialEq)]
pub struct FixSuggestion {
    pub action: FixAction,
    pub target: String,
    /// Action-specific parameters, in insertion order.
    pub parameters: Vec<(String, Json)>,
    pub description: String,
    pub priority: i64,
    pub complexity: String,
    pub alternatives: Vec<FixSuggestion>,
}

impl FixSuggestion {
    pub fn new(
        action: FixAction,
        target: impl Into<String>,
        description: impl Into<String>,
        priority: i64,
        complexity: &str,
    ) -> Self {
        FixSuggestion {
            action,
            target: target.into(),
            parameters: Vec::new(),
            description: description.into(),
            priority,
            complexity: complexity.to_string(),
            alternatives: Vec::new(),
        }
    }

    fn with_params(mut self, params: Vec<(&str, Json)>) -> Self {
        self.parameters = params
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        self
    }

    fn with_alternatives(mut self, alternatives: Vec<FixSuggestion>) -> Self {
        self.alternatives = alternatives;
        self
    }

    pub fn parameter(&self, key: &str) -> Option<&Json> {
        self.parameters
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    pub fn to_json(&self) -> Json {
        jobj! {
            "action" => self.action.value(),
            "target" => self.target.as_str(),
            "parameters" => Json::Obj(self.parameters.clone()),
            "description" => self.description.as_str(),
            "priority" => self.priority,
            "complexity" => self.complexity.as_str(),
            "alternatives" => Json::Arr(self.alternatives.iter().map(FixSuggestion::to_json).collect()),
        }
    }
}

impl std::fmt::Display for FixSuggestion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.description)
    }
}

/// Human-readable direction from a delta vector (+Y is down in KiCad).
pub fn direction_name(dx: f64, dy: f64) -> String {
    if dx.abs() < 0.01 && dy.abs() < 0.01 {
        return "in place".into();
    }
    let mut directions = Vec::new();
    if dy < -0.01 {
        directions.push("up");
    } else if dy > 0.01 {
        directions.push("down");
    }
    if dx < -0.01 {
        directions.push("left");
    } else if dx > 0.01 {
        directions.push("right");
    }
    if directions.is_empty() {
        "in place".into()
    } else {
        directions.join("-")
    }
}

fn r3(x: f64) -> Json {
    Json::Float(py_round(x, 3))
}

fn first_net_or(v: &DRCViolation, default: &str) -> String {
    v.nets
        .first()
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

/// Fix suggestion for a clearance violation (`margin` mm beyond minimum).
pub fn calculate_clearance_fix(violation: &DRCViolation, margin: f64) -> Option<FixSuggestion> {
    if !violation.is_clearance() {
        return None;
    }
    let (Some(required), Some(actual)) = (violation.required_value_mm, violation.actual_value_mm)
    else {
        return Some(FixSuggestion::new(
            FixAction::Reroute,
            "affected traces",
            "Reroute traces to increase clearance",
            2,
            "moderate",
        ));
    };
    let delta = required - actual + margin;
    let items = &violation.items;
    let (element1, element2) = match items.len() {
        0 => ("element".to_string(), "adjacent element".to_string()),
        1 => (items[0].clone(), "adjacent element".to_string()),
        _ => (items[0].clone(), items[1].clone()),
    };

    if violation.locations.len() >= 2 {
        let (loc1, loc2) = (&violation.locations[0], &violation.locations[1]);
        let mut dx = loc1.x_mm - loc2.x_mm;
        let mut dy = loc1.y_mm - loc2.y_mm;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance > 0.001 {
            dx = (dx / distance) * delta;
            dy = (dy / distance) * delta;
        } else {
            dx = delta;
            dy = 0.0;
        }
        let direction = direction_name(dx, dy);
        return Some(
            FixSuggestion::new(
                FixAction::Move,
                element1.clone(),
                format!("Move {element1} {delta:.2}mm {direction}"),
                1,
                "easy",
            )
            .with_params(vec![
                ("dx", r3(dx)),
                ("dy", r3(dy)),
                ("distance_mm", r3(delta)),
            ])
            .with_alternatives(vec![
                FixSuggestion::new(
                    FixAction::Move,
                    element2.clone(),
                    format!("Move {element2} {delta:.2}mm {}", direction_name(-dx, -dy)),
                    2,
                    "easy",
                )
                .with_params(vec![
                    ("dx", r3(-dx)),
                    ("dy", r3(-dy)),
                    ("distance_mm", r3(delta)),
                ]),
                FixSuggestion::new(
                    FixAction::Reroute,
                    first_net_or(violation, "affected net"),
                    format!("Reroute net to avoid {element2}"),
                    3,
                    "moderate",
                ),
            ]),
        );
    }

    Some(
        FixSuggestion::new(
            FixAction::Move,
            element1.clone(),
            format!("Increase clearance by {delta:.2}mm (move {element1} away from {element2})"),
            1,
            "easy",
        )
        .with_params(vec![("distance_mm", r3(delta))])
        .with_alternatives(vec![FixSuggestion::new(
            FixAction::Reroute,
            first_net_or(violation, "affected net"),
            "Reroute traces to increase clearance",
            2,
            "moderate",
        )]),
    )
}

fn track_width_fix(v: &DRCViolation) -> Option<FixSuggestion> {
    let Some(required) = v.required_value_mm else {
        return Some(FixSuggestion::new(
            FixAction::Resize,
            "track",
            "Increase track width to meet minimum requirement",
            1,
            "easy",
        ));
    };
    let delta = required - v.actual_value_mm.unwrap_or(0.0);
    let net = first_net_or(v, "affected net");
    Some(
        FixSuggestion::new(
            FixAction::Resize,
            net,
            format!("Increase track width to {required:.3}mm (+{delta:.3}mm)"),
            1,
            "easy",
        )
        .with_params(vec![
            ("new_width_mm", r3(required)),
            (
                "increase_mm",
                if delta > 0.0 { r3(delta) } else { Json::Int(0) },
            ),
        ]),
    )
}

fn via_fix(v: &DRCViolation) -> Option<FixSuggestion> {
    let required = v.required_value_mm;
    if v.vtype == ViolationType::VIA_ANNULAR_WIDTH {
        if let Some(required) = required {
            let delta = required - v.actual_value_mm.unwrap_or(0.0);
            return Some(
                FixSuggestion::new(
                    FixAction::Resize,
                    "via",
                    format!("Increase via pad diameter by {:.3}mm", delta * 2.0),
                    1,
                    "easy",
                )
                .with_params(vec![("increase_pad_mm", r3(delta * 2.0))])
                .with_alternatives(vec![FixSuggestion::new(
                    FixAction::Resize,
                    "via",
                    format!("Decrease via drill by {:.3}mm", delta * 2.0),
                    2,
                    "easy",
                )
                .with_params(vec![("decrease_drill_mm", r3(delta * 2.0))])]),
            );
        }
    }
    if v.vtype == ViolationType::VIA_HOLE_LARGER_THAN_PAD {
        return Some(FixSuggestion::new(
            FixAction::Resize,
            "via",
            "Increase via pad size or decrease drill size",
            1,
            "easy",
        ));
    }
    if matches!(
        v.vtype,
        ViolationType::MICRO_VIA_HOLE_TOO_SMALL | ViolationType::DRILL_HOLE_TOO_SMALL
    ) {
        if let Some(required) = required {
            return Some(
                FixSuggestion::new(
                    FixAction::Resize,
                    "via/hole",
                    format!("Increase drill size to at least {required:.3}mm"),
                    1,
                    "easy",
                )
                .with_params(vec![("min_drill_mm", r3(required))]),
            );
        }
    }
    None
}

fn connection_fix(v: &DRCViolation) -> Option<FixSuggestion> {
    if v.vtype == ViolationType::UNCONNECTED_ITEMS {
        let net = first_net_or(v, "unconnected net");
        let items = &v.items;
        let description = if items.len() >= 2 {
            format!(
                "Route connection between {} and {} on net {net}",
                items[0], items[1]
            )
        } else if let Some(first) = items.first() {
            format!("Complete routing for {first} on net {net}")
        } else {
            format!("Complete routing for net {net}")
        };
        let params = if items.is_empty() {
            vec![]
        } else {
            vec![("items", Json::from(items))]
        };
        return Some(
            FixSuggestion::new(FixAction::Connect, net, description, 1, "moderate")
                .with_params(params),
        );
    }
    if v.vtype == ViolationType::SHORTING_ITEMS {
        let nets = &v.nets;
        let description = if nets.len() >= 2 {
            format!("Remove short between nets {} and {}", nets[0], nets[1])
        } else {
            "Remove shorting trace/via".to_string()
        };
        let params = if nets.is_empty() {
            vec![]
        } else {
            vec![("nets", Json::from(nets))]
        };
        return Some(
            FixSuggestion::new(
                FixAction::Delete,
                "shorting element",
                description,
                1,
                "easy",
            )
            .with_params(params)
            .with_alternatives(vec![FixSuggestion::new(
                FixAction::Reroute,
                first_net_or(v, "affected net"),
                "Reroute to avoid short",
                2,
                "moderate",
            )]),
        );
    }
    None
}

fn silkscreen_fix(v: &DRCViolation) -> Option<FixSuggestion> {
    match v.vtype {
        ViolationType::SILK_OVER_COPPER => Some(
            FixSuggestion::new(
                FixAction::Move,
                "silkscreen element",
                "Move silkscreen away from copper/pads",
                2,
                "trivial",
            )
            .with_alternatives(vec![FixSuggestion::new(
                FixAction::Delete,
                "silkscreen element",
                "Remove silkscreen element if not needed",
                3,
                "trivial",
            )]),
        ),
        ViolationType::SILK_OVERLAP => Some(FixSuggestion::new(
            FixAction::Move,
            "silkscreen element",
            "Move silkscreen elements to eliminate overlap",
            2,
            "trivial",
        )),
        _ => None,
    }
}

fn edge_clearance_fix(v: &DRCViolation) -> Option<FixSuggestion> {
    if let (Some(required), Some(actual)) = (v.required_value_mm, v.actual_value_mm) {
        let delta = required - actual + 0.1;
        return Some(
            FixSuggestion::new(
                FixAction::Move,
                "copper element",
                format!("Move copper {delta:.2}mm away from board edge"),
                1,
                "easy",
            )
            .with_params(vec![("distance_mm", r3(delta))])
            .with_alternatives(vec![FixSuggestion::new(
                FixAction::Reroute,
                "edge traces",
                "Reroute traces away from board edge",
                2,
                "moderate",
            )]),
        );
    }
    Some(FixSuggestion::new(
        FixAction::Move,
        "copper element",
        "Move copper away from board edge to meet clearance",
        1,
        "easy",
    ))
}

/// Recommended fix for a DRC violation, or `None` when no suggestion exists.
pub fn generate_fix_suggestions(v: &DRCViolation) -> Option<FixSuggestion> {
    use ViolationType::*;
    if v.vtype == CLEARANCE_PAD_PAD && v.is_same_component_pad_clearance() {
        return Some(FixSuggestion::new(
            FixAction::AdjustRule,
            "clearance rule",
            "No action needed - pad-pad clearance is inherent to IC footprint geometry \
             (adjacent pins within a single package)",
            3,
            "trivial",
        ));
    }
    match v.vtype {
        CLEARANCE => return calculate_clearance_fix(v, 0.1),
        COPPER_EDGE_CLEARANCE => return edge_clearance_fix(v),
        TRACK_WIDTH => return track_width_fix(v),
        VIA_ANNULAR_WIDTH
        | VIA_HOLE_LARGER_THAN_PAD
        | MICRO_VIA_HOLE_TOO_SMALL
        | DRILL_HOLE_TOO_SMALL => return via_fix(v),
        UNCONNECTED_ITEMS | SHORTING_ITEMS => return connection_fix(v),
        SILK_OVER_COPPER | SILK_OVERLAP => return silkscreen_fix(v),
        _ => {}
    }
    if v.vtype == COURTYARD_OVERLAP && v.items.len() >= 2 {
        return Some(FixSuggestion::new(
            FixAction::Move,
            v.items[0].clone(),
            format!(
                "Move {} to eliminate courtyard overlap with {}",
                v.items[0], v.items[1]
            ),
            1,
            "easy",
        ));
    }
    if v.vtype == SOLDER_MASK_BRIDGE {
        if v.is_fine_pitch_inherent(0.1) {
            return Some(FixSuggestion::new(
                FixAction::AdjustRule,
                "solder mask rule",
                "No action needed - solder mask bridge is inherent to fine-pitch IC \
                 footprint and acceptable for manufacturing",
                3,
                "trivial",
            ));
        }
        return Some(FixSuggestion::new(
            FixAction::Move,
            "pad/via",
            "Increase spacing between pads to allow solder mask bridge",
            1,
            "moderate",
        ));
    }
    if v.vtype == HOLE_NEAR_HOLE {
        if let (Some(required), Some(actual)) = (v.required_value_mm, v.actual_value_mm) {
            let delta = required - actual + 0.1;
            return Some(
                FixSuggestion::new(
                    FixAction::Move,
                    "hole/via",
                    format!("Move hole {delta:.2}mm away from adjacent hole"),
                    1,
                    "easy",
                )
                .with_params(vec![("distance_mm", r3(delta))]),
            );
        }
    }
    if matches!(
        v.vtype,
        FOOTPRINT | DUPLICATE_FOOTPRINT | EXTRA_FOOTPRINT | MISSING_FOOTPRINT
    ) {
        return Some(FixSuggestion::new(
            FixAction::AdjustRule,
            "schematic/footprint",
            "Review schematic and footprint assignments",
            1,
            "moderate",
        ));
    }
    None
}

/// `generate_suggestions(report)`-style helper: suggestion per violation index.
pub fn generate_suggestions(violations: &[DRCViolation]) -> BTreeMap<usize, FixSuggestion> {
    violations
        .iter()
        .enumerate()
        .filter_map(|(i, v)| generate_fix_suggestions(v).map(|s| (i, s)))
        .collect()
}
