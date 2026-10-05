//! Conservative, deterministic broad-phase candidates for exact copper
//! checks (port of `kicad_tools.validate.spatial`).
//!
//! Upstream builds a shapely `STRtree` over the bounds and queries each
//! margin-expanded box; this is a sort-and-sweep over the same inclusive
//! envelope predicate, emitting the identical pair set in the identical
//! ascending `(i, j)` order.

/// `(min_x, min_y, max_x, max_y)`.
pub type Bounds = (f64, f64, f64, f64);

/// Inclusive envelope intersection of `bounds[i]` expanded by `margin` with
/// the unexpanded `bounds[j]` (shapely `box(...)` + `STRtree.query`).
#[inline]
fn hits(a: &Bounds, margin: f64, b: &Bounds) -> bool {
    a.0 - margin <= b.2 && a.2 + margin >= b.0 && a.1 - margin <= b.3 && a.3 + margin >= b.1
}

/// Every `(i, j)` with `i < j` whose margin-expanded envelopes overlap, in
/// ascending lexicographic order (upstream `candidate_pairs`).
pub fn candidate_pairs(bounds: &[Bounds], margin: f64) -> Vec<(usize, usize)> {
    let count = bounds.len();
    if count < 2 {
        return Vec::new();
    }
    let finite = margin.is_finite()
        && bounds
            .iter()
            .all(|b| b.0.is_finite() && b.1.is_finite() && b.2.is_finite() && b.3.is_finite());
    if !finite {
        // Upstream's scalar fallback: same predicate, brute force (NaN
        // envelopes never intersect anything).
        let mut out = Vec::new();
        for i in 0..count {
            for j in i + 1..count {
                if hits(&bounds[i], margin, &bounds[j]) {
                    out.push((i, j));
                }
            }
        }
        return out;
    }
    // Sweep on x: sort by min_x, and for each element scan forward while the
    // next min_x is within reach of this element's expanded max_x.
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|&a, &b| bounds[a].0.total_cmp(&bounds[b].0));
    let mut out = Vec::new();
    for (pos, &i) in order.iter().enumerate() {
        let reach = bounds[i].2 + margin;
        for &j in &order[pos + 1..] {
            if bounds[j].0 > reach {
                break;
            }
            // Symmetric predicate check in both directions keeps the exact
            // upstream semantics (query box is always the lower index's).
            let (lo, hi) = if i < j { (i, j) } else { (j, i) };
            if hits(&bounds[lo], margin, &bounds[hi]) {
                out.push((lo, hi));
            }
        }
    }
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_brute_force() {
        let b: Vec<Bounds> = (0..40)
            .map(|k| {
                let x = ((k * 37) % 23) as f64 * 0.7;
                let y = ((k * 11) % 17) as f64 * 0.9;
                (x, y, x + (k % 5) as f64 * 0.3, y + (k % 3) as f64 * 0.4)
            })
            .collect();
        for margin in [0.0, 0.1, 1.5] {
            let mut brute = Vec::new();
            for i in 0..b.len() {
                for j in i + 1..b.len() {
                    if hits(&b[i], margin, &b[j]) {
                        brute.push((i, j));
                    }
                }
            }
            assert_eq!(candidate_pairs(&b, margin), brute);
        }
    }
}
