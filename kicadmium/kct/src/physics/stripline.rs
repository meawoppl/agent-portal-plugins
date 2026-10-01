//! Quasi-static capacitance of a rectangular strip between two reference
//! planes (port of `physics.stripline`).
//!
//! Homogeneous dielectric, infinite planes, rectangular conductor, no nearby
//! conductors. For planes `y=0,H` the Dirichlet Green function is
//!
//! ```text
//! G = log((cosh(pi*dx/H)-cos(pi*(y+ys)/H)) / (cosh(pi*dx/H)-cos(pi*(y-ys)/H))) / (4*pi)
//! ```
//!
//! Constant-charge panels (cosine graded, 24-point Gauss-Legendre with the
//! analytic log self-term) enforce unit potential; total charge is
//! `C/epsilon`. Kernel reference: Khélifa and Chorfi (2019), section 2.3.

use std::collections::HashMap;
use std::f64::consts::{LN_2, PI};
use std::sync::{Mutex, OnceLock};

use super::constants::{SPEED_OF_LIGHT, VACUUM_PERMITTIVITY};
use super::{PhysResult, ValueError};

/// Impedance in ohms; dimensions share any consistent unit. `h1`/`h2` are the
/// clear dielectric gaps from the conductor faces to their planes; `t = 0` is a
/// single thin strip. Geometry solutions are cached; permittivity scales the
/// result by `1/sqrt(er)`.
pub fn stripline_impedance(w: f64, h1: f64, h2: f64, t: f64, er: f64) -> PhysResult<f64> {
    if [w, h1, h2, er].iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err(ValueError::new(
            "Stripline width, gaps and permittivity must be finite and positive",
        ));
    }
    if !t.is_finite() || t < 0.0 {
        return Err(ValueError::new(
            "Stripline copper thickness must be finite and nonnegative",
        ));
    }
    let (near, far) = if h1 <= h2 { (h1, h2) } else { (h2, h1) };
    Ok(vacuum_impedance(w, near, far, t)? / er.sqrt())
}

type Key = [u64; 4];

fn cache() -> &'static Mutex<HashMap<Key, f64>> {
    static CACHE: OnceLock<Mutex<HashMap<Key, f64>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn vacuum_impedance(w: f64, h1: f64, h2: f64, t: f64) -> PhysResult<f64> {
    let key = [w.to_bits(), h1.to_bits(), h2.to_bits(), t.to_bits()];
    if let Some(v) = cache().lock().unwrap().get(&key) {
        return Ok(*v);
    }
    let v = solve_vacuum_impedance(w, h1, h2, t)?;
    let mut map = cache().lock().unwrap();
    if map.len() >= 4096 {
        map.clear();
    }
    map.insert(key, v);
    Ok(v)
}

/// 24-point Gauss-Legendre nodes and weights on [-1, 1].
fn leggauss24() -> &'static ([f64; 24], [f64; 24]) {
    static GL: OnceLock<([f64; 24], [f64; 24])> = OnceLock::new();
    GL.get_or_init(|| {
        const N: usize = 24;
        let mut x = [0.0; N];
        let mut wts = [0.0; N];
        for i in 0..N {
            // Chebyshev-style initial guess, then Newton on P_N.
            let mut z = (PI * (i as f64 + 0.75) / (N as f64 + 0.5)).cos();
            for _ in 0..100 {
                let (mut p0, mut p1) = (1.0, z);
                for k in 2..=N {
                    let kf = k as f64;
                    let p2 = ((2.0 * kf - 1.0) * z * p1 - (kf - 1.0) * p0) / kf;
                    p0 = p1;
                    p1 = p2;
                }
                let dp = N as f64 * (z * p1 - p0) / (z * z - 1.0);
                let dz = p1 / dp;
                z -= dz;
                if dz.abs() < 1e-16 {
                    break;
                }
            }
            // Recompute derivative at the converged root for the weight.
            let (mut p0, mut p1) = (1.0, z);
            for k in 2..=N {
                let kf = k as f64;
                let p2 = ((2.0 * kf - 1.0) * z * p1 - (kf - 1.0) * p0) / kf;
                p0 = p1;
                p1 = p2;
            }
            let dp = N as f64 * (z * p1 - p0) / (z * z - 1.0);
            // numpy orders nodes ascending.
            x[N - 1 - i] = z;
            wts[N - 1 - i] = 2.0 / ((1.0 - z * z) * dp * dp);
        }
        (x, wts)
    })
}

/// numpy `logaddexp`.
fn logaddexp(a: f64, b: f64) -> f64 {
    if a == b {
        return a + LN_2;
    }
    let d = a - b;
    if d > 0.0 {
        a + (-d).exp().ln_1p()
    } else if d <= 0.0 {
        b + d.exp().ln_1p()
    } else {
        d // NaN
    }
}

fn solve_vacuum_impedance(w: f64, h1: f64, h2: f64, t: f64) -> PhysResult<f64> {
    let height = h1 + h2 + t;
    let (w, h1, t) = (w / height, h1 / height, t / height);
    let panel_count = if t > 0.0 { 48 } else { 96 };
    // numpy linspace(0, pi, n+1): i*step, last exactly pi.
    let step = PI / panel_count as f64;
    let fractions: Vec<f64> = (0..=panel_count)
        .map(|i| {
            let theta = if i == panel_count {
                PI
            } else {
                i as f64 * step
            };
            (1.0 - theta.cos()) / 2.0
        })
        .collect();
    let mut edges = vec![((-w / 2.0, h1), (w / 2.0, h1))];
    if t > 0.0 {
        edges.push(((w / 2.0, h1), (w / 2.0, h1 + t)));
        edges.push(((w / 2.0, h1 + t), (-w / 2.0, h1 + t)));
        edges.push(((-w / 2.0, h1 + t), (-w / 2.0, h1)));
    }
    let mut a: Vec<(f64, f64)> = Vec::new();
    let mut b: Vec<(f64, f64)> = Vec::new();
    for (first, last) in edges {
        let pts: Vec<(f64, f64)> = fractions
            .iter()
            .map(|f| {
                (
                    first.0 + f * (last.0 - first.0),
                    first.1 + f * (last.1 - first.1),
                )
            })
            .collect();
        a.extend_from_slice(&pts[..pts.len() - 1]);
        b.extend_from_slice(&pts[1..]);
    }
    let n = a.len();
    let mid: Vec<(f64, f64)> = (0..n)
        .map(|i| ((a[i].0 + b[i].0) / 2.0, (a[i].1 + b[i].1) / 2.0))
        .collect();
    let lengths: Vec<f64> = (0..n)
        .map(|i| (b[i].0 - a[i].0).hypot(b[i].1 - a[i].1))
        .collect();
    let (nodes, weights) = leggauss24();
    // sources[j][k]
    let sources: Vec<[(f64, f64); 24]> = (0..n)
        .map(|j| {
            let mut s = [(0.0, 0.0); 24];
            for k in 0..24 {
                s[k] = (
                    mid[j].0 + (b[j].0 - a[j].0) * nodes[k] / 2.0,
                    mid[j].1 + (b[j].1 - a[j].1) * nodes[k] / 2.0,
                );
            }
            s
        })
        .collect();
    let four_pi = 4.0 * PI;
    let mut matrix = vec![0.0f64; n * n];
    for i in 0..n {
        let (xi, y) = mid[i];
        for j in 0..n {
            let mut acc = 0.0;
            for k in 0..24 {
                let (xs, ys) = sources[j][k];
                let dx = xi - xs;
                let distance = (PI * dx / 2.0).abs();
                let log_sinh_sq = 2.0 * (distance + (-(-2.0 * distance).exp_m1()).ln() - LN_2);
                let plus = 2.0 * (PI * (y + ys) / 2.0).sin().abs().ln();
                let minus = 2.0 * (PI * (y - ys) / 2.0).sin().abs().ln();
                let green =
                    (logaddexp(log_sinh_sq, plus) - logaddexp(log_sinh_sq, minus)) / four_pi;
                acc += green * weights[k];
            }
            matrix[i * n + j] = acc * lengths[j] / 2.0;
        }
    }
    for i in 0..n {
        let li = lengths[i];
        let mut s = 0.0;
        for k in 0..24 {
            s += (nodes[k].abs() * li / 2.0).ln() * weights[k];
        }
        let sampled_self = -s * li / four_pi;
        let exact_self = li * (1.0 - (li / 2.0).ln()) / (2.0 * PI);
        matrix[i * n + i] += exact_self - sampled_self;
    }
    let density = lu_solve(&mut matrix, n, vec![1.0; n]).ok_or_else(|| {
        ValueError::new("Stripline capacitance solve did not produce a positive finite result")
    })?;
    let c: f64 = density.iter().zip(&lengths).map(|(d, l)| d * l).sum();
    if !c.is_finite() || c <= 0.0 {
        return Err(ValueError::new(
            "Stripline capacitance solve did not produce a positive finite result",
        ));
    }
    Ok(1.0 / (SPEED_OF_LIGHT * VACUUM_PERMITTIVITY * c))
}

/// Gaussian elimination with partial pivoting (row-major `m`, size `n`).
fn lu_solve(m: &mut [f64], n: usize, mut rhs: Vec<f64>) -> Option<Vec<f64>> {
    for col in 0..n {
        let mut piv = col;
        let mut best = m[col * n + col].abs();
        for r in col + 1..n {
            let v = m[r * n + col].abs();
            if v > best {
                best = v;
                piv = r;
            }
        }
        if best == 0.0 || !best.is_finite() {
            return None;
        }
        if piv != col {
            for c in 0..n {
                m.swap(col * n + c, piv * n + c);
            }
            rhs.swap(col, piv);
        }
        let d = m[col * n + col];
        for r in col + 1..n {
            let f = m[r * n + col] / d;
            if f != 0.0 {
                for c in col..n {
                    m[r * n + c] -= f * m[col * n + c];
                }
                rhs[r] -= f * rhs[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut s = rhs[r];
        for c in r + 1..n {
            s -= m[r * n + c] * x[c];
        }
        x[r] = s / m[r * n + r];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauss_legendre_integrates_polynomials() {
        let (x, w) = leggauss24();
        let sum: f64 = w.iter().sum();
        assert!((sum - 2.0).abs() < 1e-14);
        let x4: f64 = x.iter().zip(w).map(|(x, w)| x.powi(4) * w).sum();
        assert!((x4 - 0.4).abs() < 1e-14);
        assert!(x[0] < x[23]);
    }
}
