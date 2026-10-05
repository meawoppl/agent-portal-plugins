//! Endpoint-preserving, bounded-error routing outline chains (port of
//! `kicad_tools.core.outline_tessellation`).
//!
//! Callers MUST carry `max_error` into clearance consumers as a Hausdorff
//! allowance, never absorb it into a comparison epsilon. Resource or
//! floating-point limits refuse the outline instead of silently relaxing the
//! bound. Authored coordinates are never rewritten. Certification uses exact
//! rational arithmetic on the parsed binary floats (Python `Fraction`).

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

use crate::exceptions::ValueError;

pub type Point = (f64, f64);

pub const DEFAULT_MAX_ERROR_MM: f64 = 1e-4;
pub const MAX_SEGMENTS: usize = 20_000;
pub const MAX_DEPTH: usize = 32;

type Rational = BigRational;
type RationalPoint = (Rational, Rational);

const TAU: f64 = std::f64::consts::TAU;

fn err(msg: &str) -> ValueError {
    ValueError::new(msg)
}

/// Exact rational value of a finite float.
pub(crate) fn frac(x: f64) -> Rational {
    Rational::from_float(x).expect("finite float")
}

/// Python `float(Fraction)`; `None` when the value overflows.
pub(crate) fn to_float(r: &Rational) -> Option<f64> {
    r.to_f64().filter(|f| f.is_finite())
}

/// Python `math.ulp`.
pub(crate) fn ulp(x: f64) -> f64 {
    let a = x.abs();
    if a.is_nan() || a.is_infinite() {
        return a;
    }
    if a == f64::MAX {
        return a - a.next_down();
    }
    a.next_up() - a
}

/// Python `math.frexp(x)[1]`.
fn frexp_exp(x: f64) -> i32 {
    if x == 0.0 || !x.is_finite() {
        return 0;
    }
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        // Subnormal: normalize.
        let scaled = x * 2f64.powi(64);
        return frexp_exp(scaled) - 64;
    }
    exp - 1022
}

/// Python `math.ldexp`.
fn ldexp(mut x: f64, mut e: i32) -> f64 {
    while e > 1000 {
        x *= 2f64.powi(1000);
        e -= 1000;
    }
    while e < -1000 {
        x *= 2f64.powi(-1000);
        e += 1000;
    }
    x * 2f64.powi(e)
}

fn pow2(e: i64) -> Rational {
    let one = BigInt::one();
    if e >= 0 {
        Rational::from_integer(one << e as usize)
    } else {
        Rational::new(one.clone(), one << (-e) as usize)
    }
}

fn budget(points: &[Point], max_error: f64, max_segments: usize) -> Result<f64, ValueError> {
    if !max_error.is_finite() || max_error <= 0.0 {
        return Err(err("Outline max_error must be finite and positive"));
    }
    if max_segments < 1 {
        return Err(err("Outline segment resource budget must be positive"));
    }
    if points.iter().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
        return Err(err("Malformed Edge.Cuts: nonfinite or invalid coordinates"));
    }
    // Conservative precision preflight (not the geometric certificate).
    let rounding = 256.0
        * points
            .iter()
            .flat_map(|(x, y)| [ulp(*x), ulp(*y)])
            .fold(0.0, f64::max);
    let remaining = max_error - rounding;
    if remaining <= 0.0 || !rounding.is_finite() {
        return Err(err(
            "Cannot certify outline error at this coordinate precision",
        ));
    }
    Ok(remaining)
}

/// Exact finite-segment distance comparison, without division or roots.
fn inside_chord_capsule(
    point: &RationalPoint,
    start: &RationalPoint,
    end: &RationalPoint,
    error_squared: &Rational,
) -> bool {
    let dx = &end.0 - &start.0;
    let dy = &end.1 - &start.1;
    let px = &point.0 - &start.0;
    let py = &point.1 - &start.1;
    let length_squared = &dx * &dx + &dy * &dy;
    let projection = &px * &dx + &py * &dy;
    if projection <= Rational::zero() {
        return &px * &px + &py * &py <= *error_squared;
    }
    if projection >= length_squared {
        let qx = &point.0 - &end.0;
        let qy = &point.1 - &end.1;
        return &qx * &qx + &qy * &qy <= *error_squared;
    }
    let cross = &px * &dy - &py * &dx;
    &cross * &cross <= error_squared * &length_squared
}

fn mid(a: &RationalPoint, b: &RationalPoint) -> RationalPoint {
    let two = Rational::from_integer(BigInt::from(2));
    ((&a.0 + &b.0) / &two, (&a.1 + &b.1) / &two)
}

/// [`tessellate_cubic_with`] at the default error/resource budget.
pub fn tessellate_cubic(points: &[Point]) -> Result<Vec<Point>, ValueError> {
    tessellate_cubic_with(points, DEFAULT_MAX_ERROR_MM, MAX_SEGMENTS, MAX_DEPTH)
}

/// Adaptive de Casteljau subdivision with a two-sided geometric bound: a
/// chord is accepted when both inner control points lie within `max_error`
/// of the *finite* chord (its capsule), which bounds the Hausdorff error in
/// both directions.
pub fn tessellate_cubic_with(
    points: &[Point],
    max_error: f64,
    max_segments: usize,
    max_depth: usize,
) -> Result<Vec<Point>, ValueError> {
    if points.len() != 4 {
        return Err(err(
            "Malformed Edge.Cuts gr_curve: expected four cubic control points",
        ));
    }
    if max_depth > MAX_DEPTH {
        return Err(ValueError::new(format!(
            "Outline subdivision depth budget must be in [0, {MAX_DEPTH}]"
        )));
    }
    budget(points, max_error, max_segments)?;
    let rounding = 2.0
        * points
            .iter()
            .flat_map(|(x, y)| [ulp(*x), ulp(*y)])
            .fold(0.0, f64::max);
    let allowance = frac(max_error) - frac(rounding);
    let error_squared = &allowance * &allowance;
    let mut output = vec![points[0]];
    let rational: Vec<RationalPoint> = points.iter().map(|(x, y)| (frac(*x), frac(*y))).collect();
    let mut stack: Vec<(Vec<RationalPoint>, usize)> = vec![(rational, 0)];
    while let Some((control, depth)) = stack.pop() {
        let (p0, p1, p2, p3) = (&control[0], &control[1], &control[2], &control[3]);
        if inside_chord_capsule(p1, p0, p3, &error_squared)
            && inside_chord_capsule(p2, p0, p3, &error_squared)
        {
            if output.len() > max_segments {
                return Err(err("Outline subdivision exceeds segment resource budget"));
            }
            let x = to_float(&p3.0).ok_or_else(|| err("Cannot certify cubic endpoint"))?;
            let y = to_float(&p3.1).ok_or_else(|| err("Cannot certify cubic endpoint"))?;
            output.push((x, y));
            continue;
        }
        if depth >= max_depth || output.len() + stack.len() >= max_segments {
            return Err(err(
                "Cannot certify cubic within subdivision resource budget",
            ));
        }
        let a = mid(p0, p1);
        let b = mid(p1, p2);
        let c = mid(p2, p3);
        let d = mid(&a, &b);
        let e = mid(&b, &c);
        let f = mid(&d, &e);
        stack.push((vec![f.clone(), e, c, p3.clone()], depth + 1));
        stack.push((vec![p0.clone(), a, d, f], depth + 1));
    }
    Ok(output)
}

/// Exact circumcenter and orientation sign (+1 CCW, -1 CW).
fn arc_center(start: Point, mid: Point, end: Point) -> Result<(RationalPoint, i32), ValueError> {
    let x0 = frac(start.0);
    let y0 = frac(start.1);
    let bx = frac(mid.0) - &x0;
    let by = frac(mid.1) - &y0;
    let cx = frac(end.0) - &x0;
    let cy = frac(end.1) - &y0;
    let det = Rational::from_integer(BigInt::from(2)) * (&bx * &cy - &by * &cx);
    if det.is_zero() {
        return Err(err(
            "Malformed Edge.Cuts gr_arc: collinear/coincident arc points",
        ));
    }
    let b2 = &bx * &bx + &by * &by;
    let c2 = &cx * &cx + &cy * &cy;
    let center = (
        &x0 + (&cy * &b2 - &by * &c2) / &det,
        &y0 + (&bx * &c2 - &cx * &b2) / &det,
    );
    Ok((center, if det.is_positive() { 1 } else { -1 }))
}

/// Certify the actual rounded chords by exact annulus containment.
fn certify_arc_chords(
    points: &[Point],
    center: &RationalPoint,
    radius: f64,
    direction: i32,
    max_error: f64,
) -> Result<(), ValueError> {
    let relative: Vec<RationalPoint> = points
        .iter()
        .map(|(x, y)| (frac(*x) - &center.0, frac(*y) - &center.1))
        .collect();
    let r0 = &relative[0];
    let radius_squared = &r0.0 * &r0.0 + &r0.1 * &r0.1;
    let exponent = frexp_exp(radius);
    let scaled = &radius_squared / pow2(2 * exponent as i64);
    let rounded = ldexp(scaled.to_f64().unwrap_or(f64::NAN).sqrt(), exponent);
    let (mut lower, mut upper) = (rounded, rounded);
    let mut enclosed = false;
    for _ in 0..16 {
        if lower.is_finite()
            && upper.is_finite()
            && frac(lower) * frac(lower) <= radius_squared
            && radius_squared <= frac(upper) * frac(upper)
        {
            enclosed = true;
            break;
        }
        lower = if lower > 0.0 {
            lower.next_down()
        } else {
            lower
        };
        upper = upper.next_up();
    }
    if !enclosed {
        return Err(err("Cannot certify arc radius enclosure"));
    }
    let inner = frac(upper) - frac(max_error);
    let inner = if inner.is_negative() {
        Rational::zero()
    } else {
        inner
    };
    let inner_squared = &inner * &inner;
    let outer = frac(lower) + frac(max_error);
    let outer_squared = &outer * &outer;
    for pair in relative.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let a2 = &a.0 * &a.0 + &a.1 * &a.1;
        let b2 = &b.0 * &b.0 + &b.1 * &b.1;
        let dx = &b.0 - &a.0;
        let dy = &b.1 - &a.1;
        let length_squared = &dx * &dx + &dy * &dy;
        let cross = &a.0 * &b.1 - &a.1 * &b.0;
        let signed = if direction > 0 {
            cross.clone()
        } else {
            -cross.clone()
        };
        if !signed.is_positive() || length_squared.is_zero() {
            return Err(err("Cannot certify arc chord orientation"));
        }
        let projection = -(&a.0 * &dx + &a.1 * &dy);
        let nearest_squared = if projection <= Rational::zero() {
            a2.clone()
        } else if projection >= length_squared {
            b2.clone()
        } else {
            &cross * &cross / &length_squared
        };
        let far = if a2 > b2 { &a2 } else { &b2 };
        if nearest_squared < inner_squared || *far > outer_squared {
            return Err(err("Cannot certify rounded arc chord error"));
        }
    }
    Ok(())
}

/// [`tessellate_arc_with`] at the default error/resource budget.
pub fn tessellate_arc(start: Point, mid: Point, end: Point) -> Result<Vec<Point>, ValueError> {
    tessellate_arc_with(start, mid, end, DEFAULT_MAX_ERROR_MM, MAX_SEGMENTS)
}

/// Circular start/mid/end arc, preserving the authored sweep and all three
/// points (split at the authored midpoint; each subarc's chord sagitta is
/// `2*r*sin(sweep/(4n))^2`).
pub fn tessellate_arc_with(
    start: Point,
    mid: Point,
    end: Point,
    max_error: f64,
    max_segments: usize,
) -> Result<Vec<Point>, ValueError> {
    budget(&[start, mid, end], max_error, max_segments)?;
    let (exact_center, direction) = arc_center(start, mid, end)?;
    let center = match (to_float(&exact_center.0), to_float(&exact_center.1)) {
        (Some(x), Some(y)) => (x, y),
        _ => return Err(err("Cannot certify unrepresentable arc center")),
    };
    let radius = (start.0 - center.0).hypot(start.1 - center.1);
    let available = budget(
        &[start, mid, end, center, (radius, radius)],
        max_error,
        max_segments,
    )?;
    if !radius.is_finite() || radius <= 0.0 {
        return Err(err("Malformed Edge.Cuts gr_arc: invalid radius"));
    }
    let half_angle = (0.25f64.min(available / (2.0 * radius))).sqrt().asin();
    if half_angle <= 0.0 {
        return Err(err("Cannot certify arc error at this radius"));
    }
    let dir = direction as f64;
    let mut result = vec![start];
    for (first, last) in [(start, mid), (mid, end)] {
        let a = (first.1 - center.1).atan2(first.0 - center.0);
        let b = (last.1 - center.1).atan2(last.0 - center.0);
        let span = (dir * (b - a)).rem_euclid(TAU);
        if span == 0.0 {
            return Err(err("Cannot certify numerically unresolved arc sweep"));
        }
        let count = ((span / (4.0 * half_angle)).ceil() as usize).max(1);
        if result.len() - 1 + count > max_segments {
            return Err(err("Cannot certify arc within segment resource budget"));
        }
        for i in 1..count {
            let angle = a + dir * span * i as f64 / count as f64;
            result.push((
                center.0 + radius * angle.cos(),
                center.1 + radius * angle.sin(),
            ));
        }
        result.push(last);
    }
    certify_arc_chords(&result, &exact_center, radius, direction, max_error)?;
    Ok(result)
}
