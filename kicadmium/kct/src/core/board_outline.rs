//! Port of `kicad_tools.core.board_outline`: shared, layer-aware bounds of
//! top-level PCB outline (`Edge.Cuts`) geometry.
//!
//! Only the bounds half of the upstream module is ported here (what the PCB
//! model needs for board-origin detection and `board_size`); the certified
//! tessellation used by the router (`board_outline_segments`) lives with the
//! router port.

use std::f64::consts::{PI, TAU};

use crate::sexp::SExp;

pub type Point = (f64, f64);
/// `(min_x, min_y, max_x, max_y)` in sheet-absolute mm.
pub type Bounds = (f64, f64, f64, f64);

/// Rotate `point` about `center` by `angle_deg` (CCW-positive in math axes).
fn rotate_point(point: Point, center: Point, angle_deg: f64) -> Point {
    let theta = angle_deg.to_radians();
    let (dx, dy) = (point.0 - center.0, point.1 - center.1);
    let (sin_t, cos_t) = theta.sin_cos();
    (
        center.0 + dx * cos_t - dy * sin_t,
        center.1 + dx * sin_t + dy * cos_t,
    )
}

/// Normalize a legacy center/endpoint/signed-sweep arc into on-arc
/// `(start, mid, end)` points without changing coordinates.
pub fn legacy_arc_points(point: Point, center: Point, angle_deg: f64) -> (Point, Point, Point) {
    (
        point,
        rotate_point(point, center, angle_deg / 2.0),
        rotate_point(point, center, angle_deg),
    )
}

fn coordinate(child: &SExp, context: &str) -> Result<Point, String> {
    if child.children.len() != 2 {
        return Err(format!(
            "Malformed Edge.Cuts {context}: expected two coordinates"
        ));
    }
    match (child.float_at(0), child.float_at(1)) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok((x, y)),
        _ => Err(format!("Malformed Edge.Cuts {context}: invalid coordinate")),
    }
}

fn point(node: &SExp, tag: &str) -> Result<Point, String> {
    let name = node.tag().unwrap_or("");
    match node.find_child(tag) {
        Some(child) => coordinate(child, &format!("{name} {tag}")),
        None => Err(format!(
            "Malformed Edge.Cuts {name}: missing {tag} coordinate"
        )),
    }
}

fn py_mod(a: f64, m: f64) -> f64 {
    let r = a % m;
    if r < 0.0 {
        r + m
    } else {
        r
    }
}

fn arc_points(start: Point, mid: Point, end: Point) -> Result<Vec<Point>, String> {
    let (bx, by) = (mid.0 - start.0, mid.1 - start.1);
    let (cx, cy) = (end.0 - start.0, end.1 - start.1);
    let det = 2.0 * (bx * cy - by * cx);
    if det.abs() < 1e-12 {
        return Err("Malformed Edge.Cuts gr_arc: collinear arc points".into());
    }
    let (b2, c2) = (bx * bx + by * by, cx * cx + cy * cy);
    let ox = start.0 + (cy * b2 - by * c2) / det;
    let oy = start.1 + (bx * c2 - cx * b2) / det;
    let radius = (start.0 - ox).hypot(start.1 - oy);
    let ang = |p: Point| (p.1 - oy).atan2(p.0 - ox);
    let (a, m, b) = (ang(start), ang(mid), ang(end));
    let span = py_mod(b - a, TAU);
    let ccw = py_mod(m - a, TAU) <= span;
    let mut points = vec![start, mid, end];
    for angle in [0.0, PI / 2.0, PI, 3.0 * PI / 2.0] {
        let on_arc = if ccw {
            py_mod(angle - a, TAU) <= span + 1e-12
        } else {
            py_mod(a - angle, TAU) <= py_mod(a - b, TAU) + 1e-12
        };
        if on_arc {
            points.push((ox + radius * angle.cos(), oy + radius * angle.sin()));
        }
    }
    Ok(points)
}

/// Whether a cubic closes along one exact line, enclosing no board area.
///
/// Uses exact arithmetic on the represented coordinates (no tolerance), so
/// arbitrarily small real loops remain geometry.
pub fn is_degenerate_closed_curve(points: &[Point]) -> bool {
    if points.len() != 4 || points[0] != points[3] {
        return false;
    }
    let mut distinct: Vec<Point> = Vec::new();
    for p in points {
        if !distinct.contains(p) {
            distinct.push(*p);
        }
    }
    if distinct.len() <= 2 {
        return true;
    }
    let (origin, first, second) = (points[0], points[1], points[2]);
    exact::cross_is_zero(
        [first.0, origin.0, first.1, origin.1],
        [second.0, origin.0, second.1, origin.1],
    )
}

fn curve_points(points: &[Point]) -> Result<Vec<Point>, String> {
    if points.len() != 4 {
        return Err("Malformed Edge.Cuts gr_curve: expected four cubic control points".into());
    }
    let mut extrema: Vec<f64> = vec![0.0, 1.0];
    for axis in 0..2 {
        let c = |i: usize| {
            if axis == 0 {
                points[i].0
            } else {
                points[i].1
            }
        };
        let (p0, p1, p2, p3) = (c(0), c(1), c(2), c(3));
        let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
        let b = 2.0 * (p0 - 2.0 * p1 + p2);
        let cc = p1 - p0;
        let roots: Vec<f64> = if a.abs() < 1e-12 {
            if b.abs() >= 1e-12 {
                vec![-cc / b]
            } else {
                vec![]
            }
        } else {
            let disc = b * b - 4.0 * a * cc;
            if disc >= 0.0 {
                [-1.0, 1.0]
                    .iter()
                    .map(|s| (-b + s * disc.sqrt()) / (2.0 * a))
                    .collect()
            } else {
                vec![]
            }
        };
        for t in roots {
            if 0.0 < t && t < 1.0 && !extrema.contains(&t) {
                extrema.push(t);
            }
        }
    }
    Ok(extrema
        .into_iter()
        .map(|t| {
            let w = [
                (1.0 - t).powi(3),
                3.0 * (1.0 - t).powi(2) * t,
                3.0 * (1.0 - t) * t * t,
                t.powi(3),
            ];
            (
                w.iter().zip(points).map(|(w, p)| w * p.0).sum(),
                w.iter().zip(points).map(|(w, p)| w * p.1).sum(),
            )
        })
        .collect())
}

/// Direct, explicitly layer-qualified `Edge.Cuts` graphics (`gr_*` only).
pub fn outline_graphics(root: &SExp) -> impl Iterator<Item = &SExp> {
    root.children.iter().filter(|node| {
        node.is_list()
            && node.tag().is_some_and(|t| t.starts_with("gr_"))
            && node
                .find_child("layer")
                .and_then(|l| l.text_at(0))
                .is_some_and(|l| l == "Edge.Cuts")
    })
}

/// Sheet-absolute bounds of the board outline, ignoring non-outline and
/// nested graphics. `Ok(None)` when there is no outline geometry; malformed
/// or unsupported `Edge.Cuts` graphics are an error. Stroke width and closed
/// collinear cubics are excluded; closure/topology is not validated.
pub fn board_outline_bounds(root: &SExp) -> Result<Option<Bounds>, String> {
    let mut points: Vec<Point> = Vec::new();
    for node in outline_graphics(root) {
        let tag = node.tag().unwrap_or("");
        match tag {
            "gr_rect" | "gr_line" => {
                points.push(point(node, "start")?);
                points.push(point(node, "end")?);
            }
            "gr_arc" => {
                let mut start = point(node, "start")?;
                let mut end = point(node, "end")?;
                let mid = if node.find_child("mid").is_some() {
                    point(node, "mid")?
                } else {
                    let angle = node.find_child("angle");
                    let sweep = angle.and_then(|a| a.float_at(0));
                    match (angle, sweep) {
                        (Some(a), Some(s)) if a.children.len() == 1 && s.is_finite() => {
                            let (s0, m, e) = legacy_arc_points(end, start, s);
                            start = s0;
                            end = e;
                            m
                        }
                        _ => {
                            return Err(
                                "Malformed Edge.Cuts gr_arc: missing mid or invalid legacy angle"
                                    .into(),
                            )
                        }
                    }
                };
                points.extend(arc_points(start, mid, end)?);
            }
            "gr_circle" => {
                let center = point(node, "center")?;
                let end = point(node, "end")?;
                let r = (center.0 - end.0).hypot(center.1 - end.1);
                points.push((center.0 - r, center.1 - r));
                points.push((center.0 + r, center.1 + r));
            }
            "gr_poly" | "gr_curve" => {
                let mut chain = Vec::new();
                if let Some(pts) = node.find_child("pts") {
                    for xy in pts.children_named("xy") {
                        chain.push(coordinate(xy, &format!("{tag} xy"))?);
                    }
                }
                if chain.len() < 3 {
                    return Err(format!("Malformed Edge.Cuts {tag}: missing points"));
                }
                if tag == "gr_curve" {
                    if is_degenerate_closed_curve(&chain) {
                        continue;
                    }
                    points.extend(curve_points(&chain)?);
                } else {
                    points.extend(chain);
                }
            }
            other => return Err(format!("Unsupported Edge.Cuts geometry: {other}")),
        }
    }
    if points.is_empty() {
        return Ok(None);
    }
    let mut b = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (x, y) in points {
        b.0 = b.0.min(x);
        b.1 = b.1.min(y);
        b.2 = b.2.max(x);
        b.3 = b.3.max(y);
    }
    Ok(Some(b))
}

/// Exact integer arithmetic on the binary values of `f64`s (the upstream
/// implementation uses `fractions.Fraction`).
mod exact {
    /// Signed arbitrary-precision integer: little-endian u32 limbs.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Big {
        neg: bool,
        mag: Vec<u32>,
    }

    impl Big {
        fn normalize(mut self) -> Self {
            while self.mag.last() == Some(&0) {
                self.mag.pop();
            }
            if self.mag.is_empty() {
                self.neg = false;
            }
            self
        }

        fn from_shifted(neg: bool, mant: u64, shift: u32) -> Self {
            let mut mag = vec![0u32; (shift / 32) as usize];
            let bit = shift % 32;
            let wide = (mant as u128) << bit;
            for i in 0..4 {
                mag.push((wide >> (32 * i)) as u32);
            }
            Big { neg, mag }.normalize()
        }

        fn cmp_mag(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
            a.len()
                .cmp(&b.len())
                .then_with(|| a.iter().rev().cmp(b.iter().rev()))
        }

        fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
            let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
            let mut carry = 0u64;
            for i in 0..a.len().max(b.len()) {
                let s = *a.get(i).unwrap_or(&0) as u64 + *b.get(i).unwrap_or(&0) as u64 + carry;
                out.push(s as u32);
                carry = s >> 32;
            }
            out.push(carry as u32);
            out
        }

        /// `a - b` for `|a| >= |b|`.
        fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
            let mut out = Vec::with_capacity(a.len());
            let mut borrow = 0i64;
            for i in 0..a.len() {
                let mut d = a[i] as i64 - *b.get(i).unwrap_or(&0) as i64 - borrow;
                borrow = 0;
                if d < 0 {
                    d += 1 << 32;
                    borrow = 1;
                }
                out.push(d as u32);
            }
            out
        }

        fn sub(&self, other: &Big) -> Big {
            let neg_other = Big {
                neg: !other.neg,
                mag: other.mag.clone(),
            };
            self.add(&neg_other.normalize())
        }

        fn add(&self, other: &Big) -> Big {
            if self.neg == other.neg {
                return Big {
                    neg: self.neg,
                    mag: Self::add_mag(&self.mag, &other.mag),
                }
                .normalize();
            }
            match Self::cmp_mag(&self.mag, &other.mag) {
                std::cmp::Ordering::Less => Big {
                    neg: other.neg,
                    mag: Self::sub_mag(&other.mag, &self.mag),
                },
                _ => Big {
                    neg: self.neg,
                    mag: Self::sub_mag(&self.mag, &other.mag),
                },
            }
            .normalize()
        }

        fn mul(&self, other: &Big) -> Big {
            let mut out = vec![0u64; self.mag.len() + other.mag.len() + 1];
            for (i, &a) in self.mag.iter().enumerate() {
                let mut carry = 0u64;
                for (j, &b) in other.mag.iter().enumerate() {
                    let cur = out[i + j] + a as u64 * b as u64 + carry;
                    out[i + j] = cur & 0xffff_ffff;
                    carry = cur >> 32;
                }
                let mut k = i + other.mag.len();
                while carry > 0 {
                    let cur = out[k] + carry;
                    out[k] = cur & 0xffff_ffff;
                    carry = cur >> 32;
                    k += 1;
                }
            }
            Big {
                neg: self.neg != other.neg,
                mag: out.into_iter().map(|v| v as u32).collect(),
            }
            .normalize()
        }
    }

    /// `x = sign * mant * 2^exp` exactly.
    fn decompose(x: f64) -> (bool, u64, i32) {
        let bits = x.to_bits();
        let neg = bits >> 63 == 1;
        let exp_bits = ((bits >> 52) & 0x7ff) as i32;
        let frac = bits & ((1u64 << 52) - 1);
        if exp_bits == 0 {
            (neg, frac, -1074)
        } else {
            (neg, frac | (1u64 << 52), exp_bits - 1075)
        }
    }

    /// Exact test of `(a0 - a1) * (b2 - b3) == (a2 - a3) * (b0 - b1)` where
    /// `a = [ax1, ax0, ay1, ay0]`, `b = [bx1, bx0, by1, by0]`: the cross
    /// product of `(ax1-ax0, ay1-ay0)` and `(bx1-bx0, by1-by0)` is zero.
    pub(super) fn cross_is_zero(a: [f64; 4], b: [f64; 4]) -> bool {
        let values = [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]];
        let parts: Vec<(bool, u64, i32)> = values.iter().map(|v| decompose(*v)).collect();
        let min_exp = parts
            .iter()
            .filter(|p| p.1 != 0)
            .map(|p| p.2)
            .min()
            .unwrap_or(0);
        let big: Vec<Big> = parts
            .iter()
            .map(|&(neg, mant, exp)| {
                if mant == 0 {
                    Big {
                        neg: false,
                        mag: vec![],
                    }
                } else {
                    Big::from_shifted(neg, mant, (exp - min_exp) as u32)
                }
            })
            .collect();
        let ax = big[0].sub(&big[1]);
        let ay = big[2].sub(&big[3]);
        let bx = big[4].sub(&big[5]);
        let by = big[6].sub(&big[7]);
        ax.mul(&by) == ay.mul(&bx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degenerate_curves() {
        let line = [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (0.0, 0.0)];
        assert!(is_degenerate_closed_curve(&line));
        let loop_ = [(0.0, 0.0), (1.0, 1.0), (2.0, 2.000001), (0.0, 0.0)];
        assert!(!is_degenerate_closed_curve(&loop_));
        let open = [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0)];
        assert!(!is_degenerate_closed_curve(&open));
        let far = [(1e6 + 0.1, 0.3), (1e6 + 0.2, 0.6), (1e6 + 0.3, 0.9), (1e6 + 0.1, 0.3)];
        // 0.1/0.2/0.3 are not exactly collinear in binary.
        let _ = is_degenerate_closed_curve(&far);
    }

    #[test]
    fn bounds_rect_and_arc() {
        let root = crate::parse(
            r#"(kicad_pcb
              (gr_rect (start 0 0) (end 10 5) (layer "Edge.Cuts"))
              (gr_arc (start 10 0) (mid 15 5) (end 10 10) (layer "Edge.Cuts"))
              (gr_line (start 0 0) (end 99 99) (layer "F.SilkS")))"#,
        )
        .unwrap();
        let b = board_outline_bounds(&root).unwrap().unwrap();
        assert!((b.2 - 15.0).abs() < 1e-9);
        assert!((b.3 - 10.0).abs() < 1e-9);
        assert_eq!((b.0, b.1), (0.0, 0.0));
    }

    #[test]
    fn legacy_arc() {
        let (s, m, e) = legacy_arc_points((101.0, 92.0), (101.0, 93.0), -90.0);
        assert_eq!(s, (101.0, 92.0));
        assert!((e.0 - 100.0).abs() < 1e-9 && (e.1 - 93.0).abs() < 1e-9);
        assert!((m.0 - 100.29289).abs() < 1e-4 && (m.1 - 92.29289).abs() < 1e-4);
    }

    #[test]
    fn malformed_is_error() {
        let root = crate::parse(r#"(kicad_pcb (gr_line (start 0) (end 1 1) (layer "Edge.Cuts")))"#)
            .unwrap();
        assert!(board_outline_bounds(&root).is_err());
        let none = crate::parse("(kicad_pcb)").unwrap();
        assert_eq!(board_outline_bounds(&none).unwrap(), None);
    }
}
