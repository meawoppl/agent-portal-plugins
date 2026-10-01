//! Basic data structures for PCB routing (port of
//! `kicad_tools.router.primitives`).
//!
//! - [`Point`]: 3D coordinate in routing space
//! - [`GridCell`]: routing-grid cell with congestion tracking
//! - [`Via`], [`Segment`], [`Route`]: emitted copper
//! - [`Pad`]: component pad to connect
//! - [`Obstacle`]: area to avoid during routing

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

use super::layers::Layer;
use super::pyrandom;
use super::quantize;
use crate::physics::py_round;
use crate::utils::pyrepr::py_float_repr;

// Issue #3272: deterministic UUID mode (UUIDs derived from the seeded
// global `random` instead of OS entropy).
static DETERMINISTIC_UUIDS: AtomicBool = AtomicBool::new(false);
// Issue #3907: by-construction 45-degree enforcement in `Segment::to_sexp`.
static ENFORCE_SEGMENT_45: AtomicBool = AtomicBool::new(true);

/// Toggle deterministic UUID emission for [`Segment`] and [`Via`].
pub fn enable_deterministic_uuids(enabled: bool) {
    DETERMINISTIC_UUIDS.store(enabled, Ordering::SeqCst);
}

/// Restore default (non-deterministic) UUID emission.
pub fn reset_deterministic_uuids() {
    DETERMINISTIC_UUIDS.store(false, Ordering::SeqCst);
}

pub fn is_deterministic_uuids_enabled() -> bool {
    DETERMINISTIC_UUIDS.load(Ordering::SeqCst)
}

/// Toggle 45-degree serialization enforcement (issue #3907).
pub fn enable_segment_45_enforcement(enabled: bool) {
    ENFORCE_SEGMENT_45.store(enabled, Ordering::SeqCst);
}

pub fn is_segment_45_enforcement_enabled() -> bool {
    ENFORCE_SEGMENT_45.load(Ordering::SeqCst)
}

/// Scoped opt-out of 45-degree enforcement; restores the prior state on drop.
pub struct Segment45EnforcementDisabled {
    prior: bool,
}

impl Drop for Segment45EnforcementDisabled {
    fn drop(&mut self) {
        ENFORCE_SEGMENT_45.store(self.prior, Ordering::SeqCst);
    }
}

/// `with segment_45_enforcement_disabled(): ...`
pub fn segment_45_enforcement_disabled() -> Segment45EnforcementDisabled {
    let prior = ENFORCE_SEGMENT_45.swap(false, Ordering::SeqCst);
    Segment45EnforcementDisabled { prior }
}

/// `str(uuid.UUID(int=bits, version=4))`.
pub fn uuid_v4_from_int(bits: u128) -> String {
    let mut v = bits;
    // version 4: clear bits 76..80, set 0100; variant RFC 4122: 10xx at bits 62..64.
    v &= !(0xf000u128 << 64);
    v |= 0x4000u128 << 64;
    v &= !(0xc000u128 << 48);
    v |= 0x8000u128 << 48;
    uuid::Uuid::from_u128(v).hyphenated().to_string()
}

/// UUID honoring the deterministic toggle.
pub fn make_uuid() -> String {
    if is_deterministic_uuids_enabled() {
        let bits = pyrandom::with_global(|r| r.getrandbits(128));
        uuid_v4_from_int(bits)
    } else {
        uuid::Uuid::new_v4().hyphenated().to_string()
    }
}

/// Upstream `_fmt`: round to 4 decimals, print integral values as ints.
pub fn fmt_num(val: f64) -> String {
    let rounded = py_round(val, 4);
    if rounded == rounded.trunc() && rounded.abs() < 1e15 {
        format!("{}", rounded as i64)
    } else {
        py_float_repr(rounded)
    }
}

/// A point in 3D routing space (x, y, layer). Equality/hash use 4-dp
/// rounding like upstream.
#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub layer: Layer,
}

impl Point {
    pub fn new(x: f64, y: f64, layer: Layer) -> Self {
        Self { x, y, layer }
    }

    pub fn grid_key(&self, resolution: f64) -> (i64, i64, u8) {
        (
            py_round_i(self.x / resolution),
            py_round_i(self.y / resolution),
            self.layer.value(),
        )
    }

    /// Manhattan distance (same layer) plus a via estimate across layers.
    pub fn distance_to(&self, other: &Point) -> f64 {
        let d = (self.x - other.x).abs() + (self.y - other.y).abs();
        if self.layer == other.layer {
            d
        } else {
            d + (self.layer.value() as f64 - other.layer.value() as f64).abs() * 0.5
        }
    }

    fn key(&self) -> (u64, u64, u8) {
        (
            py_round(self.x, 4).to_bits(),
            py_round(self.y, 4).to_bits(),
            self.layer.value(),
        )
    }
}

impl PartialEq for Point {
    fn eq(&self, other: &Self) -> bool {
        py_round(self.x, 4) == py_round(other.x, 4)
            && py_round(self.y, 4) == py_round(other.y, 4)
            && self.layer == other.layer
    }
}

impl Eq for Point {}

impl Hash for Point {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

/// Python `round(x)` to an integer (banker's rounding).
pub fn py_round_i(x: f64) -> i64 {
    let r = x.round_ties_even();
    r as i64
}

/// A cell in the routing grid with negotiated congestion support.
#[derive(Debug, Clone, PartialEq)]
pub struct GridCell {
    pub x: i64,
    pub y: i64,
    pub layer: i64,
    pub blocked: bool,
    /// 0 = empty, >0 = assigned to net.
    pub net: i64,
    pub cost: f64,
    pub usage_count: i64,
    pub history_cost: f64,
    pub is_obstacle: bool,
    pub is_zone: bool,
    pub zone_id: Option<String>,
    pub pad_blocked: bool,
    pub original_net: i64,
}

impl GridCell {
    pub fn new(x: i64, y: i64, layer: i64) -> Self {
        Self {
            x,
            y,
            layer,
            blocked: false,
            net: 0,
            cost: 1.0,
            usage_count: 0,
            history_cost: 0.0,
            is_obstacle: false,
            is_zone: false,
            zone_id: None,
            pad_blocked: false,
            original_net: 0,
        }
    }
}

/// A via connecting layers.
#[derive(Debug, Clone, PartialEq)]
pub struct Via {
    pub x: f64,
    pub y: f64,
    pub drill: f64,
    pub diameter: f64,
    pub layers: (Layer, Layer),
    pub net: i64,
    pub net_name: String,
    /// Issue #2605: in-pad escape via (pad copper is the annular ring).
    pub in_pad: bool,
    /// Issue #3124: serialize as `(via micro ...)`.
    pub is_micro: bool,
}

impl Via {
    pub fn new(
        x: f64,
        y: f64,
        drill: f64,
        diameter: f64,
        layers: (Layer, Layer),
        net: i64,
    ) -> Self {
        Self {
            x,
            y,
            drill,
            diameter,
            layers,
            net,
            net_name: String::new(),
            in_pad: false,
            is_micro: false,
        }
    }

    pub fn with_net_name(mut self, name: &str) -> Self {
        self.net_name = name.to_string();
        self
    }

    /// KiCad S-expression (uuid before net, issue #3925; name-only net
    /// dialect when `name_only`, issue #4416).
    pub fn to_sexp(&self, name_only: bool) -> String {
        let type_token = if self.is_micro { " micro" } else { "" };
        let net_ref = net_ref(self.net, &self.net_name, name_only);
        format!(
            "(via{type_token}\n\t\t(at {:.4} {:.4})\n\t\t(size {})\n\t\t(drill {})\n\t\t(layers \"{}\" \"{}\")\n\t\t(uuid \"{}\")\n\t\t(net {net_ref})\n\t)",
            self.x,
            self.y,
            fmt_num(self.diameter),
            fmt_num(self.drill),
            self.layers.0.kicad_name(),
            self.layers.1.kicad_name(),
            make_uuid(),
        )
    }
}

fn net_ref(net: i64, net_name: &str, name_only: bool) -> String {
    if name_only && !net_name.is_empty() {
        format!("\"{net_name}\"")
    } else {
        net.to_string()
    }
}

/// A trace segment.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub width: f64,
    pub layer: Layer,
    pub net: i64,
    pub net_name: String,
}

impl Segment {
    pub fn new(x1: f64, y1: f64, x2: f64, y2: f64, width: f64, layer: Layer, net: i64) -> Self {
        Self {
            x1,
            y1,
            x2,
            y2,
            width,
            layer,
            net,
            net_name: String::new(),
        }
    }

    pub fn with_net_name(mut self, name: &str) -> Self {
        self.net_name = name.to_string();
        self
    }

    pub fn start(&self) -> (f64, f64) {
        (self.x1, self.y1)
    }

    pub fn end(&self) -> (f64, f64) {
        (self.x2, self.y2)
    }

    pub fn length(&self) -> f64 {
        (self.x2 - self.x1).hypot(self.y2 - self.y1)
    }

    /// KiCad S-expression. Off-angle displacements go through
    /// [`quantize::verify_segment_45`] (warn by default, error in strict
    /// mode); the strict error is returned as `Err`.
    pub fn try_to_sexp(&self, name_only: bool) -> Result<String, quantize::OffAngleSegmentError> {
        if is_segment_45_enforcement_enabled() {
            quantize::verify_segment_45(
                self.x1,
                self.y1,
                self.x2,
                self.y2,
                quantize::ANGLE_TOL_DEG,
                &format!("net {} on {}", self.net, self.layer.kicad_name()),
                None,
            )?;
        }
        let net_ref = net_ref(self.net, &self.net_name, name_only);
        Ok(format!(
            "(segment\n\t\t(start {:.4} {:.4})\n\t\t(end {:.4} {:.4})\n\t\t(width {})\n\t\t(layer \"{}\")\n\t\t(uuid \"{}\")\n\t\t(net {net_ref})\n\t)",
            self.x1,
            self.y1,
            self.x2,
            self.y2,
            fmt_num(self.width),
            self.layer.kicad_name(),
            make_uuid(),
        ))
    }

    /// KiCad S-expression; strict-mode off-angle copper is an error
    /// (upstream raises `OffAngleSegmentError`).
    pub fn to_sexp(&self, name_only: bool) -> Result<String, quantize::OffAngleSegmentError> {
        self.try_to_sexp(name_only)
    }
}

/// A complete route between two points.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Route {
    pub net: i64,
    pub net_name: String,
    pub segments: Vec<Segment>,
    pub vias: Vec<Via>,
    /// Issue #3441: sub-grid escape stub, not a full net route.
    pub is_escape: bool,
}

impl Route {
    pub fn new(net: i64, net_name: &str) -> Self {
        Self {
            net,
            net_name: net_name.to_string(),
            ..Default::default()
        }
    }

    pub fn try_to_sexp(&self, name_only: bool) -> Result<String, quantize::OffAngleSegmentError> {
        let mut parts = Vec::with_capacity(self.segments.len() + self.vias.len());
        for seg in &self.segments {
            parts.push(seg.try_to_sexp(name_only)?);
        }
        for via in &self.vias {
            parts.push(via.to_sexp(name_only));
        }
        Ok(parts.join("\n\t"))
    }

    /// All segment/via S-expressions; fails on strict off-angle copper.
    pub fn to_sexp(&self, name_only: bool) -> Result<String, quantize::OffAngleSegmentError> {
        self.try_to_sexp(name_only)
    }

    /// Geometric snapshot (issue #3507).
    pub fn copy_geometry(&self) -> Route {
        self.clone()
    }

    /// Insert missing vias at layer transitions; returns count inserted.
    pub fn validate_layer_transitions(&mut self, via_drill: f64, via_diameter: f64) -> usize {
        if self.segments.len() < 2 {
            return 0;
        }
        let mut inserted = 0;
        for i in 0..self.segments.len() - 1 {
            let (s1, s2) = (&self.segments[i], &self.segments[i + 1]);
            if s1.layer != s2.layer {
                let (tx, ty) = (s1.x2, s1.y2);
                let has_via = self
                    .vias
                    .iter()
                    .any(|v| (v.x - tx).abs() < 0.01 && (v.y - ty).abs() < 0.01);
                if !has_via {
                    let via = Via::new(
                        tx,
                        ty,
                        via_drill,
                        via_diameter,
                        (Layer::FCu, Layer::BCu),
                        self.net,
                    )
                    .with_net_name(&self.net_name);
                    self.vias.push(via);
                    inserted += 1;
                }
            }
        }
        inserted
    }

    pub fn total_length(&self) -> f64 {
        self.segments.iter().map(Segment::length).sum()
    }
}

/// Supported routing pad shapes.
pub const PAD_SHAPES: &[&str] = &["circle", "rect", "oval", "roundrect"];

/// A pad to connect.
#[derive(Debug, Clone, PartialEq)]
pub struct Pad {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub net: i64,
    pub net_name: String,
    pub layer: Layer,
    /// Component reference.
    pub r#ref: String,
    /// Pin number/name.
    pub pin: String,
    /// PTH pads block every layer.
    pub through_hole: bool,
    pub drill: f64,
    /// Virtual Steiner tree branch point.
    pub steiner_point: bool,
    pub footprint_name: String,
    /// Residual board-space rotation in degrees (issue #4910).
    pub rotation: f64,
    pub shape: String,
    pub drill_size: Option<(f64, f64)>,
    pub drill_rotation: f64,
    pub component_id: String,
    pub escape_terminal: bool,
    pub terminal_id: String,
}

impl Default for Pad {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            net: 0,
            net_name: String::new(),
            layer: Layer::FCu,
            r#ref: String::new(),
            pin: String::new(),
            through_hole: false,
            drill: 0.0,
            steiner_point: false,
            footprint_name: String::new(),
            rotation: 0.0,
            shape: "rect".into(),
            drill_size: None,
            drill_rotation: 0.0,
            component_id: String::new(),
            escape_terminal: false,
            terminal_id: String::new(),
        }
    }
}

impl Pad {
    /// Required-field constructor (`Pad(x, y, width, height, net, net_name)`).
    pub fn new(x: f64, y: f64, width: f64, height: f64, net: i64, net_name: &str) -> Self {
        Self {
            x,
            y,
            width,
            height,
            net,
            net_name: net_name.to_string(),
            ..Default::default()
        }
    }

    /// `__post_init__` shape validation.
    pub fn validate(&self) -> Result<(), String> {
        if PAD_SHAPES.contains(&self.shape.as_str()) {
            Ok(())
        } else {
            Err(format!(
                "Unsupported routing pad shape {} for {}.{}",
                crate::utils::pyrepr::py_str_repr(&self.shape),
                self.r#ref,
                self.pin
            ))
        }
    }

    /// Physical component key used for grouping and geometric exemptions.
    pub fn component_key(&self) -> &str {
        if self.component_id.is_empty() {
            &self.r#ref
        } else {
            &self.component_id
        }
    }

    /// Internal terminal key `(component_key, terminal_id or pin)`.
    pub fn key(&self) -> (String, String) {
        let t = if self.terminal_id.is_empty() {
            &self.pin
        } else {
            &self.terminal_id
        };
        (self.component_key().to_string(), t.clone())
    }
}

/// Board-space `(half_width, half_height)` AABB extent of `pad` (#4910).
pub fn pad_half_extents(pad: &Pad) -> (f64, f64) {
    if pad.shape == "circle" {
        let r = pad.width.max(pad.height) / 2.0;
        return (r, r);
    }
    if pad.rotation == 0.0 {
        return (pad.width / 2.0, pad.height / 2.0);
    }
    let theta = pad.rotation.to_radians();
    let (c, s) = (theta.cos().abs(), theta.sin().abs());
    (
        c * pad.width / 2.0 + s * pad.height / 2.0,
        s * pad.width / 2.0 + c * pad.height / 2.0,
    )
}

/// An obstacle to avoid.
#[derive(Debug, Clone, PartialEq)]
pub struct Obstacle {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub layer: Layer,
    pub clearance: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_matches_python() {
        assert_eq!(fmt_num(0.2), "0.2");
        assert_eq!(fmt_num(1.0), "1");
        assert_eq!(fmt_num(0.60004), "0.6");
        assert_eq!(fmt_num(0.12345), "0.1235");
    }

    #[test]
    fn segment_sexp_shape() {
        let s = Segment::new(1.0, 2.0, 3.0, 2.0, 0.2, Layer::FCu, 3).with_net_name("A");
        let txt = s.to_sexp(false).unwrap();
        assert!(txt.starts_with("(segment\n\t\t(start 1.0000 2.0000)\n\t\t(end 3.0000 2.0000)\n\t\t(width 0.2)\n\t\t(layer \"F.Cu\")\n\t\t(uuid \""));
        assert!(txt.ends_with("(net 3)\n\t)"));
        assert!(s.to_sexp(true).unwrap().ends_with("(net \"A\")\n\t)"));
    }

    #[test]
    fn via_sexp_and_micro() {
        let mut v = Via::new(1.0, 1.0, 0.3, 0.6, (Layer::FCu, Layer::BCu), 2);
        v.is_micro = true;
        let t = v.to_sexp(false);
        assert!(t.starts_with("(via micro\n\t\t(at 1.0000 1.0000)\n\t\t(size 0.6)\n\t\t(drill 0.3)\n\t\t(layers \"F.Cu\" \"B.Cu\")"));
    }

    #[test]
    fn point_eq_hash_round() {
        let a = Point::new(1.00001, 2.0, Layer::FCu);
        let b = Point::new(1.00002, 2.0, Layer::FCu);
        assert_eq!(a, b);
        assert_eq!(a.grid_key(0.1), (10, 20, 0));
        assert_eq!(
            Point::new(0.0, 0.0, Layer::FCu).distance_to(&Point::new(1.0, 1.0, Layer::BCu)),
            4.5
        );
    }

    #[test]
    fn layer_transition_inserts_via() {
        let mut r = Route::new(1, "N");
        r.segments
            .push(Segment::new(0.0, 0.0, 1.0, 0.0, 0.2, Layer::FCu, 1));
        r.segments
            .push(Segment::new(1.0, 0.0, 2.0, 0.0, 0.2, Layer::BCu, 1));
        assert_eq!(r.validate_layer_transitions(0.35, 0.7), 1);
        assert_eq!(r.validate_layer_transitions(0.35, 0.7), 0);
    }

    #[test]
    fn half_extents() {
        let mut p = Pad::new(0.0, 0.0, 2.0, 1.0, 1, "N");
        assert_eq!(pad_half_extents(&p), (1.0, 0.5));
        p.rotation = 90.0;
        let (w, h) = pad_half_extents(&p);
        assert!((w - 0.5).abs() < 1e-12 && (h - 1.0).abs() < 1e-12);
        p.shape = "circle".into();
        assert_eq!(pad_half_extents(&p), (1.0, 1.0));
        p.shape = "custom".into();
        assert!(p.validate().is_err());
    }
}
