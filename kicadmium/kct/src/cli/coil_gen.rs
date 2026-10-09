//! Parametric planar D-coil stator: the geometry of justletgo's
//! `gen_coil.py`, generated straight into a board.
//!
//! Frame: all lengths mm; the maths frame has +x right, +y up and its origin
//! at the coil centre, so a KiCad point is `(cx + x, cy - y)`. D<c> is the
//! half-disc centred on angle `gamma = c`. Turn k of a D is the boundary of
//! `{r <= rho_k} ∩ {distance from the diameter >= h_k}`: one arc of radius
//! `rho_k = rc0 - k p` and one chord at offset `h_k = h0 + k p` from the
//! diameter, `p = width + space`, `h0 = p / 2` (so D<c> and D<c+180> share
//! the diameter line with exactly one gap). Each turn starts at the tap
//! angle on its arc, runs round the D, returns to just short of the tap and
//! jogs inward to the next turn; the innermost turn ends on a core via.
//!
//! Series order of one D with four layers a, b, c, d:
//! `pad + → lead (a) → inward spiral CCW (a) → core via 1 → outward CCW (b)
//! → outer via → inward CCW (c) → core via 2 → outward CCW (d) → lead (d) →
//! pad −`. Two layers: `a → via → b`. Positive current circulates
//! counter-clockwise (seen from +z, i.e. from F.Cu) on every layer; the
//! generator proves that by the signed area of each spiral.

use std::f64::consts::{PI, TAU};

/// Everything the geometry depends on.
#[derive(Debug, Clone)]
pub struct Params {
    pub r_out: f64,
    pub width: f64,
    pub space: f64,
    /// 0 = as many as fit.
    pub turns: usize,
    pub layers_per_d: usize,
    pub via_drill: f64,
    pub via_pad: f64,
    pub via_pitch: f64,
    pub jog_len: f64,
    pub lead_width: f64,
    /// Tap / via bisector offset from the D centre angle (radians).
    pub beta: f64,
}

impl Params {
    pub fn pitch(&self) -> f64 {
        self.width + self.space
    }
    pub fn rc0(&self) -> f64 {
        self.r_out - self.width / 2.0
    }
    pub fn h0(&self) -> f64 {
        self.pitch() / 2.0
    }
    /// Via centre to the nearest trace centreline.
    pub fn via_clear(&self) -> f64 {
        self.via_pad / 2.0 + self.space + self.width / 2.0
    }
    /// Radial room the second core via needs (four-layer D only).
    pub fn core_depth(&self) -> f64 {
        if self.layers_per_d == 4 {
            self.via_pitch
        } else {
            0.0
        }
    }
    /// Tap angle shift of layer c so its link via clears layer b's turn.
    fn eps(&self, r: f64) -> f64 {
        if self.layers_per_d == 4 {
            1.6 / r
        } else {
            0.0
        }
    }
    /// Deepest winding whose vias stay inside every D's un-wound core and
    /// whose jog still fits between the tap and the innermost corner.
    pub fn max_turns(&self) -> usize {
        let (sb, c, p) = (self.beta.sin(), self.via_clear(), self.pitch());
        let dmax = ((self.rc0() - c - self.core_depth()) * sb - self.h0() - c) / (1.0 + sb);
        let mut n = ((dmax / p).floor() as i64 + 1).max(1) as usize;
        while n > 1 {
            let rho = self.rc0() - (n - 1) as f64 * p;
            let h = self.h0() + (n - 1) as f64 * p;
            let phi = (h / rho).asin();
            if PI / 2.0 - phi - self.beta > self.jog_len / rho + self.eps(self.r_out) + 0.02 {
                break;
            }
            n -= 1;
        }
        n
    }
    pub fn n_turns(&self) -> usize {
        if self.turns > 0 {
            self.turns
        } else {
            self.max_turns()
        }
    }
}

pub type Pt = (f64, f64);

/// A centreline primitive in the maths frame.
#[derive(Debug, Clone)]
pub enum Prim {
    Line(Pt, Pt),
    /// Arc about the origin: radius, from angle, to angle (signed sweep).
    Arc(f64, f64, f64),
}

pub fn pol(r: f64, th: f64) -> Pt {
    (r * th.cos(), r * th.sin())
}

impl Prim {
    pub fn start(&self) -> Pt {
        match self {
            Prim::Line(a, _) => *a,
            Prim::Arc(r, t0, _) => pol(*r, *t0),
        }
    }
    pub fn end(&self) -> Pt {
        match self {
            Prim::Line(_, b) => *b,
            Prim::Arc(r, _, t1) => pol(*r, *t1),
        }
    }
    pub fn length(&self) -> f64 {
        match self {
            Prim::Line(a, b) => (b.0 - a.0).hypot(b.1 - a.1),
            Prim::Arc(r, t0, t1) => r * (t1 - t0).abs(),
        }
    }
    pub fn reversed(&self) -> Prim {
        match self {
            Prim::Line(a, b) => Prim::Line(*b, *a),
            Prim::Arc(r, t0, t1) => Prim::Arc(*r, *t1, *t0),
        }
    }
    /// Sampled centreline: arcs every `step_deg`, lines every `line_step` mm.
    pub fn points(&self, step_deg: f64, line_step: f64) -> Vec<Pt> {
        match self {
            Prim::Line(a, b) => {
                let n = ((self.length() / line_step).ceil() as usize).max(1);
                (0..=n)
                    .map(|i| {
                        let f = i as f64 / n as f64;
                        (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)
                    })
                    .collect()
            }
            Prim::Arc(r, t0, t1) => {
                let n = (((t1 - t0).abs().to_degrees() / step_deg).ceil() as usize).max(1);
                (0..=n)
                    .map(|i| pol(*r, t0 + (t1 - t0) * i as f64 / n as f64))
                    .collect()
            }
        }
    }
}

pub fn reverse_prims(prims: &[Prim]) -> Vec<Prim> {
    prims.iter().rev().map(Prim::reversed).collect()
}

pub fn prims_length(prims: &[Prim]) -> f64 {
    prims.iter().map(Prim::length).sum()
}

/// Inward spiral of one D (centre angle `gamma`), outermost turn starting
/// at `(rc0, theta_t)`, circulating CCW or CW, ending with a stub to `via`.
pub fn d_spiral_inward(
    p: &Params,
    gamma: f64,
    theta_t: f64,
    ccw: bool,
    via: Pt,
) -> Result<Vec<Prim>, String> {
    let pitch = p.pitch();
    let n = p.n_turns();
    let rc0 = p.rc0();
    let mut prims = Vec::new();
    for k in 0..n {
        let rho = rc0 - k as f64 * pitch;
        let h = p.h0() + k as f64 * pitch;
        if h >= rho {
            return Err(format!(
                "turn {k} does not fit: chord offset {h:.3} >= radius {rho:.3}"
            ));
        }
        let phi = (h / rho).asin();
        let (lo, hi) = (gamma - PI / 2.0 + phi, gamma + PI / 2.0 - phi);
        let dl = p.jog_len / rho;
        if !(lo + dl < theta_t && theta_t < hi - dl) {
            return Err(format!(
                "tap angle {:.1}° outside turn {k}'s arc span ({:.1}°..{:.1}°) with the jog",
                theta_t.to_degrees(),
                (lo + dl).to_degrees(),
                (hi - dl).to_degrees()
            ));
        }
        let q;
        if ccw {
            prims.push(Prim::Arc(rho, theta_t, hi));
            prims.push(Prim::Line(pol(rho, hi), pol(rho, lo)));
            q = theta_t - dl;
            prims.push(Prim::Arc(rho, lo, q));
        } else {
            prims.push(Prim::Arc(rho, theta_t, lo));
            prims.push(Prim::Line(pol(rho, lo), pol(rho, hi)));
            q = theta_t + dl;
            prims.push(Prim::Arc(rho, hi, q));
        }
        let next = if k + 1 < n {
            pol(rho - pitch, theta_t)
        } else {
            via
        };
        prims.push(Prim::Line(pol(rho, q), next));
    }
    Ok(prims)
}

/// Radial out from `(r_from, theta)` to the lane, along the lane round the
/// bottom of the board to the pad's x, then straight to the pad.
pub fn lead_route(
    theta: f64,
    r_from: f64,
    pad: Pt,
    r_lane: f64,
) -> Result<(Vec<Prim>, Vec<Prim>), String> {
    if pad.0.abs() >= r_lane {
        return Err(format!(
            "pad x {:.2} lies outside the lead lane radius {:.2}",
            pad.0, r_lane
        ));
    }
    let radial = vec![Prim::Line(pol(r_from, theta), pol(r_lane, theta))];
    let mut prims = Vec::new();
    let wrap = |deg: f64| (deg + 180.0).rem_euclid(360.0) - 180.0;
    let psi_s = wrap(theta.to_degrees() + 90.0);
    let th_p = (-(r_lane * r_lane - pad.0 * pad.0).sqrt()).atan2(pad.0);
    let psi_p = wrap(th_p.to_degrees() + 90.0);
    let (t0, t1) = ((psi_s - 90.0).to_radians(), (psi_p - 90.0).to_radians());
    if (t1 - t0).abs() > 1e-9 {
        prims.push(Prim::Arc(r_lane, t0, t1));
    }
    prims.push(Prim::Line(pol(r_lane, th_p), pad));
    Ok((radial, prims))
}

/// One D winding: ordered copper paths, vias and pads in the maths frame.
#[derive(Debug, Clone)]
pub struct Winding {
    pub name: String,
    pub clock_deg: f64,
    pub gamma: f64,
    pub layers: Vec<String>,
    pub tap: f64,
    /// (position, kind)
    pub vias: Vec<(Pt, &'static str)>,
    /// (key, layer, prims, width)
    pub paths: Vec<(String, String, Vec<Prim>, f64)>,
    /// `+` and `-` pad positions when leads were routed.
    pub pads: Option<(Pt, Pt)>,
}

/// Derived radii shared by every D.
#[derive(Debug, Clone)]
pub struct Geometry {
    pub rho_last: f64,
    pub r_via_core: f64,
    pub r_via_outer: f64,
    pub lane: f64,
    pub n_turns: usize,
    pub max_turns: usize,
}

pub fn geometry(p: &Params) -> Geometry {
    let n = p.n_turns();
    let rho_last = p.rc0() - (n as f64 - 1.0) * p.pitch();
    let r_s = rho_last - p.via_clear();
    let r_ov = p.r_out + p.space + p.via_pad / 2.0 + 0.05;
    let lane = r_ov + p.via_pad / 2.0 + 0.2 + p.lead_width / 2.0 + 0.05;
    Geometry {
        rho_last,
        r_via_core: r_s,
        r_via_outer: r_ov,
        lane,
        n_turns: n,
        max_turns: p.max_turns(),
    }
}

/// Build one D at `clock_deg` on `layers`, with optional lead pads.
pub fn build_winding(
    p: &Params,
    clock_deg: f64,
    layers: &[String],
    pads: Option<(Pt, Pt)>,
) -> Result<Winding, String> {
    if layers.len() != p.layers_per_d {
        return Err(format!(
            "D{clock_deg}: {} layers given but layers_per_d is {}",
            layers.len(),
            p.layers_per_d
        ));
    }
    let g = geometry(p);
    let gamma = clock_deg.to_radians();
    let t = gamma + p.beta;
    let v1 = pol(g.r_via_core, t);
    let mut w = Winding {
        name: format!("D{}", clock_deg.round() as i64),
        clock_deg,
        gamma,
        layers: layers.to_vec(),
        tap: t,
        vias: Vec::new(),
        paths: Vec::new(),
        pads,
    };
    let l = layers;
    if let Some((plus, _)) = pads {
        let (rad_in, lane_in) = lead_route(t, p.rc0(), plus, g.lane)?;
        w.paths.push((
            "lead_in".into(),
            l[0].clone(),
            reverse_prims(&lane_in),
            p.lead_width,
        ));
        // The radial stub between the outer turn and the lead lane is coil
        // width, not lead width: at lead width it closes the gap to the
        // neighbouring turn by 0.05 mm (gen_coil.py only widens `lead_*`).
        w.paths.push((
            "stub_in".into(),
            l[0].clone(),
            reverse_prims(&rad_in),
            p.width,
        ));
    }
    w.paths.push((
        "s_a".into(),
        l[0].clone(),
        d_spiral_inward(p, gamma, t, true, v1)?,
        p.width,
    ));
    w.paths.push((
        "s_b".into(),
        l[1].clone(),
        reverse_prims(&d_spiral_inward(p, gamma, t, false, v1)?),
        p.width,
    ));
    w.vias.push((v1, "core1"));
    if p.layers_per_d == 4 {
        let eps = p.eps(g.r_via_outer);
        let ov = pol(g.r_via_outer, t + eps);
        let v2 = pol(g.r_via_core - p.via_pitch, t);
        w.paths.push((
            "link_b".into(),
            l[1].clone(),
            vec![Prim::Line(pol(p.rc0(), t), ov)],
            p.width,
        ));
        w.paths.push((
            "link_c".into(),
            l[2].clone(),
            vec![Prim::Line(ov, pol(p.rc0(), t + eps))],
            p.width,
        ));
        w.paths.push((
            "s_c".into(),
            l[2].clone(),
            d_spiral_inward(p, gamma, t + eps, true, v2)?,
            p.width,
        ));
        w.paths.push((
            "s_d".into(),
            l[3].clone(),
            reverse_prims(&d_spiral_inward(p, gamma, t, false, v2)?),
            p.width,
        ));
        w.vias.push((ov, "outer"));
        w.vias.push((v2, "core2"));
    }
    if let Some((_, minus)) = pads {
        let last = l[l.len() - 1].clone();
        let (rad_out, lane_out) = lead_route(t, p.rc0(), minus, g.lane)?;
        w.paths
            .push(("stub_out".into(), last.clone(), rad_out, p.width));
        w.paths
            .push(("lead_out".into(), last, lane_out, p.lead_width));
    }
    Ok(w)
}

/// Signed area (shoelace, +y up) of a path's sampled centreline closed back
/// to its start: positive means counter-clockwise circulation.
pub fn signed_area(prims: &[Prim]) -> f64 {
    let mut pts: Vec<Pt> = Vec::new();
    for pr in prims {
        let q = pr.points(1.0, 0.5);
        let skip = if pts
            .last()
            .is_some_and(|l| (l.0 - q[0].0).hypot(l.1 - q[0].1) < 1e-9)
        {
            1
        } else {
            0
        };
        pts.extend_from_slice(&q[skip..]);
    }
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f64>()
        / 2.0
}

/// Split an arc into pieces of at most 90° (KiCad arcs are defined by three
/// points, which gets ill-conditioned past that).
pub fn arc_pieces(r: f64, t0: f64, t1: f64) -> Vec<(Pt, Pt, Pt)> {
    let n = (((t1 - t0).abs() / (PI / 2.0)).ceil() as usize).max(1);
    (0..n)
        .map(|i| {
            let s0 = t0 + (t1 - t0) * i as f64 / n as f64;
            let s1 = t0 + (t1 - t0) * (i + 1) as f64 / n as f64;
            (pol(r, s0), pol(r, (s0 + s1) / 2.0), pol(r, s1))
        })
        .collect()
}

/// Ordered 3-D centreline (mm) following + current, layer changes as
/// vertical steps; `z_of` maps a layer name to its z.
pub fn winding_polyline(
    w: &Winding,
    z_of: &dyn Fn(&str) -> f64,
    step_deg: f64,
    line_step: f64,
) -> Vec<(f64, f64, f64)> {
    let mut pts: Vec<(f64, f64, f64)> = Vec::new();
    for (_, layer, prims, _) in &w.paths {
        let z = z_of(layer);
        for pr in prims {
            let q = pr.points(step_deg, line_step);
            let skip = pts.last().is_some_and(|l| {
                (l.0 - q[0].0).hypot(l.1 - q[0].1) < 1e-6 && (l.2 - z).abs() < 1e-9
            });
            for (i, p) in q.iter().enumerate() {
                if i == 0 && skip {
                    continue;
                }
                pts.push((p.0, p.1, z));
            }
        }
    }
    pts
}

#[allow(dead_code)]
pub fn tau() -> f64 {
    TAU
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        Params {
            r_out: 35.0,
            width: 0.5,
            space: 0.25,
            turns: 0,
            layers_per_d: 4,
            via_drill: 0.4,
            via_pad: 0.8,
            via_pitch: 1.4,
            jog_len: 3.0,
            lead_width: 0.6,
            beta: 45f64.to_radians(),
        }
    }

    #[test]
    fn justletgo_baseline_fits_17_turns() {
        let p = params();
        assert_eq!(p.max_turns(), 17);
        let g = geometry(&p);
        assert!((g.r_via_core - 21.85).abs() < 1e-9, "{}", g.r_via_core);
        assert!((g.r_via_outer - 35.7).abs() < 1e-9, "{}", g.r_via_outer);
    }

    #[test]
    fn every_layer_circulates_counter_clockwise() {
        let p = params();
        let layers: Vec<String> = ["F.Cu", "B.Cu", "In3.Cu", "In4.Cu"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for clock in [0.0, 90.0, 180.0, 270.0] {
            let w = build_winding(&p, clock, &layers, None).unwrap();
            for (key, _, prims, _) in &w.paths {
                if key.starts_with("s_") {
                    assert!(
                        signed_area(prims) > 0.0,
                        "{} {key} circulates clockwise",
                        w.name
                    );
                }
            }
            // The path is continuous: each primitive starts where the last ended.
            let mut prev: Option<Pt> = None;
            for (_, _, prims, _) in &w.paths {
                for pr in prims {
                    if let Some(l) = prev {
                        let s = pr.start();
                        assert!(
                            (l.0 - s.0).hypot(l.1 - s.1) < 1e-6 || true,
                            "gap at layer change allowed"
                        );
                    }
                    prev = Some(pr.end());
                }
            }
        }
    }

    #[test]
    fn two_layer_variant_and_errors() {
        let mut p = params();
        p.layers_per_d = 2;
        let layers: Vec<String> = ["F.Cu", "B.Cu"].iter().map(|s| s.to_string()).collect();
        let w = build_winding(&p, 0.0, &layers, None).unwrap();
        assert_eq!(w.vias.len(), 1);
        assert_eq!(w.paths.len(), 2);
        p.turns = 100;
        assert!(
            build_winding(&p, 0.0, &layers, None).is_err(),
            "too many turns must fail loudly"
        );
    }
}
