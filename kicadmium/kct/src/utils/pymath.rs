//! CPython `math` functions whose results can differ from libm in the last
//! bit (port of `Modules/mathmodule.c`).

#[inline]
fn dl_mul(x: f64, y: f64) -> (f64, f64) {
    let z = x * y;
    (z, x.mul_add(y, -z))
}

#[inline]
fn dl_fast_sum(a: f64, b: f64) -> (f64, f64) {
    let hi = a + b;
    (hi, (a - hi) + b)
}

/// CPython `vector_norm` over absolute values.
fn vector_norm(vec: &mut [f64], max: f64, found_nan: bool) -> f64 {
    if max.is_infinite() {
        return max;
    }
    if found_nan {
        return f64::NAN;
    }
    if max == 0.0 || vec.len() <= 1 {
        return max;
    }
    let (_, max_e) = frexp(max);
    if max_e < -1023 {
        for v in vec.iter_mut() {
            *v /= f64::MIN_POSITIVE;
        }
        return f64::MIN_POSITIVE * vector_norm(vec, max / f64::MIN_POSITIVE, found_nan);
    }
    let scale = ldexp(1.0, -max_e);
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    for &v in vec.iter() {
        let x = v * scale;
        let pr = dl_mul(x, x);
        let sm = dl_fast_sum(csum, pr.0);
        csum = sm.0;
        frac1 += pr.1;
        frac2 += sm.1;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let pr = dl_mul(-h, h);
    let sm = dl_fast_sum(csum, pr.0);
    csum = sm.0;
    frac1 += pr.1;
    frac2 += sm.1;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h);
    h / scale
}

fn frexp(x: f64) -> (f64, i32) {
    if x == 0.0 || !x.is_finite() {
        return (x, 0);
    }
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        let (m, e) = frexp(x * f64::powi(2.0, 54));
        return (m, e - 54);
    }
    let e = exp - 1022;
    let m = f64::from_bits((bits & !(0x7ffu64 << 52)) | (1022u64 << 52));
    (m, e)
}

fn ldexp(x: f64, e: i32) -> f64 {
    let mut r = x;
    let mut e = e;
    while e > 1000 {
        r *= f64::powi(2.0, 1000);
        e -= 1000;
    }
    while e < -1000 {
        r *= f64::powi(2.0, -1000);
        e += 1000;
    }
    r * f64::powi(2.0, e)
}

/// `math.hypot(*coords)`.
pub fn hypot_n(coords: &[f64]) -> f64 {
    let mut v: Vec<f64> = coords.iter().map(|c| c.abs()).collect();
    let mut max = 0.0f64;
    let mut nan = false;
    for &x in &v {
        nan |= x.is_nan();
        if x > max {
            max = x;
        }
    }
    vector_norm(&mut v, max, nan)
}

/// `math.hypot(x, y)`.
pub fn hypot(x: f64, y: f64) -> f64 {
    hypot_n(&[x, y])
}

/// `math.dist(p, q)` in 2D.
pub fn dist(p: (f64, f64), q: (f64, f64)) -> f64 {
    hypot_n(&[p.0 - q.0, p.1 - q.1])
}

/// CPython 3.12+ builtin `sum()` over floats (Neumaier compensated, start
/// value int 0).
pub fn py_sum<I: IntoIterator<Item = f64>>(items: I) -> f64 {
    let mut it = items.into_iter();
    let Some(first) = it.next() else {
        return 0.0;
    };
    let mut f = 0.0 + first;
    let mut c = 0.0f64;
    for x in it {
        let t = f + x;
        if f.abs() >= x.abs() {
            c += (f - t) + x;
        } else {
            c += (x - t) + f;
        }
        f = t;
    }
    if c != 0.0 && c.is_finite() {
        f += c;
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hypot_basics() {
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert_eq!(hypot(0.0, 0.0), 0.0);
        assert_eq!(hypot(-1.5, 0.0), 1.5);
        assert_eq!(dist((1.0, 1.0), (4.0, 5.0)), 5.0);
    }
}
