//! Port of the routed-copper primitives from `kicad_tools.router.primitives`:
//! [`Segment`], [`Via`], [`Route`] and their KiCad serializers.
//!
//! Grid/pad/obstacle types are ported with the router core. Deterministic
//! UUID mode (`enable_deterministic_uuids`, which draws from Python's seeded
//! `random`) is not reproducible in Rust; UUIDs are always v4.

use std::cell::Cell;

use super::layers::Layer;
use super::quantize::{verify_segment_45, OffAngleSegmentError};
use crate::pyjson::{py_float_repr, py_round};

thread_local! {
    static ENFORCE_SEGMENT_45: Cell<bool> = const { Cell::new(true) };
}

/// Toggle 45-degree enforcement in [`Segment::to_sexp`] (issue #3907).
pub fn enable_segment_45_enforcement(enabled: bool) {
    ENFORCE_SEGMENT_45.with(|c| c.set(enabled));
}

pub fn is_segment_45_enforcement_enabled() -> bool {
    ENFORCE_SEGMENT_45.with(Cell::get)
}

/// Run `f` with 45-degree enforcement disabled, restoring the prior state.
pub fn segment_45_enforcement_disabled<T>(f: impl FnOnce() -> T) -> T {
    let prior = is_segment_45_enforcement_enabled();
    enable_segment_45_enforcement(false);
    let out = f();
    enable_segment_45_enforcement(prior);
    out
}

/// Fresh UUID string (`_make_uuid`).
pub fn make_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `_fmt`: round to 4 decimals, render integral values without a fraction.
pub fn fmt_num(val: f64) -> String {
    let rounded = py_round(val, 4);
    if rounded == rounded.trunc() && rounded.is_finite() {
        format!("{}", rounded as i64)
    } else {
        py_float_repr(rounded)
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
    pub in_pad: bool,
    pub is_micro: bool,
}

impl Via {
    pub fn new(x: f64, y: f64, drill: f64, diameter: f64, layers: (Layer, Layer), net: i64) -> Self {
        Via {
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

    /// KiCad `(via ...)` text (uuid before net, like KiCad's writer).
    pub fn to_sexp(&self, name_only: bool) -> String {
        let type_token = if self.is_micro { " micro" } else { "" };
        let net_ref = if name_only && !self.net_name.is_empty() {
            format!("\"{}\"", self.net_name)
        } else {
            self.net.to_string()
        };
        format!(
            "(via{type_token}\n\t\t(at {:.4} {:.4})\n\t\t(size {})\n\t\t(drill {})\n\t\t(layers \"{}\" \"{}\")\n\t\t(uuid \"{}\")\n\t\t(net {net_ref})\n\t)",
            self.x,
            self.y,
            fmt_num(self.diameter),
            fmt_num(self.drill),
            self.layers.0.kicad_name(),
            self.layers.1.kicad_name(),
            make_uuid()
        )
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        width: f64,
        layer: Layer,
        net: i64,
        net_name: &str,
    ) -> Self {
        Segment {
            x1,
            y1,
            x2,
            y2,
            width,
            layer,
            net,
            net_name: net_name.to_string(),
        }
    }

    /// Copy of this segment's width/layer/net with new endpoints.
    pub fn with_points(&self, x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Segment {
            x1,
            y1,
            x2,
            y2,
            width: self.width,
            layer: self.layer,
            net: self.net,
            net_name: self.net_name.clone(),
        }
    }

    pub fn start(&self) -> (f64, f64) {
        (self.x1, self.y1)
    }

    pub fn end(&self) -> (f64, f64) {
        (self.x2, self.y2)
    }

    /// KiCad `(segment ...)` text. Fails only in strict 45-degree mode.
    pub fn to_sexp(&self, name_only: bool) -> Result<String, OffAngleSegmentError> {
        if is_segment_45_enforcement_enabled() {
            verify_segment_45(
                self.x1,
                self.y1,
                self.x2,
                self.y2,
                &format!("net {} on {}", self.net, self.layer.kicad_name()),
                None,
            )?;
        }
        let net_ref = if name_only && !self.net_name.is_empty() {
            format!("\"{}\"", self.net_name)
        } else {
            self.net.to_string()
        };
        Ok(format!(
            "(segment\n\t\t(start {:.4} {:.4})\n\t\t(end {:.4} {:.4})\n\t\t(width {})\n\t\t(layer \"{}\")\n\t\t(uuid \"{}\")\n\t\t(net {net_ref})\n\t)",
            self.x1,
            self.y1,
            self.x2,
            self.y2,
            fmt_num(self.width),
            self.layer.kicad_name(),
            make_uuid()
        ))
    }
}

/// A complete route between two points.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Route {
    pub net: i64,
    pub net_name: String,
    pub segments: Vec<Segment>,
    pub vias: Vec<Via>,
    pub is_escape: bool,
}

impl Route {
    pub fn new(net: i64, net_name: &str, segments: Vec<Segment>, vias: Vec<Via>) -> Self {
        Route {
            net,
            net_name: net_name.to_string(),
            segments,
            vias,
            is_escape: false,
        }
    }

    /// All segment and via S-expressions joined by `"\n\t"`.
    pub fn to_sexp(&self, name_only: bool) -> Result<String, OffAngleSegmentError> {
        let mut parts = Vec::new();
        for seg in &self.segments {
            parts.push(seg.to_sexp(name_only)?);
        }
        for via in &self.vias {
            parts.push(via.to_sexp(name_only));
        }
        Ok(parts.join("\n\t"))
    }

    /// Field-level geometric snapshot.
    pub fn copy_geometry(&self) -> Route {
        self.clone()
    }

    /// Insert missing vias at layer transitions; returns how many were added.
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
                    let mut via = Via::new(
                        tx,
                        ty,
                        via_drill,
                        via_diameter,
                        (Layer::FCu, Layer::BCu),
                        self.net,
                    );
                    via.net_name = self.net_name.clone();
                    self.vias.push(via);
                    inserted += 1;
                }
            }
        }
        inserted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_matches_python() {
        assert_eq!(fmt_num(0.25), "0.25");
        assert_eq!(fmt_num(1.0), "1");
        assert_eq!(fmt_num(0.123456), "0.1235");
    }

    #[test]
    fn segment_sexp_shape() {
        let s = Segment::new(0.0, 0.0, 1.5, 0.0, 0.2, Layer::FCu, 3, "GND");
        let text = s.to_sexp(false).unwrap();
        assert!(text.starts_with("(segment\n\t\t(start 0.0000 0.0000)\n\t\t(end 1.5000 0.0000)\n\t\t(width 0.2)\n\t\t(layer \"F.Cu\")\n\t\t(uuid \""));
        assert!(text.ends_with("\t\t(net 3)\n\t)"));
        assert!(s.to_sexp(true).unwrap().contains("(net \"GND\")"));
    }
}
