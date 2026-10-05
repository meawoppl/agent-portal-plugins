//! Port of `kicad_tools.recovery.applicator`: apply placement strategies
//! (move, move-multiple, rotate, mirror/layer-flip, reorder-pins) to a PCB
//! and check them for safety first.

use std::collections::BTreeMap;

use anyhow::{bail, Result};

use super::types::{Rectangle, ResolutionStrategy, StrategyType};
use crate::pyjson::Json;
use crate::schema::pcb::{GraphicItem, Pcb};
use crate::schema::physical_identity::footprint_keys;
use crate::sexp::{SExp, Value};

/// Swap a layer name to the other board side (`F.* <-> B.*`).
pub fn flip_layer_side(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("F.") {
        format!("B.{rest}")
    } else if let Some(rest) = name.strip_prefix("B.") {
        format!("F.{rest}")
    } else {
        name.to_string()
    }
}

/// Result of applying a strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationResult {
    pub success: bool,
    pub components_moved: Vec<String>,
    pub message: String,
    pub conflicts_created: i64,
}

impl ApplicationResult {
    fn fail(message: impl Into<String>) -> Self {
        ApplicationResult {
            success: false,
            components_moved: Vec::new(),
            message: message.into(),
            conflicts_created: 0,
        }
    }

    fn ok(moved: Vec<String>, message: impl Into<String>) -> Self {
        ApplicationResult {
            success: true,
            components_moved: moved,
            message: message.into(),
            conflicts_created: 0,
        }
    }
}

const COSMETIC_TAGS: [&str; 9] = [
    "property",
    "fp_text",
    "fp_text_box",
    "fp_line",
    "fp_rect",
    "fp_circle",
    "fp_arc",
    "fp_poly",
    "fp_curve",
];

fn num(params: &Json, key: &str) -> Option<f64> {
    params.get(key).and_then(|v| v.as_f64())
}

/// Applies resolution strategies to a PCB.
#[derive(Debug, Clone, Copy, Default)]
pub struct StrategyApplicator;

impl StrategyApplicator {
    /// Board margin (mm) kept clear of the outline.
    pub const BOARD_EDGE_MARGIN: f64 = 1.0;
    /// Maximum move distance (mm).
    pub const MAX_MOVE_DISTANCE: f64 = 10.0;

    pub fn new() -> Self {
        StrategyApplicator
    }

    /// Footprint index for a physical key or an unambiguous reference;
    /// errors on an ambiguous authored reference.
    pub fn find_footprint(pcb: &Pcb, target: &str) -> Result<Option<usize>> {
        let fps = pcb.footprints();
        let ids: Vec<(&str, &str)> = fps
            .iter()
            .map(|f| (f.reference.as_str(), f.uuid.as_str()))
            .collect();
        if let Some(i) = footprint_keys(&ids).iter().position(|k| k == target) {
            return Ok(Some(i));
        }
        if fps.iter().filter(|f| f.reference == target).count() > 1 {
            bail!("Ambiguous footprint reference {target:?}; use a physical component ID");
        }
        Ok(None)
    }

    /// Apply `strategy`. Every placement selector is resolved first so an
    /// ambiguous reference cannot leave a partial edit.
    pub fn apply_strategy(
        &self,
        pcb: &mut Pcb,
        strategy: &ResolutionStrategy,
    ) -> Result<ApplicationResult> {
        for a in &strategy.actions {
            if matches!(
                a.kind.as_str(),
                "move" | "rotate" | "mirror" | "reorder_pins"
            ) {
                Self::find_footprint(pcb, &a.target)?;
            }
        }
        Ok(match strategy.kind {
            StrategyType::MoveComponent => self.apply_move_component(pcb, strategy)?,
            StrategyType::MoveMultiple => self.apply_move_multiple(pcb, strategy)?,
            StrategyType::RotateComponent => self.apply_rotate_component(pcb, strategy)?,
            StrategyType::MirrorComponent => self.apply_mirror_component(pcb, strategy)?,
            StrategyType::ReorderPins => self.apply_reorder_pins(pcb, strategy)?,
            other => ApplicationResult::fail(format!(
                "Strategy type {} cannot be applied to placement",
                other.value()
            )),
        })
    }

    fn apply_move_component(
        &self,
        pcb: &mut Pcb,
        s: &ResolutionStrategy,
    ) -> Result<ApplicationResult> {
        let Some(a) = s.actions.first() else {
            return Ok(ApplicationResult::fail("No actions in strategy"));
        };
        if a.kind != "move" {
            return Ok(ApplicationResult::fail(format!(
                "Expected move action, got {}",
                a.kind
            )));
        }
        let (Some(x), Some(y)) = (num(&a.params, "x"), num(&a.params, "y")) else {
            return Ok(ApplicationResult::fail(
                "Missing x or y in move action params",
            ));
        };
        let Some(i) = Self::find_footprint(pcb, &a.target)? else {
            return Ok(ApplicationResult::fail(format!(
                "Component {} not found",
                a.target
            )));
        };
        let mut fp = pcb.footprint_mut_at(i).expect("index resolved");
        let (ox, oy) = fp.position;
        fp.set_position((x, y));
        Ok(ApplicationResult::ok(
            vec![a.target.clone()],
            format!(
                "Moved {} from ({ox:.2}, {oy:.2}) to ({x:.2}, {y:.2})",
                a.target
            ),
        ))
    }

    fn apply_move_multiple(
        &self,
        pcb: &mut Pcb,
        s: &ResolutionStrategy,
    ) -> Result<ApplicationResult> {
        let mut moved = Vec::new();
        let mut messages = Vec::new();
        for a in s.actions.iter().filter(|a| a.kind == "move") {
            let (Some(x), Some(y)) = (num(&a.params, "x"), num(&a.params, "y")) else {
                continue;
            };
            let Some(i) = Self::find_footprint(pcb, &a.target)? else {
                messages.push(format!("{}: not found", a.target));
                continue;
            };
            let mut fp = pcb.footprint_mut_at(i).expect("index resolved");
            let (ox, oy) = fp.position;
            fp.set_position((x, y));
            moved.push(a.target.clone());
            messages.push(format!(
                "{}: ({ox:.2}, {oy:.2}) -> ({x:.2}, {y:.2})",
                a.target
            ));
        }
        if moved.is_empty() {
            return Ok(ApplicationResult::fail("No components moved"));
        }
        let n = moved.len();
        Ok(ApplicationResult::ok(
            moved,
            format!("Moved {n} components: {}", messages.join("; ")),
        ))
    }

    fn apply_rotate_component(
        &self,
        pcb: &mut Pcb,
        s: &ResolutionStrategy,
    ) -> Result<ApplicationResult> {
        let Some(a) = s.actions.first() else {
            return Ok(ApplicationResult::fail("No actions in strategy"));
        };
        if a.kind != "rotate" {
            return Ok(ApplicationResult::fail(format!(
                "Expected rotate action, got {}",
                a.kind
            )));
        }
        let Some(delta) = num(&a.params, "rotation_delta") else {
            return Ok(ApplicationResult::fail(
                "Missing rotation_delta in rotate action params",
            ));
        };
        let Some(i) = Self::find_footprint(pcb, &a.target)? else {
            return Ok(ApplicationResult::fail(format!(
                "Component {} not found",
                a.target
            )));
        };
        let mut fp = pcb.footprint_mut_at(i).expect("index resolved");
        let old = fp.rotation;
        let new = (old + delta).rem_euclid(360.0);
        fp.set_rotation(new);
        for pi in 0..fp.pad_count() {
            let mut pad = fp.pad_mut(pi).expect("pad index");
            let r = (pad.rotation + delta).rem_euclid(360.0);
            pad.set_rotation(r);
        }
        Ok(ApplicationResult::ok(
            vec![a.target.clone()],
            format!(
                "Rotated {} by {delta:.1} deg ({old:.1} -> {new:.1})",
                a.target
            ),
        ))
    }

    fn apply_mirror_component(
        &self,
        pcb: &mut Pcb,
        s: &ResolutionStrategy,
    ) -> Result<ApplicationResult> {
        let Some(a) = s.actions.first() else {
            return Ok(ApplicationResult::fail("No actions in strategy"));
        };
        if a.kind != "mirror" {
            return Ok(ApplicationResult::fail(format!(
                "Expected mirror action, got {}",
                a.kind
            )));
        }
        let Some(i) = Self::find_footprint(pcb, &a.target)? else {
            return Ok(ApplicationResult::fail(format!(
                "Component {} not found",
                a.target
            )));
        };
        let (old_layer, new_layer);
        {
            let mut fp = pcb.footprint_mut_at(i).expect("index resolved");
            old_layer = fp.layer.clone();
            new_layer = flip_layer_side(&old_layer);
            fp.set_layer(&new_layer);
            let rot = (180.0 - fp.rotation).rem_euclid(360.0);
            fp.set_rotation(rot);
            for pi in 0..fp.pad_count() {
                let mut pad = fp.pad_mut(pi).expect("pad index");
                let (px, py) = pad.position;
                pad.set_position((px, -py));
                let r = (180.0 - pad.rotation).rem_euclid(360.0);
                pad.set_rotation(r);
                let layers: Vec<String> = pad.layers.iter().map(|l| flip_layer_side(l)).collect();
                let refs: Vec<&str> = layers.iter().map(String::as_str).collect();
                pad.set_layers(&refs);
            }
        }
        if let Some(ni) = pcb.footprint_node_index(i) {
            let node = &mut pcb.sexp_mut().children[ni];
            for child in node.children.iter_mut() {
                if child.tag().is_some_and(|t| COSMETIC_TAGS.contains(&t)) {
                    mirror_cosmetic_node(child);
                }
            }
            // Re-derive footprint texts/graphics from the flipped tree.
            pcb.reload_from_tree()?;
        }
        Ok(ApplicationResult::ok(
            vec![a.target.clone()],
            format!(
                "Mirrored {} (layer {old_layer} -> {new_layer}, left/right flip)",
                a.target
            ),
        ))
    }

    fn apply_reorder_pins(
        &self,
        pcb: &mut Pcb,
        s: &ResolutionStrategy,
    ) -> Result<ApplicationResult> {
        let Some(a) = s.actions.first() else {
            return Ok(ApplicationResult::fail("No actions in strategy"));
        };
        if a.kind != "reorder_pins" {
            return Ok(ApplicationResult::fail(format!(
                "Expected reorder_pins action, got {}",
                a.kind
            )));
        }
        let pad_map: Vec<(String, String)> = match a.params.get("pad_map") {
            Some(Json::Obj(items)) => items
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.py_repr()),
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        if pad_map.is_empty() {
            return Ok(ApplicationResult::fail(format!(
                "reorder_pins action for {} carries an empty pad_map",
                a.target
            )));
        }
        let Some(i) = Self::find_footprint(pcb, &a.target)? else {
            return Ok(ApplicationResult::fail(format!(
                "Component {} not found",
                a.target
            )));
        };
        let fp = &pcb.footprints()[i];
        let reference = fp.reference.clone();
        let mut missing: Vec<&String> = pad_map
            .iter()
            .map(|(k, _)| k)
            .filter(|k| !fp.pads.iter().any(|p| &p.number == *k))
            .collect();
        missing.sort();
        if !missing.is_empty() {
            let m: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
            return Ok(ApplicationResult::fail(format!(
                "Pad(s) {} not found on {}",
                m.join(", "),
                a.target
            )));
        }
        for (pad, net) in &pad_map {
            pcb.assign_net_to_footprint_pad(&reference, pad, net);
        }
        let sorted: BTreeMap<&String, &String> = pad_map.iter().map(|(k, v)| (k, v)).collect();
        let bindings: Vec<String> = sorted.iter().map(|(k, v)| format!("{k}->{v}")).collect();
        Ok(ApplicationResult::ok(
            Vec::new(),
            format!(
                "Re-bound {} pad(s) on {} ({})",
                pad_map.len(),
                a.target,
                bindings.join(", ")
            ),
        ))
    }

    /// Whether `strategy` can be applied safely (targets exist; moves stay
    /// on the board and within [`Self::MAX_MOVE_DISTANCE`]).
    pub fn is_safe_to_apply(&self, strategy: &ResolutionStrategy, pcb: &Pcb) -> Result<bool> {
        let non_moving = match strategy.kind {
            StrategyType::RotateComponent => Some("rotate"),
            StrategyType::MirrorComponent => Some("mirror"),
            StrategyType::ReorderPins => Some("reorder_pins"),
            _ => None,
        };
        if let Some(expected) = non_moving {
            for a in strategy.actions.iter().filter(|a| a.kind == expected) {
                if Self::find_footprint(pcb, &a.target)?.is_none() {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if !matches!(
            strategy.kind,
            StrategyType::MoveComponent | StrategyType::MoveMultiple
        ) {
            return Ok(false);
        }
        let Some(bounds) = Self::get_board_bounds(pcb) else {
            return Ok(false);
        };
        for a in strategy.actions.iter().filter(|a| a.kind == "move") {
            let (Some(x), Some(y)) = (num(&a.params, "x"), num(&a.params, "y")) else {
                return Ok(false);
            };
            let Some(i) = Self::find_footprint(pcb, &a.target)? else {
                return Ok(false);
            };
            if !Self::position_within_bounds(x, y, &bounds) {
                return Ok(false);
            }
            let (ox, oy) = pcb.footprints()[i].position;
            if ((x - ox).powi(2) + (y - oy).powi(2)).sqrt() > Self::MAX_MOVE_DISTANCE {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Vector clearing `failure_area` (away from its centre unless a
    /// direction is given).
    pub fn calculate_move_vector(
        &self,
        pcb: &Pcb,
        r: &str,
        failure_area: &Rectangle,
        direction: Option<(f64, f64)>,
    ) -> Option<(f64, f64)> {
        let fp = pcb.footprints().iter().find(|f| f.reference == r)?;
        let (cx, cy) = failure_area.center();
        let (mut dx, mut dy) = match direction {
            None => (fp.position.0 - cx, fp.position.1 - cy),
            Some(d) => d,
        };
        let dist = (dx * dx + dy * dy).sqrt();
        if direction.is_none() && dist < 0.1 {
            (dx, dy) = (1.0, 0.0);
        } else if direction.is_none() || dist > 0.0 {
            dx /= dist;
            dy /= dist;
        }
        let clear = failure_area.width().max(failure_area.height()) / 2.0 + 0.5;
        Some((dx * clear, dy * clear))
    }

    /// Vector spreading `r` away from `center` by `spread_distance`.
    ///
    /// For a component at the centre, upstream picks a direction from
    /// Python's (seeded, run-dependent) `hash(ref)`; here a stable FNV-1a
    /// hash of the reference is used instead.
    pub fn calculate_spread_vector(
        &self,
        pcb: &Pcb,
        r: &str,
        center: (f64, f64),
        spread_distance: f64,
    ) -> Option<(f64, f64)> {
        let fp = pcb.footprints().iter().find(|f| f.reference == r)?;
        let (mut dx, mut dy) = (fp.position.0 - center.0, fp.position.1 - center.1);
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 0.1 {
            let h = r.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
            });
            let angle = ((h % 360) as f64).to_radians();
            (dx, dy) = (angle.cos(), angle.sin());
        } else {
            dx /= dist;
            dy /= dist;
        }
        Some((dx * spread_distance, dy * spread_distance))
    }

    /// New positions a strategy would produce, without applying it.
    pub fn simulate_placement_change(
        &self,
        strategy: &ResolutionStrategy,
    ) -> Vec<(String, (f64, f64))> {
        let mut out: Vec<(String, (f64, f64))> = Vec::new();
        for a in strategy.actions.iter().filter(|a| a.kind == "move") {
            if let (Some(x), Some(y)) = (num(&a.params, "x"), num(&a.params, "y")) {
                match out.iter_mut().find(|(r, _)| *r == a.target) {
                    Some(slot) => slot.1 = (x, y),
                    None => out.push((a.target.clone(), (x, y))),
                }
            }
        }
        out
    }

    /// Edge.Cuts bounds in the footprint (board-relative) frame, else
    /// footprint positions +/- 10 mm; `None` with neither.
    pub fn get_board_bounds(pcb: &Pcb) -> Option<Rectangle> {
        let (ox, oy) = pcb.board_origin();
        let (mut x0, mut y0, mut x1, mut y1) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        let mut found = false;
        let mut take = |a: (f64, f64), b: (f64, f64)| {
            x0 = x0.min(a.0 - ox).min(b.0 - ox);
            y0 = y0.min(a.1 - oy).min(b.1 - oy);
            x1 = x1.max(a.0 - ox).max(b.0 - ox);
            y1 = y1.max(a.1 - oy).max(b.1 - oy);
        };
        for item in pcb.graphic_items() {
            let (layer, start, end) = match item {
                GraphicItem::Line(l) => (&l.layer, l.start, l.end),
                GraphicItem::Arc(a) => (&a.layer, a.start, a.end),
                GraphicItem::Other(g) => (&g.layer, g.start, g.end),
            };
            if layer.contains("Edge") {
                found = true;
                take(start, end);
            }
        }
        if !found {
            for fp in pcb.footprints() {
                let (x, y) = fp.position;
                x0 = x0.min(x - 10.0);
                y0 = y0.min(y - 10.0);
                x1 = x1.max(x + 10.0);
                y1 = y1.max(y + 10.0);
            }
            if x0 == f64::INFINITY {
                return None;
            }
        }
        Some(Rectangle::new(x0, y0, x1, y1))
    }

    pub fn position_within_bounds(x: f64, y: f64, b: &Rectangle) -> bool {
        let m = Self::BOARD_EDGE_MARGIN;
        b.min_x + m <= x && x <= b.max_x - m && b.min_y + m <= y && y <= b.max_y - m
    }
}

fn find_child_mut<'a>(node: &'a mut SExp, tag: &str) -> Option<&'a mut SExp> {
    node.children.iter_mut().find(|c| c.has_tag(tag))
}

/// Y-mirror + side-swap one footprint text/graphic node.
pub fn mirror_cosmetic_node(node: &mut SExp) {
    let tag = node.tag().unwrap_or("").to_string();
    let is_text = matches!(tag.as_str(), "property" | "fp_text" | "fp_text_box");
    if let Some(at) = find_child_mut(node, "at") {
        if let Some(y) = at.float_at(1) {
            at.set_value(1, Value::Float(-y));
        }
        match at.float_at(2) {
            Some(angle) => at.set_value(2, Value::Float((180.0 - angle).rem_euclid(360.0))),
            None if is_text => {
                at.push(SExp::atom(Value::Float(180.0)));
            }
            None => {}
        }
    }
    for t in ["start", "end", "center", "mid"] {
        if let Some(g) = find_child_mut(node, t) {
            if let Some(y) = g.float_at(1) {
                g.set_value(1, Value::Float(-y));
            }
        }
    }
    if tag == "fp_arc" && !node.children.iter().any(|c| c.has_tag("mid")) {
        if let Some(an) = find_child_mut(node, "angle") {
            if let Some(a) = an.float_at(0) {
                an.set_value(0, Value::Float(-a));
            }
        }
    }
    if let Some(pts) = find_child_mut(node, "pts") {
        for xy in pts.children.iter_mut().filter(|c| c.has_tag("xy")) {
            if let Some(y) = xy.float_at(1) {
                xy.set_value(1, Value::Float(-y));
            }
        }
    }
    if let Some(layer) = find_child_mut(node, "layer") {
        if let Some(name) = layer.text_at(0).filter(|n| !n.is_empty()) {
            layer.children[0] = SExp::quoted(flip_layer_side(&name));
        }
    }
    if is_text {
        if let Some(effects) = find_child_mut(node, "effects") {
            let is_mirror = |c: &SExp| {
                c.is_atom()
                    && c.value
                        .as_ref()
                        .is_some_and(|v| v.as_str() == Some("mirror"))
            };
            match effects.children.iter().position(|c| c.has_tag("justify")) {
                None => {
                    effects.push(SExp::list("justify", [SExp::atom("mirror")]));
                }
                Some(ji) => {
                    let justify = &mut effects.children[ji];
                    if justify.children.iter().any(is_mirror) {
                        justify.children.retain(|c| !is_mirror(c));
                        if justify.children.is_empty() {
                            effects.children.remove(ji);
                        }
                    } else {
                        justify.push(SExp::atom("mirror"));
                    }
                }
            }
        }
    }
}
