//! Port of the 45-degree helpers from `kicad_tools.router.quantize`.
//!
//! Only the construction-time pieces the optimizer needs are here
//! ([`dogleg_points`], [`snap_direction_8`], [`verify_segment_45`]); the
//! file-level `quantize_pcb_file` / census pass is ported with the router.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;

use crate::pyjson::py_round;

/// Degrees off the nearest multiple of 45 below which a segment counts as
/// 45-aligned.
pub const ANGLE_TOL_DEG: f64 = 0.01;

/// One unit of the 4-decimal serialization grid (`Segment::to_sexp`).
pub const SERIALIZE_QUANTUM_MM: f64 = 1e-4;

/// Environment variable that turns the off-angle warning into an error.
pub const SEGMENT_45_STRICT_ENV: &str = "KICAD_TOOLS_SEGMENT_45_STRICT";

const DIAG: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// The 8 legal routing directions (unit vectors).
pub const EIGHT_DIRECTIONS: [(f64, f64); 8] = [
    (1.0, 0.0),
    (DIAG, DIAG),
    (0.0, 1.0),
    (-DIAG, DIAG),
    (-1.0, 0.0),
    (-DIAG, -DIAG),
    (0.0, -1.0),
    (DIAG, -DIAG),
];

/// Degrees off the nearest multiple of 45 for displacement `(dx, dy)`.
pub fn off_angle_degrees(dx: f64, dy: f64) -> f64 {
    if dx == 0.0 && dy == 0.0 {
        return 0.0;
    }
    let ang = dy.atan2(dx).to_degrees().rem_euclid(45.0);
    ang.min(45.0 - ang)
}

/// True if `(dx, dy)` lies on the 0/45/90/135 angle set.
pub fn is_45_aligned(dx: f64, dy: f64, tol_deg: f64) -> bool {
    off_angle_degrees(dx, dy) <= tol_deg
}

/// Snap `(dx, dy)` to the nearest of the 8 routing directions.
pub fn snap_direction_8(dx: f64, dy: f64) -> (f64, f64) {
    if dx == 0.0 && dy == 0.0 {
        return EIGHT_DIRECTIONS[0];
    }
    let ang = dy.atan2(dx);
    let idx = ((ang / (std::f64::consts::PI / 4.0)).round_ties_even() as i64).rem_euclid(8);
    EIGHT_DIRECTIONS[idx as usize]
}

/// Polyline from `(x1, y1)` to `(x2, y2)` using only 45-aligned legs.
pub fn dogleg_points(x1: f64, y1: f64, x2: f64, y2: f64, axis_first: bool) -> Vec<(f64, f64)> {
    dogleg_points_tol(x1, y1, x2, y2, axis_first, ANGLE_TOL_DEG)
}

/// [`dogleg_points`] with an explicit angle tolerance.
pub fn dogleg_points_tol(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    axis_first: bool,
    tol_deg: f64,
) -> Vec<(f64, f64)> {
    let dx = x2 - x1;
    let dy = y2 - y1;
    if is_45_aligned(dx, dy, tol_deg) {
        return vec![(x1, y1), (x2, y2)];
    }
    let adx = dx.abs();
    let ady = dy.abs();
    let mid = if !axis_first {
        if adx >= ady {
            (x1 + ady.copysign(dx), y2)
        } else {
            (x2, y1 + adx.copysign(dy))
        }
    } else if adx >= ady {
        (x2 - ady.copysign(dx), y1)
    } else {
        (x1, y2 - adx.copysign(dy))
    };
    vec![(x1, y1), mid, (x2, y2)]
}

/// Raised (strict mode only) when a serialized segment is off-angle.
#[derive(Debug, Clone)]
pub struct OffAngleSegmentError {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub off_deg: f64,
    pub context: String,
}

impl fmt::Display for OffAngleSegmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let where_ = if self.context.is_empty() {
            String::new()
        } else {
            format!(" [{}]", self.context)
        };
        write!(
            f,
            "off-angle segment{where_}: ({:.4}, {:.4}) -> ({:.4}, {:.4}) is {:.4} deg off the \
             0/45/90/135 set (tol {ANGLE_TOL_DEG} deg).  Emit a dogleg \
             (kicad_tools.router.quantize.dogleg_points) at construction time instead of \
             serializing skewed copper (issue #3907).",
            self.x1, self.y1, self.x2, self.y2, self.off_deg
        )
    }
}

impl std::error::Error for OffAngleSegmentError {}

/// Whether [`SEGMENT_45_STRICT_ENV`] requests strict mode.
pub fn segment_45_strict_enabled() -> bool {
    let val = std::env::var(SEGMENT_45_STRICT_ENV).unwrap_or_default();
    matches!(val.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

fn is_quantum_aligned(dx: f64, dy: f64) -> bool {
    let adx = dx.abs();
    let ady = dy.abs();
    let q = SERIALIZE_QUANTUM_MM * 1.5;
    adx <= q || ady <= q || (adx - ady).abs() <= q
}

thread_local! {
    /// Python's default warning filter prints each distinct message once.
    static WARNED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Check that a segment is 45-legal as serialized (4 decimals).
///
/// Strict mode (`strict = Some(true)` or the env switch) returns an error;
/// otherwise an `OffAngleSegmentWarning` is printed to stderr (once per
/// distinct message, like Python's `warnings`) and `Ok` is returned.
pub fn verify_segment_45(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    context: &str,
    strict: Option<bool>,
) -> Result<(), OffAngleSegmentError> {
    let tol_deg = ANGLE_TOL_DEG;
    let (sx1, sy1) = (py_round(x1, 4), py_round(y1, 4));
    let (sx2, sy2) = (py_round(x2, 4), py_round(y2, 4));
    let dx = sx2 - sx1;
    let dy = sy2 - sy1;
    if dx == 0.0 && dy == 0.0 {
        return Ok(());
    }
    if is_quantum_aligned(dx, dy) {
        return Ok(());
    }
    let off = off_angle_degrees(dx, dy);
    if off <= tol_deg {
        return Ok(());
    }
    if strict.unwrap_or_else(segment_45_strict_enabled) {
        return Err(OffAngleSegmentError {
            x1: sx1,
            y1: sy1,
            x2: sx2,
            y2: sy2,
            off_deg: off,
            context: context.to_string(),
        });
    }
    let where_ = if context.is_empty() {
        String::new()
    } else {
        format!(" [{context}]")
    };
    let msg = format!(
        "off-angle segment{where_}: ({sx1:.4}, {sy1:.4}) -> ({sx2:.4}, {sy2:.4}) is {off:.4} deg \
         off the 0/45/90/135 set (tol {tol_deg} deg).  Serializing as-is for the legacy \
         quantize_pcb_file fallback to repair; migrate this emitter to a by-construction \
         dogleg (issue #3907).  Set {SEGMENT_45_STRICT_ENV}=1 to make this a hard error."
    );
    let fresh = WARNED.with(|w| w.borrow_mut().insert(msg.clone()));
    if fresh {
        eprintln!("OffAngleSegmentWarning: {msg}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dogleg_inserts_one_vertex() {
        assert_eq!(dogleg_points(0.0, 0.0, 1.0, 0.0, false).len(), 2);
        let pts = dogleg_points(0.0, 0.0, 3.0, 1.0, false);
        assert_eq!(pts, vec![(0.0, 0.0), (1.0, 1.0), (3.0, 1.0)]);
        let pts = dogleg_points(0.0, 0.0, 3.0, 1.0, true);
        assert_eq!(pts, vec![(0.0, 0.0), (2.0, 0.0), (3.0, 1.0)]);
    }

    #[test]
    fn verify_accepts_quantum_jitter() {
        assert!(verify_segment_45(0.0, 0.0, 0.1385, 0.1384, "", Some(true)).is_ok());
        assert!(verify_segment_45(0.0, 0.0, 3.0, 1.0, "", Some(true)).is_err());
    }

    #[test]
    fn snap_direction() {
        assert_eq!(snap_direction_8(1.0, 0.1), (1.0, 0.0));
        assert_eq!(snap_direction_8(-1.0, -1.0), (-DIAG, -DIAG));
    }
}
