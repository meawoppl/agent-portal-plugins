//! Courtyard / package-body geometry (geometry.courtyard,
//! geometry.package_body). Upstream exercises these through
//! schema.pcb-based placement/DRC tests; these synthetic cases pin the same
//! resolution rules.

use geo::{Area, Contains};
use kct::geometry::courtyard::{
    chain_lines, courtyard_polygon, courtyard_side, has_courtyard_geometry, side_has_geometry,
    SimpleFootprint,
};
use kct::geometry::package_body::{
    footprint_side, package_body_polygon, polygonize, BODY_SOURCE_COURTYARD, BODY_SOURCE_FAB,
};
use kct::parse;

fn fp(text: &str) -> SimpleFootprint {
    SimpleFootprint::from_sexp(&parse(text).unwrap())
}

#[test]
fn rect_courtyard_rotated_into_board_frame() {
    let f = fp(r#"(footprint "X" (layer "F.Cu") (at 10 20 90)
        (fp_rect (start -2 -1) (end 2 1) (layer "F.CrtYd")))"#);
    let poly = courtyard_polygon(&f, "F").unwrap();
    assert!((poly.unsigned_area() - 8.0).abs() < 1e-9);
    // KiCad negated rotation: local (2, 0) -> board (10, 18).
    assert!(poly.contains(&geo::Point::new(10.0, 18.5)));
    assert!(!poly.contains(&geo::Point::new(11.5, 20.0)));
    assert!(courtyard_polygon(&f, "B").is_none());
    assert!(has_courtyard_geometry(&f));
    assert!(side_has_geometry(&f, "F") && !side_has_geometry(&f, "B"));
    assert_eq!(courtyard_side("B.CrtYd"), Some("B"));
    assert_eq!(courtyard_side("F.SilkS"), None);
}

#[test]
fn line_chain_courtyard() {
    let f = fp(r#"(footprint "X" (layer "B.Cu") (at 0 0)
        (fp_line (start 0 0) (end 4 0) (layer "B.CrtYd"))
        (fp_line (start 4 3) (end 4 0.0005) (layer "B.CrtYd"))
        (fp_line (start 4 3) (end 0 3) (layer "B.CrtYd"))
        (fp_line (start 0 3) (end 0 0) (layer "B.CrtYd")))"#);
    let poly = courtyard_polygon(&f, "B").unwrap();
    assert!((poly.unsigned_area() - 12.0).abs() < 1e-2);
    // Open chain does not resolve.
    let open = fp(r#"(footprint "X" (at 0 0)
        (fp_line (start 0 0) (end 4 0) (layer "F.CrtYd"))
        (fp_line (start 4 0) (end 4 3) (layer "F.CrtYd")))"#);
    assert!(courtyard_polygon(&open, "F").is_none());
    assert!(chain_lines(&[]).is_none());
}

#[test]
fn poly_courtyard_and_self_intersection_repair() {
    let f = fp(r#"(footprint "X" (at 0 0)
        (fp_poly (pts (xy 0 0) (xy 2 0) (xy 2 2) (xy 0 2)) (layer "F.CrtYd")))"#);
    assert!((courtyard_polygon(&f, "F").unwrap().unsigned_area() - 4.0).abs() < 1e-9);
    // Bow-tie: repaired, not dropped.
    let bow = fp(r#"(footprint "X" (at 0 0)
        (fp_poly (pts (xy 0 0) (xy 2 2) (xy 2 0) (xy 0 2)) (layer "F.CrtYd")))"#);
    let p = courtyard_polygon(&bow, "F").unwrap();
    assert!(p.unsigned_area() > 0.0);
}

#[test]
fn package_body_prefers_largest_fab_shape_then_courtyard() {
    let f = fp(r#"(footprint "QFN" (layer "F.Cu") (at 50 50)
        (fp_rect (start -3 -3) (end 3 3) (layer "F.CrtYd"))
        (fp_poly (pts (xy -2 -2) (xy 2 -2) (xy 2 2) (xy -2 2)) (layer "F.Fab"))
        (fp_circle (center -1.5 -1.5) (end -1.3 -1.5) (layer "F.Fab")))"#);
    let (poly, src) = package_body_polygon(&f, true).unwrap();
    assert_eq!(src, BODY_SOURCE_FAB);
    assert!((poly.unsigned_area() - 16.0).abs() < 1e-9);

    let f = fp(r#"(footprint "R" (layer "B.Cu") (at 0 0)
        (fp_rect (start -1 -1) (end 1 1) (layer "B.CrtYd"))
        (fp_rect (start -5 -5) (end 5 5) (layer "F.Fab")))"#);
    assert_eq!(footprint_side(&f), "B");
    let (poly, src) = package_body_polygon(&f, true).unwrap();
    assert_eq!(src, BODY_SOURCE_COURTYARD);
    assert!((poly.unsigned_area() - 4.0).abs() < 1e-9);
    assert!(package_body_polygon(&f, false).is_none());
}

#[test]
fn package_body_polygonizes_fab_lines_and_arcs() {
    // Chamfered body from lines plus one arc corner.
    let f = fp(r#"(footprint "U" (layer "F.Cu") (at 0 0)
        (fp_line (start -2 -1) (end 2 -1) (layer "F.Fab"))
        (fp_line (start 2 -1) (end 2 1) (layer "F.Fab"))
        (fp_line (start 2 1) (end -1 1) (layer "F.Fab"))
        (fp_arc (start -1 1) (mid -1.7071 0.7071) (end -2 0) (layer "F.Fab"))
        (fp_line (start -2 0) (end -2 -1) (layer "F.Fab"))
        (fp_line (start 5 5) (end 6 6) (layer "F.Fab")))"#);
    let (poly, src) = package_body_polygon(&f, true).unwrap();
    assert_eq!(src, BODY_SOURCE_FAB);
    let area = poly.unsigned_area();
    assert!(area > 7.0 && area < 8.0, "{area}");
}

#[test]
fn polygonize_faces_and_holes() {
    let sq = |x0: f64, y0: f64, s: f64| {
        vec![
            vec![(x0, y0), (x0 + s, y0)],
            vec![(x0 + s, y0), (x0 + s, y0 + s)],
            vec![(x0 + s, y0 + s), (x0, y0 + s)],
            vec![(x0, y0 + s), (x0, y0)],
        ]
    };
    // Two adjacent faces sharing an edge.
    let mut lines = sq(0.0, 0.0, 2.0);
    lines.push(vec![(1.0, 0.0), (1.0, 2.0)]);
    // Note: the shared edge's endpoints are not nodes of the square's
    // edges, so (like shapely without noding) only the outer square forms.
    let faces = polygonize(&lines);
    assert_eq!(faces.len(), 1);

    // Properly noded split square -> two faces.
    let lines = vec![
        vec![(0.0, 0.0), (1.0, 0.0)],
        vec![(1.0, 0.0), (2.0, 0.0)],
        vec![(2.0, 0.0), (2.0, 2.0)],
        vec![(2.0, 2.0), (1.0, 2.0)],
        vec![(1.0, 2.0), (0.0, 2.0)],
        vec![(0.0, 2.0), (0.0, 0.0)],
        vec![(1.0, 0.0), (1.0, 2.0)],
    ];
    let mut areas: Vec<f64> = polygonize(&lines)
        .iter()
        .map(|p| p.unsigned_area())
        .collect();
    areas.sort_by(f64::total_cmp);
    assert_eq!(areas, vec![2.0, 2.0]);

    // Nested square becomes a hole of the outer face (and its own face).
    let mut lines = sq(0.0, 0.0, 10.0);
    lines.extend(sq(4.0, 4.0, 2.0));
    let mut areas: Vec<f64> = polygonize(&lines)
        .iter()
        .map(|p| p.unsigned_area())
        .collect();
    areas.sort_by(f64::total_cmp);
    assert_eq!(areas, vec![4.0, 96.0]);
}
