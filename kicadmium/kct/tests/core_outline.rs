//! Ports of upstream tests/test_curved_outline_tessellation.py and the
//! board_outline parts of tests/test_routing_outline_bounds.py.

use kct::core::board_outline::{
    board_outline_bounds, board_outline_segments, circle_sagitta, circle_segment_count,
    circle_tessellation_points, routing_curved_chain, OutlineSegments, CIRCLE_MAX_SEGMENTS,
    CIRCLE_MIN_SEGMENTS, CIRCLE_TESSELLATION_MAX_ERROR_MM,
};
use kct::core::outline_tessellation::{
    tessellate_arc, tessellate_arc_with, tessellate_cubic, tessellate_cubic_with, MAX_DEPTH,
    MAX_SEGMENTS,
};
use kct::parse;

type P = (f64, f64);

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn pt_seg(p: P, a: P, b: P) -> f64 {
    kct::geometry::copper::point_segment_distance(p, a, b)
}

fn pt_chain(p: P, chain: &[P]) -> f64 {
    if chain.len() == 1 {
        return (p.0 - chain[0].0).hypot(p.1 - chain[0].1);
    }
    chain
        .windows(2)
        .map(|w| pt_seg(p, w[0], w[1]))
        .fold(f64::INFINITY, f64::min)
}

fn cubic(points: &[P], t: f64) -> P {
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    (
        (0..4).map(|i| w[i] * points[i].0).sum(),
        (0..4).map(|i| w[i] * points[i].1).sum(),
    )
}

/// Symmetric Hausdorff distance between two polylines (vertex-sampled with
/// chord midpoints, sufficient at these densities).
fn hausdorff(a: &[P], b: &[P]) -> f64 {
    let dir = |x: &[P], y: &[P]| {
        let mut m: f64 = 0.0;
        for w in x.windows(2) {
            for t in [0.0, 0.5, 1.0] {
                let p = (
                    w[0].0 + t * (w[1].0 - w[0].0),
                    w[0].1 + t * (w[1].1 - w[0].1),
                );
                m = m.max(pt_chain(p, y));
            }
        }
        m
    };
    dir(a, b).max(dir(b, a))
}

fn chain_length(c: &[P]) -> f64 {
    c.windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .sum()
}

#[test]
fn cubic_preserves_endpoints_and_error() {
    let cases: Vec<Vec<P>> = vec![
        vec![(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)],
        vec![(0.0, 0.0), (10.0, 10.0), (-10.0, -10.0), (1.0, 0.0)],
        vec![(0.0, 0.0), (10.0, 0.0), (-10.0, 0.0), (1.0, 0.0)],
        vec![(0.0, 0.0), (1.0, 1e-10), (2.0, -1e-10), (3.0, 0.0)],
        vec![
            (1e9, 1e9),
            (1e9, 1e9 + 10.0),
            (1e9 + 10.0, 1e9 + 10.0),
            (1e9 + 10.0, 1e9),
        ],
        vec![(2.0, 3.0); 4],
        vec![(0.0, 0.0), (1.0, 2.0), (-1.0, 2.0), (0.0, 0.0)],
    ];
    for points in cases {
        let chain = tessellate_cubic_with(&points, 1e-3, MAX_SEGMENTS, MAX_DEPTH).unwrap();
        assert_eq!(chain[0], points[0]);
        assert_eq!(*chain.last().unwrap(), points[3]);
        let samples: Vec<P> = (0..=2000)
            .map(|i| cubic(&points, i as f64 / 2000.0))
            .collect();
        let worst = samples
            .iter()
            .map(|p| pt_chain(*p, &chain))
            .fold(0.0, f64::max);
        assert!(worst <= 1e-3, "{points:?}: {worst}");
        let dense: Vec<P> = (0..=10000)
            .map(|i| cubic(&points, i as f64 / 10000.0))
            .collect();
        for w in chain.windows(2) {
            let m = ((w[0].0 + w[1].0) / 2.0, (w[0].1 + w[1].1) / 2.0);
            assert!(pt_chain(m, &dense) <= 1e-3 + 1e-9, "{points:?}");
        }
    }
}

#[test]
fn arc_preserves_midpoint_sweep_and_error() {
    let radius = 7.0;
    for angles in [
        [0.0, 45.0, 90.0],
        [0.0, 180.0, 270.0],
        [0.0, -45.0, -90.0],
        [350.0, 360.0, 370.0],
        [10.0, -170.0, -260.0],
    ] {
        let at = |a: f64| {
            (
                13.0 + radius * a.to_radians().cos(),
                -3.0 + radius * a.to_radians().sin(),
            )
        };
        let authored: Vec<P> = angles.iter().map(|a| at(*a)).collect();
        let chain =
            tessellate_arc_with(authored[0], authored[1], authored[2], 1e-4, MAX_SEGMENTS).unwrap();
        assert_eq!(chain[0], authored[0]);
        assert_eq!(*chain.last().unwrap(), authored[2]);
        assert!(chain.contains(&authored[1]));
        let dense: Vec<P> = (0..=10000)
            .map(|i| at(angles[0] + (angles[2] - angles[0]) * i as f64 / 10000.0))
            .collect();
        assert!(hausdorff(&dense, &chain) <= 1e-4, "{angles:?}");
        let expected = radius * (angles[2] - angles[0]).abs().to_radians();
        assert!(approx(chain_length(&chain), expected, 0.002));
    }
}

#[test]
fn resource_caps_refuse_instead_of_weakening_bound() {
    let pts = [(0.0, 0.0), (0.0, 100.0), (100.0, 100.0), (100.0, 0.0)];
    let re = |m: String| m.contains("budget") || m.contains("resource") || m.contains("certif");
    assert!(re(tessellate_cubic_with(&pts, 1e-8, 2, MAX_DEPTH)
        .unwrap_err()
        .0));
    assert!(re(tessellate_cubic_with(&pts, 1e-8, MAX_SEGMENTS, 1)
        .unwrap_err()
        .0));
    assert!(re(tessellate_arc_with(
        (1000.0, 0.0),
        (0.0, 1000.0),
        (-1000.0, 0.0),
        1e-8,
        2
    )
    .unwrap_err()
    .0));
}

#[test]
fn malformed_or_uncertifiable_cubic_refuses() {
    assert!(tessellate_cubic(&[(0.0, 0.0); 3]).is_err());
    assert!(tessellate_cubic(&[(0.0, 0.0), (0.0, f64::NAN), (1.0, 1.0), (2.0, 2.0)]).is_err());
    assert!(tessellate_cubic(&[(0.0, 0.0), (1e300, 1e300), (-1e300, 0.0), (2.0, 2.0)]).is_err());
}

#[test]
fn collinear_arc_refuses() {
    let e = tessellate_arc((0.0, 0.0), (1.0, 0.0), (2.0, 0.0)).unwrap_err();
    assert!(e.0.contains("collinear"));
}

#[test]
fn legacy_parser_preserves_signed_sweep() {
    for sweep in [-270.0f64, -90.0, 90.0, 270.0] {
        let node = parse(&format!(
            "(gr_arc (start 0 0) (end 10 0) (angle {sweep}) (layer \"Edge.Cuts\"))"
        ))
        .unwrap();
        let chain = routing_curved_chain(&node).unwrap();
        assert_eq!(chain[0], (10.0, 0.0));
        let theta = sweep.to_radians();
        let last = *chain.last().unwrap();
        assert!(
            approx(last.0, 10.0 * theta.cos(), 1e-9) && approx(last.1, 10.0 * theta.sin(), 1e-9)
        );
        let signed: f64 = chain
            .windows(2)
            .map(|w| {
                let a = w[1].1.atan2(w[1].0) - w[0].1.atan2(w[0].0);
                (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
            })
            .sum();
        assert!(approx(signed, theta, 1e-9));
    }
}

#[test]
fn legacy_parser_refuses_unrepresentable_sweep() {
    for angle in ["0", "360", "-360", "720", "nan", "90 180"] {
        let node = parse(&format!(
            "(gr_arc (start 0 0) (end 10 0) (angle {angle}) (layer \"Edge.Cuts\"))"
        ))
        .unwrap();
        let e = routing_curved_chain(&node).unwrap_err();
        assert!(e.0.contains("Edge.Cuts"), "{angle}: {e}");
    }
}

#[test]
fn cubic_parser_refuses_malformed_controls() {
    for coords in [
        "",
        "(xy 0 0) (xy 1 1) (xy 2 2)",
        "(xy 0 0) (xy 1) (xy 2 2) (xy 3 3)",
        "(xy 0 0) (xy inf 1) (xy 2 2) (xy 3 3)",
    ] {
        let node = parse(&format!("(gr_curve (pts {coords}) (layer \"Edge.Cuts\"))")).unwrap();
        let e = routing_curved_chain(&node).unwrap_err();
        assert!(e.0.contains("Edge.Cuts"), "{coords}: {e}");
    }
}

#[test]
fn nearly_collinear_arc_retains_authored_midpoint() {
    let chain = tessellate_arc((-1.0, 0.0), (0.0, 1e-6), (1.0, 0.0)).unwrap();
    assert_eq!(chain, vec![(-1.0, 0.0), (0.0, 1e-6), (1.0, 0.0)]);
}

#[test]
fn arc_large_sheet_offset_has_bounded_error() {
    let o = 1e9;
    let chain = tessellate_arc((o + 10.0, o), (o, o + 10.0), (o - 10.0, o)).unwrap();
    let centered: Vec<P> = chain.iter().map(|(x, y)| (x - o, y - o)).collect();
    let dense: Vec<P> = (0..=10000)
        .map(|i| {
            let a = std::f64::consts::PI * i as f64 / 10000.0;
            (10.0 * a.cos(), 10.0 * a.sin())
        })
        .collect();
    assert!(hausdorff(&centered, &dense) <= 1e-4);
}

#[test]
fn arc_default_resource_or_precision_limit_refuses() {
    for r in [1e8, 1e16] {
        let e = tessellate_arc((r, 0.0), (0.0, r), (-r, 0.0)).unwrap_err();
        assert!(e.0.contains("certif") || e.0.contains("budget"), "{e}");
    }
}

#[test]
fn subnormal_scale_arc_preserves_endpoints() {
    let r = 1e-300;
    let chain = tessellate_arc((r, 0.0), (0.0, r), (-r, 0.0)).unwrap();
    assert_eq!(chain, vec![(r, 0.0), (0.0, r), (-r, 0.0)]);
}

// ------------------------------------------------------------------ board_outline

fn root(body: &str) -> kct::SExp {
    parse(&format!("(kicad_pcb {body})")).unwrap()
}

#[test]
fn malformed_or_unsupported_outline_is_explicit() {
    for outline in [
        "(gr_rect (start 0 0) (layer \"Edge.Cuts\"))",
        "(gr_line (start nan 0) (end 12 12) (layer \"Edge.Cuts\"))",
        "(gr_poly (pts (xy 0 0) (xy 12) (xy 12 12)) (layer \"Edge.Cuts\"))",
        "(gr_text \"unsupported\" (at 1 1) (layer \"Edge.Cuts\"))",
        "(gr_arc (start 0 0) (mid 1 1) (end 2 2) (layer \"Edge.Cuts\"))",
    ] {
        let e = board_outline_bounds(&root(outline)).unwrap_err();
        assert!(e.0.contains("Edge.Cuts"), "{outline}: {e}");
    }
}

#[test]
fn missing_outline_is_none() {
    assert_eq!(board_outline_bounds(&root("")).unwrap(), None);
    assert_eq!(
        board_outline_bounds(&root(
            "(gr_rect (start 0 0) (end 10 10) (layer \"F.SilkS\"))"
        ))
        .unwrap(),
        None
    );
    // Nested (footprint) graphics are not outline graphics.
    assert_eq!(
        board_outline_bounds(&root(
            "(footprint \"R\" (fp_rect (start 0 0) (end 9 9) (layer \"Edge.Cuts\")))"
        ))
        .unwrap(),
        None
    );
}

#[test]
fn rect_and_lines_bounds() {
    let b = board_outline_bounds(&root(
        "(gr_rect (start 228.5 155) (end 68.5 55) (layer \"Edge.Cuts\"))",
    ))
    .unwrap()
    .unwrap();
    assert_eq!(b, (68.5, 55.0, 228.5, 155.0));
}

#[test]
fn circle_uses_radius_not_center_end_bbox() {
    let b = board_outline_bounds(&root(
        "(gr_circle (center -4 8) (end -1 12) (layer \"Edge.Cuts\"))",
    ))
    .unwrap()
    .unwrap();
    assert_eq!(b, (-9.0, 3.0, 1.0, 13.0));
}

#[test]
fn arc_includes_extrema_between_control_points() {
    for reverse in [false, true] {
        let mut pts: Vec<P> = [
            std::f64::consts::FRAC_PI_4,
            std::f64::consts::PI,
            7.0 * std::f64::consts::FRAC_PI_4,
        ]
        .iter()
        .map(|a| (10.0 * a.cos(), 10.0 * a.sin()))
        .collect();
        if reverse {
            pts.reverse();
        }
        let body = format!(
            "(gr_arc (start {} {}) (mid {} {}) (end {} {}) (layer \"Edge.Cuts\"))",
            pts[0].0, pts[0].1, pts[1].0, pts[1].1, pts[2].0, pts[2].1
        );
        let b = board_outline_bounds(&root(&body)).unwrap().unwrap();
        let e = (-10.0, -10.0, 50f64.sqrt(), 10.0);
        assert!(
            approx(b.0, e.0, 1e-9)
                && approx(b.1, e.1, 1e-9)
                && approx(b.2, e.2, 1e-9)
                && approx(b.3, e.3, 1e-9),
            "{b:?}"
        );
    }
}

#[test]
fn cubic_bounds_use_curve_extrema() {
    let b = board_outline_bounds(&root(
        "(gr_curve (pts (xy 0 0) (xy 0 10) (xy 10 10) (xy 10 0)) (layer \"Edge.Cuts\"))",
    ))
    .unwrap()
    .unwrap();
    assert_eq!(b, (0.0, 0.0, 10.0, 7.5));
}

#[test]
fn routing_preserves_supported_curved_edge_obstacles() {
    for outline in [
        "(gr_arc (start 0 0) (mid 5 5) (end 10 0) (layer \"Edge.Cuts\"))",
        "(gr_arc (start 0 0) (end 10 0) (angle -90) (layer \"Edge.Cuts\"))",
        "(gr_curve (pts (xy 0 0) (xy 0 10) (xy 10 10) (xy 10 0)) (layer \"Edge.Cuts\"))",
    ] {
        let segs = board_outline_segments(&root(outline)).unwrap();
        assert!(segs.len() > 2, "{outline}");
        assert!(0.0 < segs.max_error_mm && segs.max_error_mm <= 1e-4);
    }
}

#[test]
fn legacy_arc_bounds_preserve_signed_sweep() {
    for (sweep, e) in [
        (270.0, (-10.0, -10.0, 10.0, 10.0)),
        (-90.0, (0.0, -10.0, 10.0, 0.0)),
    ] {
        let b = board_outline_bounds(&root(&format!(
            "(gr_arc (start 0 0) (end 10 0) (angle {sweep}) (layer \"Edge.Cuts\"))"
        )))
        .unwrap()
        .unwrap();
        assert!(
            approx(b.0, e.0, 1e-9)
                && approx(b.1, e.1, 1e-9)
                && approx(b.2, e.2, 1e-9)
                && approx(b.3, e.3, 1e-9),
            "{sweep}: {b:?}"
        );
    }
}

#[test]
fn invalid_legacy_arc_is_rejected() {
    for angle in ["", "(angle nan)", "(angle 90 180)", "(angle 0)"] {
        let e = board_outline_bounds(&root(&format!(
            "(gr_arc (start 0 0) (end 10 0) {angle} (layer \"Edge.Cuts\"))"
        )))
        .unwrap_err();
        assert!(e.0.contains("Malformed Edge.Cuts"), "{angle}: {e}");
    }
}

#[test]
fn circle_tessellation_chord_error_bounded() {
    for radius in [0.05, 1.0, 5.0, 50.0, 500.0, 1200.0] {
        let c = (3.0, -7.0);
        let ring = circle_tessellation_points(c, radius, CIRCLE_TESSELLATION_MAX_ERROR_MM).unwrap();
        assert!(ring.len() >= 12);
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            let m = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            let sag = radius - (m.0 - c.0).hypot(m.1 - c.1);
            assert!((-1e-9..=CIRCLE_TESSELLATION_MAX_ERROR_MM + 1e-9).contains(&sag));
        }
    }
}

#[test]
fn circle_segment_count_rejects_bad_inputs() {
    for r in [0.0, -5.0, f64::NAN] {
        assert!(circle_segment_count(r, CIRCLE_TESSELLATION_MAX_ERROR_MM)
            .unwrap_err()
            .0
            .contains("radius"));
    }
    for e in [0.0, f64::INFINITY] {
        assert!(circle_segment_count(5.0, e)
            .unwrap_err()
            .0
            .contains("max_error"));
    }
    assert_eq!(
        circle_segment_count(1.0, 10.0).unwrap(),
        CIRCLE_MIN_SEGMENTS
    );
}

#[test]
fn circle_segments_form_closed_chain_on_the_true_circle() {
    let (r, c) = (12.5, (2.0, 4.0));
    let segs = board_outline_segments(&root(&format!(
        "(gr_circle (center {} {}) (end {} {}) (layer \"Edge.Cuts\"))",
        c.0,
        c.1,
        c.0 + r,
        c.1
    )))
    .unwrap();
    assert!(segs.len() >= 12);
    for ((x1, y1), (x2, y2)) in segs.iter() {
        assert!(approx((x1 - c.0).hypot(y1 - c.1), r, 1e-9));
        assert!(approx((x2 - c.0).hypot(y2 - c.1), r, 1e-9));
    }
    for i in 0..segs.len() {
        assert_eq!(segs[i].1, segs[(i + 1) % segs.len()].0);
    }
    assert!(segs.max_error_mm > 0.0 && segs.max_error_mm <= CIRCLE_TESSELLATION_MAX_ERROR_MM);
}

#[test]
fn circle_zero_radius_and_malformed_are_rejected() {
    let e = board_outline_segments(&root(
        "(gr_circle (center 5 5) (end 5 5) (layer \"Edge.Cuts\"))",
    ))
    .unwrap_err();
    assert!(e.0.contains("zero-radius"));
    for circle in [
        "(gr_circle (center 5 5) (layer \"Edge.Cuts\"))",
        "(gr_circle (end 5 5) (layer \"Edge.Cuts\"))",
        "(gr_circle (center nan 5) (end 10 5) (layer \"Edge.Cuts\"))",
        "(gr_circle (center 5 5) (end nan 5) (layer \"Edge.Cuts\"))",
    ] {
        let e = board_outline_segments(&root(circle)).unwrap_err();
        assert!(e.0.contains("Malformed Edge.Cuts"), "{circle}: {e}");
    }
}

#[test]
fn straight_outlines_certify_zero_error() {
    let segs = board_outline_segments(&root(
        "(gr_rect (start 0 0) (end 100 80) (layer \"Edge.Cuts\"))",
    ))
    .unwrap();
    assert_eq!(segs.len(), 4);
    assert_eq!(segs.max_error_mm, 0.0);
    let o = OutlineSegments::new(segs.segments.clone(), 0.05);
    assert_eq!(o.max_error_mm, 0.05);
}

#[test]
fn circle_segment_count_honours_the_bound_up_to_the_cap() {
    let r = 1200.0;
    let n = circle_segment_count(r, CIRCLE_TESSELLATION_MAX_ERROR_MM).unwrap();
    assert!((CIRCLE_MIN_SEGMENTS..=CIRCLE_MAX_SEGMENTS).contains(&n));
    assert!(circle_sagitta(r, n).unwrap() <= CIRCLE_TESSELLATION_MAX_ERROR_MM);
    assert!(circle_sagitta(r, n - 1).unwrap() > CIRCLE_TESSELLATION_MAX_ERROR_MM);
}

#[test]
fn circle_segment_count_refuses_radii_beyond_the_cap() {
    for r in [1e4, 1e8] {
        assert!(circle_segment_count(r, CIRCLE_TESSELLATION_MAX_ERROR_MM)
            .unwrap_err()
            .0
            .contains("more than"));
        assert!(
            circle_tessellation_points((0.0, 0.0), r, CIRCLE_TESSELLATION_MAX_ERROR_MM)
                .unwrap_err()
                .0
                .contains("more than")
        );
        let e = board_outline_segments(&root(&format!(
            "(gr_circle (center 0 0) (end {r} 0) (layer \"Edge.Cuts\"))"
        )))
        .unwrap_err();
        assert!(e.0.contains("more than"));
    }
    assert!(circle_segment_count(1e12, 1e-9)
        .unwrap_err()
        .0
        .contains("more than"));
}

#[test]
fn circle_sagitta_matches_the_closed_form() {
    for n in [12usize, 100, 1571, 24336] {
        let naive = 5.0 * (1.0 - (std::f64::consts::PI / n as f64).cos());
        let tol = 1e-12 * n as f64;
        let s = circle_sagitta(5.0, n).unwrap();
        assert!((s - naive).abs() <= tol * naive.abs() + 1e-18, "{n}");
    }
    assert!(circle_sagitta(5.0, 2).is_err());
}

#[test]
fn closed_collinear_curve_does_not_expand_board() {
    for coords in [
        "(xy 1500 100) (xy 1500 100) (xy 1500 100) (xy 1500 100)",
        "(xy 1500 100) (xy 1500 101) (xy 1500 102) (xy 1500 100)",
        "(xy 1500 100) (xy 1501 101) (xy 1502 102) (xy 1500 100)",
    ] {
        let text = format!(
            "(gr_rect (start 68.5 55) (end 88.5 75) (layer \"Edge.Cuts\")) \
             (gr_curve (pts {coords}) (layer \"Edge.Cuts\"))"
        );
        let r = root(&text);
        assert_eq!(
            board_outline_bounds(&r).unwrap().unwrap(),
            (68.5, 55.0, 88.5, 75.0)
        );
        assert!(board_outline_segments(&r)
            .unwrap()
            .iter()
            .any(|(a, _)| a.0 >= 1500.0));
    }
}

#[test]
fn small_closed_and_open_collinear_curves_keep_bounds() {
    for coords in [
        "(xy 1500 100) (xy 1500 101) (xy 1500.000001 101) (xy 1500 100)",
        "(xy 1500 100) (xy 1500 101) (xy 1500 102) (xy 1500 103)",
    ] {
        let r = root(&format!("(gr_curve (pts {coords}) (layer \"Edge.Cuts\"))"));
        assert!(board_outline_bounds(&r).unwrap().is_some());
    }
}

#[test]
fn gr_poly_outline_edges() {
    let r = root(
        "(gr_poly (pts (xy 68.5 55) (xy 98.5 55) (xy 98.5 75) (xy 68.5 75)) (layer \"Edge.Cuts\"))",
    );
    assert_eq!(
        board_outline_bounds(&r).unwrap().unwrap(),
        (68.5, 55.0, 98.5, 75.0)
    );
    let segs = board_outline_segments(&r).unwrap();
    assert_eq!(segs.len(), 4);
    assert_eq!(segs[3], ((68.5, 75.0), (68.5, 55.0)));
}
