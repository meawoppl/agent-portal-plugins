use kct::region::{query, Bounds};
fn board(body: &str) -> String {
    format!(
        r#"(kicad_pcb (layers (0 "F.Cu" signal) (2 "In1.Cu" signal) (31 "B.Cu" signal)) {body})"#
    )
}
fn bounds() -> Bounds {
    Bounds {
        x0: -1.,
        y0: -1.,
        x1: 1.,
        y1: 1.,
    }
}
#[test]
fn crossing_copper_and_spanning_zone_are_included() {
    let src = board(
        r#"(segment (start -10 0) (end 10 0) (width 0.2) (layer "F.Cu") (net "N") (uuid "t")) (via (at 0 0) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net "N") (uuid "v")) (zone (net "N") (layer "F.Cu") (uuid "z") (polygon (pts (xy -20 -20) (xy 20 -20) (xy 20 20) (xy -20 20))))"#,
    );
    let all = query(&src, bounds(), &[], &[], 50).unwrap();
    assert_eq!(all.counts["track"], 1);
    assert_eq!(all.counts["zone"], 1);
    assert_eq!(all.counts["via"], 1);
    let inner = query(&src, bounds(), &["In1.Cu".into()], &["N".into()], 50).unwrap();
    assert_eq!(inner.items.len(), 1);
    assert_eq!(inner.items[0]["kind"], "via");
    assert!(query(&src, bounds(), &[], &["OTHER".into()], 50)
        .unwrap()
        .items
        .is_empty());
}
#[test]
fn rotated_pad_and_courtyard_select_footprint_outside_origin() {
    let src = board(
        r#"(footprint "Test" (layer "F.Cu") (at 0 10 90) (uuid "u") (property "Reference" "U1") (property "Value" "chip") (pad "1" smd rect (at 10 0 90) (size 1 1) (layers "F.Cu") (net "N") (uuid "p")) (fp_rect (start 9 -1) (end 11 1) (layer "F.CrtYd") (stroke (width 0.05) (type default))))"#,
    );
    let r = query(&src, bounds(), &[], &[], 50).unwrap();
    assert_eq!(r.counts["footprint"], 1);
    assert_eq!(r.counts["pad"], 1);
    assert!(
        r.items.iter().find(|i| i["kind"] == "footprint").unwrap()["courtyard_bbox"].is_array()
    );
}
#[test]
fn table_is_bounded_but_counts_are_complete() {
    let mut body = String::new();
    for i in 0..80 {
        body += &format!(
            r#"(via (at 0 0) (size 0.5) (drill 0.25) (layers "F.Cu" "B.Cu") (net "N") (uuid "v{i}"))"#
        );
    }
    let r = query(&board(&body), bounds(), &[], &[], 50).unwrap();
    assert_eq!(r.counts["via"], 80);
    assert_eq!(r.omitted, 30);
    assert!(r.table().lines().count() < 60);
    assert_eq!(
        query(&board(&body), bounds(), &[], &[], 0)
            .unwrap()
            .items
            .len(),
        80
    );
}
#[test]
fn bad_bounds_are_rejected() {
    assert!(query(&board(""), Bounds { x0: 2., ..bounds() }, &[], &[], 50).is_err());
}
#[test]
fn footprint_positions_use_sheet_coordinates_with_offset_outline() {
    let s = board(
        r#"(gr_rect (start 100 100) (end 200 200) (stroke (width 0.05) (type default)) (fill none) (layer "Edge.Cuts")) (footprint "IC" (layer "F.Cu") (at 120 130) (uuid "fp") (property "Reference" "U1"))"#,
    );
    let r = query(
        &s,
        Bounds {
            x0: 119.,
            y0: 129.,
            x1: 121.,
            y1: 131.,
        },
        &[],
        &[],
        0,
    )
    .unwrap();
    assert_eq!(r.items[0]["at"]["x"], 120.);
    assert_eq!(r.items[0]["at"]["y"], 130.);
}
