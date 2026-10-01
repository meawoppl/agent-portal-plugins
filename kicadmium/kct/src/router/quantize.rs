//! 45-degree quantization helpers for segment-emitting/mutating passes
//! (port of `kicad_tools.router.quantize`, issues #3532/#3907/#4529).
//!
//! When a displacement is not on the 8-direction set, emitters produce a
//! two-leg DOGLEG (one exact 45-degree leg plus one axis-aligned leg)
//! instead of a single skewed segment.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::physics::py_round;

/// Degrees off the nearest multiple of 45 below which a segment is aligned.
pub const ANGLE_TOL_DEG: f64 = 0.01;
/// One unit of the 4-decimal serialization grid.
pub const SERIALIZE_QUANTUM_MM: f64 = 1e-4;
/// Env var flipping the guard from WARN to raise.
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

/// Python float `%` (result has the sign of the divisor).
fn py_mod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && (r < 0.0) != (b < 0.0) {
        r + b
    } else {
        r
    }
}

/// Degrees off the nearest multiple of 45 for displacement (dx, dy).
pub fn off_angle_degrees(dx: f64, dy: f64) -> f64 {
    if dx == 0.0 && dy == 0.0 {
        return 0.0;
    }
    let ang = py_mod(dy.atan2(dx).to_degrees(), 45.0);
    ang.min(45.0 - ang)
}

/// True if (dx, dy) lies on the 0/45/90/135 angle set.
pub fn is_45_aligned(dx: f64, dy: f64, tol_deg: f64) -> bool {
    off_angle_degrees(dx, dy) <= tol_deg
}

/// Snap (dx, dy) to the nearest of the 8 routing directions.
pub fn snap_direction_8(dx: f64, dy: f64) -> (f64, f64) {
    if dx == 0.0 && dy == 0.0 {
        return EIGHT_DIRECTIONS[0];
    }
    let ang = dy.atan2(dx);
    let idx = (ang / (std::f64::consts::PI / 4.0)).round_ties_even() as i64;
    EIGHT_DIRECTIONS[idx.rem_euclid(8) as usize]
}

fn copysign(mag: f64, like: f64) -> f64 {
    mag.abs().copysign(like)
}

/// Polyline vertices connecting (x1, y1) to (x2, y2) with only 45-aligned
/// legs (see upstream docstring for the exactness argument).
pub fn dogleg_points(
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
            (x1 + copysign(ady, dx), y2)
        } else {
            (x2, y1 + copysign(adx, dy))
        }
    } else if adx >= ady {
        (x2 - copysign(ady, dx), y1)
    } else {
        (x1, y2 - copysign(adx, dy))
    };
    vec![(x1, y1), mid, (x2, y2)]
}

/// `dogleg_points` with the default tolerance.
pub fn dogleg(x1: f64, y1: f64, x2: f64, y2: f64, axis_first: bool) -> Vec<(f64, f64)> {
    dogleg_points(x1, y1, x2, y2, axis_first, ANGLE_TOL_DEG)
}

/// A segment whose serialized displacement is off the 45-degree set
/// (strict mode only).
#[derive(Debug, Clone, PartialEq)]
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

/// True when the by-construction guard should raise, not just warn.
pub fn segment_45_strict_enabled() -> bool {
    std::env::var(SEGMENT_45_STRICT_ENV)
        .map(|v| {
            matches!(
                v.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// True if (dx, dy) is on the 45-degree set within one 4dp quantum.
pub fn is_quantum_aligned(dx: f64, dy: f64) -> bool {
    let adx = dx.abs();
    let ady = dy.abs();
    let q = SERIALIZE_QUANTUM_MM * 1.5;
    adx <= q || ady <= q || (adx - ady).abs() <= q
}

/// Check that segment (x1,y1)->(x2,y2) is 45-legal as written (4dp).
/// Off-angle copper warns on stderr by default and errors in strict mode.
pub fn verify_segment_45(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    tol_deg: f64,
    context: &str,
    strict: Option<bool>,
) -> Result<(), OffAngleSegmentError> {
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
    let strict = strict.unwrap_or_else(segment_45_strict_enabled);
    if strict {
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
    eprintln!(
        "OffAngleSegmentWarning: off-angle segment{where_}: ({sx1:.4}, {sy1:.4}) -> ({sx2:.4}, \
         {sy2:.4}) is {off:.4} deg off the 0/45/90/135 set (tol {tol_deg} deg).  Serializing \
         as-is for the legacy quantize_pcb_file fallback to repair; migrate this emitter to a \
         by-construction dogleg (issue #3907).  Set {SEGMENT_45_STRICT_ENV}=1 to make this a \
         hard error."
    );
    Ok(())
}

fn segment_block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r#"(?m)^([ \t]+)\(segment\s*\n"#,
            r#"\s*\(start ([-\d.]+) ([-\d.]+)\)\s*\n"#,
            r#"\s*\(end ([-\d.]+) ([-\d.]+)\)\s*\n"#,
            r#"\s*\(width ([-\d.]+)\)\s*\n"#,
            r#"\s*\(layer "([^"]+)"\)\s*\n"#,
            r#"(?:\s*\(uuid "([^"]+)"\)\s*\n)?"#,
            r#"\s*(?:\(net (\d+)\)|\(net "([^"]+)"\))\s*\n"#,
            r#"(?:\s*\(uuid "([^"]+)"\)\s*\n)?"#,
            r#"[ \t]*\)"#
        ))
        .expect("segment regex")
    })
}

fn uuid_attr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\(uuid "([^"]+)"\)"#).expect("uuid regex"))
}

fn net_header_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\(net (\d+) "([^"]*)"\)"#).expect("net regex"))
}

/// Net id -> name from the board's `(net N "name")` header table
/// (upstream `optimizer.pcb.parse_net_names`).
pub fn parse_net_names(text: &str) -> BTreeMap<i64, String> {
    let mut out = BTreeMap::new();
    for c in net_header_re().captures_iter(text) {
        if let Ok(id) = c[1].parse::<i64>() {
            out.insert(id, c[2].to_string());
        }
    }
    out
}

/// One off-angle census entry.
#[derive(Debug, Clone, PartialEq)]
pub struct OffAngleSegment {
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub layer: String,
    pub net: i64,
    pub uuid: Option<String>,
    pub off_deg: f64,
}

/// Angle census over every `(segment ...)` in PCB text.
pub fn segment_angle_census_text(text: &str, tol_deg: f64) -> (usize, Vec<OffAngleSegment>) {
    let name_to_id: std::collections::HashMap<String, i64> = parse_net_names(text)
        .into_iter()
        .map(|(id, name)| (name, id))
        .collect();
    let mut total = 0;
    let mut bad = Vec::new();
    for m in segment_block_re().captures_iter(text) {
        total += 1;
        let f = |i: usize| m[i].parse::<f64>().unwrap_or(0.0);
        let (x1, y1, x2, y2) = (f(2), f(3), f(4), f(5));
        if is_quantum_aligned(x2 - x1, y2 - y1) {
            continue;
        }
        let off = off_angle_degrees(x2 - x1, y2 - y1);
        if off > tol_deg {
            let net = match m.get(9) {
                Some(n) => n.as_str().parse().unwrap_or(0),
                None => m
                    .get(10)
                    .and_then(|n| name_to_id.get(n.as_str()).copied())
                    .unwrap_or(0),
            };
            bad.push(OffAngleSegment {
                start: (x1, y1),
                end: (x2, y2),
                layer: m[7].to_string(),
                net,
                uuid: m
                    .get(8)
                    .or_else(|| m.get(11))
                    .map(|u| u.as_str().to_string()),
                off_deg: off,
            });
        }
    }
    (total, bad)
}

/// Angle census over a PCB file.
pub fn segment_angle_census(
    pcb_path: &Path,
    tol_deg: f64,
) -> std::io::Result<(usize, Vec<OffAngleSegment>)> {
    let text = std::fs::read_to_string(pcb_path)?;
    Ok(segment_angle_census_text(&text, tol_deg))
}

// ---------------------------------------------------------------------------
// Exact decimal arithmetic for the file-level pass (Python `Decimal`).
// Coordinates are parsed as scaled integers so legs are exactly 45-aligned
// in the serialized text.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Dec {
    /// value * 10^SCALE
    v: i128,
}

const DEC_SCALE: u32 = 12;

impl Dec {
    fn parse(s: &str) -> Option<Dec> {
        let (neg, body) = match s.strip_prefix('-') {
            Some(b) => (true, b),
            None => (false, s),
        };
        let (int_part, frac_part) = match body.split_once('.') {
            Some((a, b)) => (a, b),
            None => (body, ""),
        };
        if frac_part.len() > DEC_SCALE as usize {
            return None;
        }
        let ip: i128 = if int_part.is_empty() {
            0
        } else {
            int_part.parse().ok()?
        };
        let mut fp: i128 = if frac_part.is_empty() {
            0
        } else {
            frac_part.parse().ok()?
        };
        fp *= 10i128.pow(DEC_SCALE - frac_part.len() as u32);
        let v = ip * 10i128.pow(DEC_SCALE) + fp;
        Some(Dec {
            v: if neg { -v } else { v },
        })
    }

    fn abs(self) -> Dec {
        Dec { v: self.v.abs() }
    }

    fn to_f64(self) -> f64 {
        self.v as f64 / 10f64.powi(DEC_SCALE as i32)
    }

    /// `_fmt_decimal`: plain notation, trailing zeros stripped, "-0" -> "0".
    fn fmt(self) -> String {
        let neg = self.v < 0;
        let a = self.v.abs();
        let scale = 10i128.pow(DEC_SCALE);
        let ip = a / scale;
        let fp = a % scale;
        let mut s = if fp == 0 {
            ip.to_string()
        } else {
            let frac = format!("{:0width$}", fp, width = DEC_SCALE as usize);
            format!("{ip}.{}", frac.trim_end_matches('0'))
        };
        if neg && s != "0" {
            s.insert(0, '-');
        }
        s
    }
}

impl std::ops::Add for Dec {
    type Output = Dec;
    fn add(self, o: Dec) -> Dec {
        Dec { v: self.v + o.v }
    }
}

impl std::ops::Sub for Dec {
    type Output = Dec;
    fn sub(self, o: Dec) -> Dec {
        Dec { v: self.v - o.v }
    }
}

fn decimal_dogleg_mid(x1: Dec, y1: Dec, x2: Dec, y2: Dec, axis_first: bool) -> (Dec, Dec) {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let adx = dx.abs();
    let ady = dy.abs();
    let signed = |mag: Dec, like: Dec| if like.v >= 0 { mag } else { Dec { v: -mag.v } };
    if !axis_first {
        if adx >= ady {
            return (x1 + signed(ady, dx), y2);
        }
        return (x2, y1 + signed(adx, dy));
    }
    if adx >= ady {
        return (x2 - signed(ady, dx), y1);
    }
    (x1, y2 - signed(adx, dy))
}

/// Deterministic, collision-free uuid5 for a dogleg second leg.
pub fn derive_dogleg_uuid(parent_uuid: &str, seen: &mut HashSet<String>) -> String {
    let ns = uuid::Uuid::NAMESPACE_OID;
    let mut candidate = uuid::Uuid::new_v5(&ns, format!("{parent_uuid}:dogleg").as_bytes())
        .hyphenated()
        .to_string();
    let mut n = 1;
    while seen.contains(&candidate) {
        n += 1;
        candidate = uuid::Uuid::new_v5(&ns, format!("{parent_uuid}:dogleg:{n}").as_bytes())
            .hyphenated()
            .to_string();
    }
    seen.insert(candidate.clone());
    candidate
}

/// Options for [`quantize_pcb_text`] / [`quantize_pcb_file`].
#[derive(Debug, Clone, Default)]
pub struct QuantizeOptions {
    pub tol_deg: Option<f64>,
    pub axis_first_uuids: HashSet<String>,
    pub skip_uuids: HashSet<String>,
    pub dry_run: bool,
}

/// Rewrite every off-angle segment block in `text` as an exact dogleg.
/// Returns `(new_text, replaced_keys)`.
pub fn quantize_pcb_text(text: &str, opts: &QuantizeOptions) -> (String, Vec<String>) {
    let tol = opts.tol_deg.unwrap_or(ANGLE_TOL_DEG);
    let mut seen: HashSet<String> = uuid_attr_re()
        .captures_iter(text)
        .map(|c| c[1].to_string())
        .collect();
    let mut replaced = Vec::new();
    let re = segment_block_re();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in re.captures_iter(text) {
        let whole = m.get(0).expect("match");
        out.push_str(&text[last..whole.start()]);
        last = whole.end();
        let original = whole.as_str();
        let (Some(x1), Some(y1), Some(x2), Some(y2)) = (
            Dec::parse(&m[2]),
            Dec::parse(&m[3]),
            Dec::parse(&m[4]),
            Dec::parse(&m[5]),
        ) else {
            out.push_str(original);
            continue;
        };
        let indent = &m[1];
        let width = &m[6];
        let layer = &m[7];
        let net_field = match m.get(9) {
            Some(n) => format!("(net {})", n.as_str()),
            None => format!("(net \"{}\")", m.get(10).map_or("", |n| n.as_str())),
        };
        let seg_uuid = m
            .get(8)
            .or_else(|| m.get(11))
            .map(|u| u.as_str().to_string());
        let dx = (x2 - x1).to_f64();
        let dy = (y2 - y1).to_f64();
        if (dx == 0.0 && dy == 0.0) || is_45_aligned(dx, dy, tol) {
            out.push_str(original);
            continue;
        }
        let key = seg_uuid
            .clone()
            .unwrap_or_else(|| format!("{},{}-{},{}", &m[2], &m[3], &m[4], &m[5]));
        if opts.skip_uuids.contains(&key) {
            out.push_str(original);
            continue;
        }
        replaced.push(key.clone());
        if opts.dry_run {
            out.push_str(original);
            continue;
        }
        let (mx, my) = decimal_dogleg_mid(x1, y1, x2, y2, opts.axis_first_uuids.contains(&key));
        let inner = if indent == "\t" || indent == "  " {
            indent.repeat(2)
        } else {
            format!("{indent}\t")
        };
        let block = |sx: Dec, sy: Dec, ex: Dec, ey: Dec, u: Option<&str>| {
            let mut lines = vec![
                format!("{indent}(segment"),
                format!("{inner}(start {} {})", sx.fmt(), sy.fmt()),
                format!("{inner}(end {} {})", ex.fmt(), ey.fmt()),
                format!("{inner}(width {width})"),
                format!("{inner}(layer \"{layer}\")"),
            ];
            if let Some(u) = u {
                lines.push(format!("{inner}(uuid \"{u}\")"));
            }
            lines.push(format!("{inner}{net_field}"));
            lines.push(format!("{indent})"));
            lines.join("\n")
        };
        let second = seg_uuid
            .as_deref()
            .map(|u| derive_dogleg_uuid(u, &mut seen));
        out.push_str(&block(x1, y1, mx, my, seg_uuid.as_deref()));
        out.push('\n');
        out.push_str(&block(mx, my, x2, y2, second.as_deref()));
    }
    out.push_str(&text[last..]);
    (out, replaced)
}

/// Rewrite `pcb_path` in place (unless dry run); returns replaced keys.
pub fn quantize_pcb_file(pcb_path: &Path, opts: &QuantizeOptions) -> std::io::Result<Vec<String>> {
    let text = std::fs::read_to_string(pcb_path)?;
    let (new_text, replaced) = quantize_pcb_text(&text, opts);
    if !replaced.is_empty() && !opts.dry_run {
        std::fs::write(pcb_path, new_text)?;
    }
    Ok(replaced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles() {
        assert_eq!(off_angle_degrees(1.0, 0.0), 0.0);
        assert!(off_angle_degrees(1.0, 1.0) < 1e-9);
        assert!((off_angle_degrees(1.0, 0.5) - 18.43494882292201).abs() < 1e-9);
        assert!((off_angle_degrees(-1.0, -0.5) - 18.43494882292201).abs() < 1e-9);
        assert_eq!(snap_direction_8(1.0, 0.9), EIGHT_DIRECTIONS[1]);
        assert_eq!(snap_direction_8(-1.0, -0.1), EIGHT_DIRECTIONS[4]);
    }

    #[test]
    fn dogleg_variants() {
        assert_eq!(
            dogleg(0.0, 0.0, 2.0, 1.0, false),
            vec![(0.0, 0.0), (1.0, 1.0), (2.0, 1.0)]
        );
        assert_eq!(
            dogleg(0.0, 0.0, 2.0, 1.0, true),
            vec![(0.0, 0.0), (1.0, 0.0), (2.0, 1.0)]
        );
        assert_eq!(
            dogleg(0.0, 0.0, 1.0, 2.0, false),
            vec![(0.0, 0.0), (1.0, 1.0), (1.0, 2.0)]
        );
        assert_eq!(dogleg(0.0, 0.0, 2.0, 2.0, false).len(), 2);
    }

    #[test]
    fn verify_quantum_jitter_ok_and_strict_err() {
        assert!(verify_segment_45(0.0, 0.0, 0.1385, 0.1384, ANGLE_TOL_DEG, "", Some(true)).is_ok());
        assert!(verify_segment_45(0.0, 0.0, 2.0, 1.0, ANGLE_TOL_DEG, "x", Some(true)).is_err());
    }

    #[test]
    fn quantize_text_roundtrip() {
        let text = "(kicad_pcb\n\t(net 1 \"A\")\n\t(segment\n\t\t(start 0 0)\n\t\t(end 2 1)\n\t\t(width 0.2)\n\t\t(layer \"F.Cu\")\n\t\t(uuid \"u1\")\n\t\t(net 1)\n\t)\n)\n";
        let (total, bad) = segment_angle_census_text(text, ANGLE_TOL_DEG);
        assert_eq!((total, bad.len()), (1, 1));
        assert_eq!(bad[0].net, 1);
        let (out, rep) = quantize_pcb_text(text, &QuantizeOptions::default());
        assert_eq!(rep, vec!["u1".to_string()]);
        assert!(out.contains("(start 0 0)\n\t\t(end 1 1)"));
        assert!(out.contains("(start 1 1)\n\t\t(end 2 1)"));
        let expected = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, b"u1:dogleg").to_string();
        assert!(out.contains(&expected));
        let (_, bad2) = segment_angle_census_text(&out, ANGLE_TOL_DEG);
        assert!(bad2.is_empty());
    }

    #[test]
    fn decimal_fmt() {
        assert_eq!(Dec::parse("-0.0").unwrap().fmt(), "0");
        assert_eq!(Dec::parse("1.2500").unwrap().fmt(), "1.25");
        assert_eq!(Dec::parse("-3").unwrap().fmt(), "-3");
    }
}
