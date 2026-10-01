//! Ports of upstream tests for core.types / core.layers / core.geometry /
//! geometry.copper (test_clearance_via_barrel_layers.py helper unit tests,
//! test_rotation_convention.py, test_geometry_copper.py).

use geo::{Area, Intersects};
use kct::core::geometry::{
    point_to_segment_distance, rotate_pad_offset, segment_clearance, segment_to_segment_distance,
    segments_intersect,
};
use kct::core::layers::{
    is_declared_copper_layer, validate_copper_layer, via_spans_layer, COPPER_LAYER_ORDER,
};
use kct::core::severity::SeverityMixin;
use kct::core::types::{CopperLayer, ErcSeverity, Layer, LayoutStyle, RiskLevel, Severity};
use kct::geometry::copper::{
    point_segment_distance, segment_centerline_distance, segment_copper_polygon,
    segments_copper_touch,
};

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

// ------------------------------------------------------------------ types

#[test]
fn severity_from_string() {
    assert_eq!(Severity::from_string("Error"), Severity::Error);
    assert_eq!(Severity::from_string(" WARNING "), Severity::Warning);
    assert_eq!(Severity::from_string("info"), Severity::Info);
    assert_eq!(Severity::from_string("bogus"), Severity::Info);
    assert_eq!(
        Severity::from_string_or("bogus", Some(Severity::Warning)),
        Severity::Warning
    );
    assert_eq!(ErcSeverity::from_string("excluded"), ErcSeverity::Exclusion);
    assert_eq!(ErcSeverity::from_string("info"), ErcSeverity::Exclusion);
    assert_eq!(ErcSeverity::from_string("error"), ErcSeverity::Error);
    assert_eq!(Severity::Error.to_string(), "error");
    assert_eq!(
        serde_json::to_string(&Severity::Warning).unwrap(),
        "\"warning\""
    );
    assert_eq!(
        serde_json::from_str::<ErcSeverity>("\"exclusion\"").unwrap(),
        ErcSeverity::Exclusion
    );
}

#[test]
fn risk_layer_and_style() {
    assert_eq!(
        RiskLevel::from_string("CRITICAL", None),
        RiskLevel::Critical
    );
    assert_eq!(RiskLevel::from_string("nope", None), RiskLevel::Low);
    assert_eq!(
        RiskLevel::from_string("nope", Some(RiskLevel::High)),
        RiskLevel::High
    );
    assert_eq!(Layer::from_string("F.Cu").unwrap(), Layer::FCu);
    assert!(Layer::from_string("X.Cu")
        .unwrap_err()
        .to_string()
        .contains("Unknown KiCad layer name: X.Cu"));
    assert!(Layer::In2Cu.is_copper() && !Layer::In2Cu.is_outer());
    assert!(Layer::BCu.is_outer() && Layer::BCu.is_back() && !Layer::BCu.is_front());
    assert!(!Layer::EdgeCuts.is_copper());
    assert_eq!(Layer::copper_layers()[5], Layer::BCu);
    assert_eq!(
        CopperLayer::from_kicad_name("In3.Cu").unwrap(),
        CopperLayer::In3Cu
    );
    assert_eq!(CopperLayer::In3Cu.value(), 3);
    assert_eq!(CopperLayer::BCu.to_layer(), Layer::BCu);
    assert!(CopperLayer::FCu.is_outer());
    assert!(CopperLayer::from_kicad_name("F.SilkS").is_err());
    assert_eq!(LayoutStyle::Physical.as_str(), "physical");
}

// ------------------------------------------------------------------ layers

#[test]
fn via_spans_layer_cases() {
    let through = ["F.Cu", "B.Cu"];
    for l in ["F.Cu", "In1.Cu", "In2.Cu", "In30.Cu", "B.Cu"] {
        assert!(via_spans_layer(&through, l));
    }
    let blind = ["F.Cu", "In1.Cu"];
    assert!(via_spans_layer(&blind, "F.Cu"));
    assert!(via_spans_layer(&blind, "In1.Cu"));
    assert!(!via_spans_layer(&blind, "In2.Cu"));
    assert!(!via_spans_layer(&blind, "B.Cu"));
    let buried = ["In1.Cu", "In3.Cu"];
    assert!(!via_spans_layer(&buried, "F.Cu"));
    for l in ["In1.Cu", "In2.Cu", "In3.Cu"] {
        assert!(via_spans_layer(&buried, l));
    }
    assert!(!via_spans_layer(&buried, "In4.Cu"));
    assert!(!via_spans_layer(&buried, "B.Cu"));
    assert!(via_spans_layer(&["B.Cu", "F.Cu"], "In2.Cu"));
    assert!(!via_spans_layer(&["F.Cu", "B.Cu"], "Edge.Cuts"));
    assert!(!via_spans_layer(&["Edge.Cuts"], "In1.Cu"));
    assert!(!via_spans_layer::<&str>(&[], "F.Cu"));
    assert!(via_spans_layer(&["In1.Cu"], "In1.Cu"));
    assert!(!via_spans_layer(&["In1.Cu"], "In2.Cu"));
    assert_eq!(COPPER_LAYER_ORDER[0], "F.Cu");
    assert_eq!(COPPER_LAYER_ORDER[31], "B.Cu");
    assert_eq!(COPPER_LAYER_ORDER.len(), 32);
    assert_eq!(COPPER_LAYER_ORDER[17], "In17.Cu");
}

#[test]
fn validate_and_declared_copper_layers() {
    assert!(validate_copper_layer("F.Cu", ["F.Cu", "B.Cu"]).is_ok());
    let e = validate_copper_layer("F.SilkS", ["F.Cu"]).unwrap_err();
    assert!(e
        .to_string()
        .starts_with("'F.SilkS' is not a valid copper layer name."));
    let e = validate_copper_layer("In1.Cu", ["B.Cu", "F.Cu"]).unwrap_err();
    assert_eq!(
        e.to_string(),
        "Layer 'In1.Cu' is not available on this board. Declared copper layers: F.Cu, B.Cu."
    );
    assert!(validate_copper_layer("F.Cu", Vec::<String>::new()).is_err());
    assert!(is_declared_copper_layer("In2.Cu", "mixed"));
    assert!(is_declared_copper_layer("B.Cu", "jumper"));
    assert!(!is_declared_copper_layer("B.Cu", "user"));
    assert!(!is_declared_copper_layer("User.1", "signal"));
}

// ------------------------------------------------------------------ core.geometry

const FP_POS: (f64, f64) = (100.0, 100.0);
const PAD_LOCAL: (f64, f64) = (2.0, 0.0);
const ORACLE: [(f64, (f64, f64)); 4] = [
    (0.0, (102.0, 100.0)),
    (90.0, (100.0, 98.0)),
    (180.0, (98.0, 100.0)),
    (270.0, (100.0, 102.0)),
];

#[test]
fn rotate_pad_offset_matches_pcbnew_oracle() {
    for (rot, expected) in ORACLE {
        let (rx, ry) = rotate_pad_offset(PAD_LOCAL.0, PAD_LOCAL.1, rot);
        let actual = (FP_POS.0 + rx, FP_POS.1 + ry);
        assert!(approx(actual.0, expected.0, 1e-6), "deg{rot}");
        assert!(approx(actual.1, expected.1, 1e-6), "deg{rot}");
    }
}

#[test]
fn rotation_90_and_270_discriminate_standard_ccw() {
    for (rot, oracle) in [ORACLE[1], ORACLE[3]] {
        let rad = rot.to_radians();
        let ccw = (
            FP_POS.0 + PAD_LOCAL.0 * rad.cos() - PAD_LOCAL.1 * rad.sin(),
            FP_POS.1 + PAD_LOCAL.0 * rad.sin() + PAD_LOCAL.1 * rad.cos(),
        );
        assert!(!(approx(ccw.0, oracle.0, 0.5) && approx(ccw.1, oracle.1, 0.5)));
    }
}

#[test]
fn rotate_pad_offset_inverse() {
    let (x, y) = rotate_pad_offset(1.25, -0.5, 37.0);
    let (bx, by) = rotate_pad_offset(x, y, -37.0);
    assert!(approx(bx, 1.25, 1e-12) && approx(by, -0.5, 1e-12));
}

#[test]
fn core_segment_distances() {
    assert_eq!(
        point_to_segment_distance(5.0, 2.0, 0.0, 0.0, 10.0, 0.0),
        2.0
    );
    assert_eq!(
        point_to_segment_distance(13.0, 4.0, 0.0, 0.0, 10.0, 0.0),
        5.0
    );
    assert_eq!(point_to_segment_distance(3.0, 4.0, 0.0, 0.0, 0.0, 0.0), 5.0);
    // Translation invariance (#3714): exact differences.
    let base = point_to_segment_distance(0.3, 0.7, 0.1, 0.2, 1.1, 0.9);
    let shifted = point_to_segment_distance(1000.3, 0.7, 1000.1, 0.2, 1001.1, 0.9);
    assert!(approx(base, shifted, 1e-9));
    assert!(segments_intersect(-1.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 1.0));
    // Shared endpoint is not a proper intersection.
    assert!(!segments_intersect(0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0));
    assert_eq!(
        segment_to_segment_distance(-1.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 1.0),
        0.0
    );
    assert_eq!(
        segment_to_segment_distance(0.0, 0.0, 10.0, 0.0, 0.0, 3.0, 10.0, 3.0),
        3.0
    );
    assert!(approx(
        segment_clearance(0.0, 0.0, 10.0, 0.0, 0.2, 0.0, 1.0, 10.0, 1.0, 0.4),
        0.7,
        1e-12
    ));
}

// ------------------------------------------------------------------ geometry.copper

#[test]
fn segment_polygon_has_expected_area() {
    let poly = segment_copper_polygon((0.0, 0.0), (10.0, 0.0), 0.25);
    let expected = 10.0 * 0.25 + std::f64::consts::PI * 0.125f64.powi(2);
    assert!(((poly.unsigned_area() - expected) / expected).abs() < 1e-3);
}

#[test]
fn touching_and_apart_segments() {
    let a = segment_copper_polygon((0.0, 0.0), (5.0, 0.0), 0.25);
    let b = segment_copper_polygon((5.0, 0.0), (5.0, 5.0), 0.25);
    assert!(a.intersects(&b));
    let a = segment_copper_polygon((0.0, 0.0), (5.0, 0.0), 0.001);
    let b = segment_copper_polygon((5.0, 0.009), (5.0, 5.0), 0.001);
    assert!(!a.intersects(&b));
}

#[test]
fn degenerate_segment_geometry() {
    let line = segment_copper_polygon((0.0, 0.0), (10.0, 0.0), 0.0);
    assert!(matches!(line, geo::Geometry::LineString(_)));
    assert_eq!(line.unsigned_area(), 0.0);
    let pt = segment_copper_polygon((1.0, 1.0), (1.0, 1.0), 0.0);
    assert!(matches!(pt, geo::Geometry::Point(_)));
    assert_eq!(pt.unsigned_area(), 0.0);
    let disk = segment_copper_polygon((1.0, 1.0), (1.0, 1.0), 0.5);
    let expected = std::f64::consts::PI * 0.25f64.powi(2);
    assert!(((disk.unsigned_area() - expected) / expected).abs() < 1e-2);
}

#[test]
fn point_segment_distance_clamps() {
    assert!(approx(
        point_segment_distance((5.0, 2.0), (0.0, 0.0), (10.0, 0.0)),
        2.0,
        1e-12
    ));
    assert!(approx(
        point_segment_distance((13.0, 4.0), (0.0, 0.0), (10.0, 0.0)),
        5.0,
        1e-12
    ));
    assert!(approx(
        point_segment_distance((3.0, 4.0), (0.0, 0.0), (0.0, 0.0)),
        5.0,
        1e-12
    ));
}

#[test]
fn segment_centerline_distance_cases() {
    type Seg = ((f64, f64), (f64, f64));
    let cases: [(Seg, Seg, f64); 6] = [
        (((-5.0, 0.0), (5.0, 0.0)), ((0.0, -5.0), (0.0, 5.0)), 0.0),
        (((0.0, 0.0), (10.0, 0.0)), ((5.0, 0.0), (5.0, 5.0)), 0.0),
        (((0.0, 0.0), (10.0, 0.0)), ((4.0, 0.0), (6.0, 0.0)), 0.0),
        (((0.0, 0.0), (4.0, 0.0)), ((6.0, 0.0), (10.0, 0.0)), 2.0),
        (((0.0, 0.0), (10.0, 0.0)), ((0.0, 3.0), (10.0, 3.0)), 3.0),
        (((0.0, 0.0), (10.0, 0.0)), ((5.0, 1.5), (5.0, 5.0)), 1.5),
    ];
    for (a, b, expected) in cases {
        let d = segment_centerline_distance(a.0, a.1, b.0, b.1);
        assert!(approx(d, expected, 1e-12), "{a:?} {b:?}");
        // Agrees with geo's Euclidean distance between the LineStrings.
        use geo::{Distance, Euclidean, Line};
        let gd = Euclidean.distance(&Line::from([a.0, a.1]), &Line::from([b.0, b.1]));
        assert!(approx(d, gd, 1e-9));
    }
}

#[test]
fn segments_copper_touch_uses_full_width() {
    let (s, e, w) = ((0.0, 0.0), (10.0, 0.0), 0.6);
    assert!(segments_copper_touch(s, e, w, (5.0, 0.3), (5.0, 5.0), 0.4));
    assert!(!segments_copper_touch(
        s,
        e,
        w,
        (5.0, 0.501),
        (5.0, 5.0),
        0.4
    ));
    assert!(segments_copper_touch(s, e, w, (5.0, 0.5), (5.0, 5.0), 0.4));
}

#[test]
fn segments_copper_touch_agrees_with_buffered_polygons() {
    let cases = [
        ((0.0, 0.0), (10.0, 0.0), 0.6, (5.0, 0.0), (5.0, 5.0), 0.4),
        ((0.0, 0.0), (10.0, 0.0), 0.6, (3.0, 0.5), (7.0, 0.5), 0.6),
        ((0.0, 0.0), (10.0, 0.0), 0.6, (5.0, 0.8), (9.0, 0.8), 0.6),
        ((0.0, 0.0), (10.0, 0.0), 0.6, (5.0, 5.0), (9.0, 5.0), 0.6),
        ((0.0, 0.0), (0.0, 0.0), 0.4, (0.1, 0.0), (5.0, 0.0), 0.2),
        ((0.0, 0.0), (0.0, 0.0), 0.4, (0.4, 0.0), (5.0, 0.0), 0.2),
    ];
    for (a0, a1, aw, b0, b1, bw) in cases {
        let pa = segment_copper_polygon(a0, a1, aw);
        let pb = segment_copper_polygon(b0, b1, bw);
        if pa.intersects(&pb) {
            assert!(segments_copper_touch(a0, a1, aw, b0, b1, bw));
        }
    }
}

#[test]
fn segments_copper_touch_invariant_under_collinear_splits() {
    let (ts, te, tw) = ((0.0, 0.0), (10.0, 0.0), 0.6);
    for (branch, expected) in [
        (((5.0, 0.0), (5.0, 5.0)), true),
        (((5.0, 0.601), (5.0, 5.0)), false),
    ] {
        assert_eq!(
            segments_copper_touch(ts, te, tw, branch.0, branch.1, 0.6),
            expected
        );
        for parts in [2, 3, 7] {
            let touched = (0..parts).any(|i| {
                segments_copper_touch(
                    (10.0 * i as f64 / parts as f64, 0.0),
                    (10.0 * (i + 1) as f64 / parts as f64, 0.0),
                    tw,
                    branch.0,
                    branch.1,
                    0.6,
                )
            });
            assert_eq!(touched, expected, "{parts}-way split");
        }
    }
}
