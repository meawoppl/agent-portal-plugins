use pcb_lint::{
    Config, lint,
    model::{Board, Point, pad_distance},
    review,
};
fn board(body: &str) -> String {
    format!(
        "(kicad_pcb (version 20260206) (layers (0 \"F.Cu\" signal) (4 \"In1.Cu\" signal) (2 \"B.Cu\" signal)) {body})"
    )
}
fn seg(id: &str, a: (f64, f64), b: (f64, f64), w: f64, layer: &str) -> String {
    format!(
        "(segment (start {} {}) (end {} {}) (width {w}) (layer \"{layer}\") (net \"N\") (uuid \"{id}\"))",
        a.0, a.1, b.0, b.1
    )
}
fn via(id: &str, x: f64, y: f64) -> String {
    format!(
        "(via (at {x} {y}) (size 0.5) (drill 0.25) (layers \"F.Cu\" \"B.Cu\") (net \"N\") (uuid \"{id}\"))"
    )
}
fn report(s: &str) -> pcb_lint::Report {
    lint(&board(s), "test-board", Config::default()).unwrap()
}
fn count(r: &pcb_lint::Report, rule: &str) -> usize {
    r.findings.iter().filter(|f| f.rule == rule).count()
}
#[test]
fn straight_vs_off_angle() {
    assert_eq!(
        count(
            &report(&seg("a", (0., 0.), (10., 10.), 0.3, "F.Cu")),
            "trace.off_angle"
        ),
        0
    );
    assert_eq!(
        count(
            &report(&seg("a", (0., 0.), (10., 3.), 0.3, "F.Cu")),
            "trace.off_angle"
        ),
        1
    );
}
#[test]
fn reversed_duplicates() {
    let s = seg("a", (0., 0.), (1., 1.), 0.3, "F.Cu") + &seg("b", (1., 1.), (0., 0.), 0.3, "F.Cu");
    assert_eq!(count(&report(&s), "trace.duplicate"), 1);
}
#[test]
fn layer_separation() {
    let s = seg("a", (0., 0.), (1., 1.), 0.3, "F.Cu") + &seg("b", (1., 1.), (0., 0.), 0.3, "B.Cu");
    let r = report(&s);
    assert_eq!(count(&r, "trace.duplicate"), 0);
    assert_eq!(count(&r, "trace.open_end"), 4);
}
#[test]
fn tee_midsegment_attached() {
    let s = seg("a", (0., 0.), (10., 0.), 0.3, "F.Cu") + &seg("b", (5., 0.), (5., 5.), 0.3, "F.Cu");
    assert_eq!(count(&report(&s), "trace.open_end"), 3);
}
#[test]
fn rotated_back_pad_is_not_double_mirrored() {
    let b=Board::read(&board(r#"(footprint "X" (layer "B.Cu") (at 10 20 90) (uuid "f") (property "Reference" "J1") (pad "1" smd rect (at 2 1 90) (size 4 1) (layers "B.Cu") (net "N") (uuid "p")))"#)).unwrap();
    let p = &b.pads[0];
    assert!(p.at.distance(Point { x: 11., y: 18. }) < 1e-9);
    assert_eq!(pad_distance(Point { x: 11., y: 19.5 }, p), 0.);
    assert!(pad_distance(Point { x: 12., y: 18. }, p) > 0.4);
}
#[test]
fn old_net_codes_and_timestamp() {
    let b=Board::read(&board(r#"(net 0 "") (net 1 "GND") (segment (start 0 0) (end 1 0) (width 0.3) (layer "F.Cu") (net 1) (tstamp "legacy"))"#)).unwrap();
    assert_eq!(b.tracks[0].net, "GND");
    assert_eq!(b.tracks[0].id, "legacy");
    assert!(b.tracks[0].stable);
}
#[test]
fn malformed_nonfinite_and_depth_fail() {
    for s in ["(kicad_pcb", "(kicad_pcb) junk", "((())))"] {
        assert!(Board::read(s).is_err());
    }
    assert!(Board::read(&board(&seg("a", (f64::NAN, 0.), (1., 0.), 0.3, "F.Cu"))).is_err());
    assert!(pcb_lint::model::parse(&"(".repeat(150)).is_err());
}
#[test]
fn escapes_and_unicode() {
    let s = pcb_lint::model::parse(r#"(property "Value" "µF \"quote\" \\ path")"#).unwrap();
    assert_eq!(s.val(2), "µF \"quote\" \\ path");
}
#[test]
fn fallback_identity_cannot_ignore() {
    let s = seg("a", (0., 0.), (10., 3.), 0.3, "F.Cu").replace("(uuid \"a\")", "");
    let r = report(&s);
    let f = r
        .findings
        .iter()
        .find(|f| f.rule == "trace.off_angle")
        .unwrap();
    assert!(!f.stable_identity);
    let d = tempfile::tempdir().unwrap();
    assert!(
        review::update(
            &d.path().join("r.json"),
            &r,
            &f.key,
            "ignore",
            "intentional",
            "tester",
            None
        )
        .is_err()
    );
}
#[test]
fn identity_survives_format_order_and_endpoint_reversal() {
    let a = seg("a", (0., 0.), (10., 3.), 0.3, "F.Cu");
    let b = seg("b", (10., 3.), (11., 3.), 0.3, "F.Cu");
    let r = report(&(a + &b));
    let q =
        report(&(b + &seg("a", (10., 3.), (0., 0.), 0.3, "F.Cu")).replace("(width", "\n   (width"));
    let one: Vec<_> = r.findings.iter().map(|f| (&f.key, &f.evidence)).collect();
    let two: Vec<_> = q.findings.iter().map(|f| (&f.key, &f.evidence)).collect();
    assert_eq!(one, two);
}
#[test]
fn ignores_change_expire_and_do_not_cross_boards() {
    let s = seg("a", (0., 0.), (10., 3.), 0.3, "F.Cu");
    let r = report(&s);
    let f = r
        .findings
        .iter()
        .find(|f| f.rule == "trace.off_angle")
        .unwrap();
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("reviews.json");
    review::update(
        &p,
        &r,
        &f.key,
        "ignore",
        "intentional RF escape",
        "agent/test",
        None,
    )
    .unwrap();
    let l = review::load(&p).unwrap();
    let mut a = report(&s);
    review::apply(&mut a, &l, review::now());
    assert_eq!(
        a.findings.iter().find(|g| g.key == f.key).unwrap().state,
        "ignored"
    );
    let mut changed = report(&s.replace("width 0.3", "width 0.4"));
    review::apply(&mut changed, &l, review::now());
    assert_eq!(changed.review_audit[0].status, "changed");
    assert!(changed.findings.iter().all(|f| f.state == "open"));
    let mut other = lint(&board(&s), "other", Config::default()).unwrap();
    review::apply(&mut other, &l, review::now());
    assert!(other.review_audit.is_empty());
    let mut expired = l;
    expired.decisions[0].expires_at = Some(1);
    review::apply(&mut a, &expired, 2);
    assert_eq!(a.review_audit[0].status, "expired");
    let mut missing = report("");
    review::apply(&mut missing, &expired, 2);
    assert_eq!(missing.review_audit[0].status, "not_observed");
}
#[test]
fn neighbor_edit_reopens_but_unrelated_edit_does_not() {
    let a = seg("a", (0., 0.), (10., 3.), 0.3, "F.Cu");
    let r = report(&a);
    let get = |r: pcb_lint::Report| {
        r.findings
            .into_iter()
            .find(|f| f.rule == "trace.off_angle" && f.subjects == ["a"])
            .unwrap()
            .evidence
    };
    let original = get(r);
    let near = seg("b", (1., 0.), (2., 0.), 0.3, "F.Cu").replace("net \"N\"", "net \"OTHER\"");
    assert_ne!(original, get(report(&(a.clone() + &near))));
    let far = seg("b", (100., 0.), (101., 0.), 0.3, "F.Cu").replace("net \"N\"", "net \"OTHER\"");
    assert_eq!(original, get(report(&(a + &far))));
}
#[test]
fn zone_geometry_invalidates() {
    let a = seg("a", (0., 0.), (10., 3.), 0.3, "F.Cu");
    let z = r#"(zone (net "N") (layer "F.Cu") (polygon (pts (xy 0 0) (xy 5 0) (xy 5 5))))"#;
    let r = report(&(a.clone() + z));
    let q = report(&(a + &z.replace("xy 5 5", "xy 6 5")));
    assert_ne!(r.findings[0].evidence, q.findings[0].evidence);
}
#[test]
fn review_flag_clear_and_lock() {
    let r = report(&seg("a", (0., 0.), (10., 3.), 0.3, "F.Cu"));
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("reviews.json");
    let k = &r.findings[0].key;
    review::update(&p, &r, k, "flag", "needs reroute", "test", None).unwrap();
    let mut r2 = report(&seg("a", (0., 0.), (10., 3.), 0.3, "F.Cu"));
    review::apply(&mut r2, &review::load(&p).unwrap(), review::now());
    assert_eq!(r2.findings[0].state, "flagged");
    std::fs::write(p.with_extension("lock"), "pid=1").unwrap();
    assert!(review::update(&p, &r, k, "clear", "", "", None).is_err());
    std::fs::remove_file(p.with_extension("lock")).unwrap();
    review::update(&p, &r, k, "clear", "", "", None).unwrap();
    assert!(review::load(&p).unwrap().decisions.is_empty());
}
#[test]
fn whole_run_not_first_segment_width_island() {
    let mut s = String::new();
    for (i, w) in [0.5, 0.2, 0.2, 0.5].iter().enumerate() {
        s += &seg(
            &format!("s{i}"),
            (i as f64 * 0.4, 0.),
            ((i + 1) as f64 * 0.4, 0.),
            *w,
            "F.Cu",
        );
    }
    let r = report(&s);
    assert_eq!(count(&r, "route.width_island"), 1);
    let f = r
        .findings
        .iter()
        .find(|f| f.rule == "route.width_island")
        .unwrap();
    assert!((f.metrics["run_mm"] - 0.8).abs() < 1e-9);
    let c = Config {
        narrow_run_mm: 0.6,
        ..Config::default()
    };
    assert_eq!(
        count(&lint(&board(&s), "x", c).unwrap(), "route.width_island"),
        0
    );
}
#[test]
fn detour_and_excursion() {
    let s = seg("a", (0., 0.), (0., 10.), 0.3, "B.Cu")
        + &seg("b", (0., 10.), (1., 10.), 0.3, "B.Cu")
        + &seg("c", (1., 10.), (1., 0.), 0.3, "B.Cu")
        + &via("v1", 0., 0.)
        + &via("v2", 1., 0.)
        + &seg("d", (-2., 0.), (0., 0.), 0.3, "F.Cu")
        + &seg("e", (1., 0.), (3., 0.), 0.3, "F.Cu");
    let r = report(&s);
    assert_eq!(count(&r, "route.detour"), 1);
    assert_eq!(count(&r, "route.layer_excursion"), 1);
    assert_eq!(count(&r, "via.low_attachment"), 0);
}
#[test]
fn via_layers_follow_stackup() {
    let b = Board::read(&board(&via("v", 0., 0.))).unwrap();
    assert_eq!(b.via_layers(&b.vias[0]), ["F.Cu", "In1.Cu", "B.Cu"]);
}
#[test]
fn contracts_and_missing_coverage() {
    let c:Config=serde_json::from_str(r#"{"net_rules":[{"net":"N","min_width_mm":0.4,"max_vias":0,"allowed_layers":["B.Cu"]}],"pin_nets":[{"reference":"J1","pad":"1","net":"+5V"}]}"#).unwrap();
    let r = lint(
        &board(&(seg("s", (0., 0.), (1., 0.), 0.3, "F.Cu") + &via("v", 0., 0.))),
        "x",
        c,
    )
    .unwrap();
    for rule in [
        "contract.net_width",
        "contract.net_layer",
        "contract.via_budget",
        "contract.pin_net",
    ] {
        assert_eq!(count(&r, rule), 1);
    }
    let plain = report("");
    assert_eq!(
        plain
            .coverage
            .iter()
            .find(|r| r.rule == "contract.pin_net")
            .unwrap()
            .status,
        "needs_contract"
    );
}
#[test]
fn bad_configuration_and_duplicate_uuid_rejected() {
    assert!(serde_json::from_str::<Config>(r#"{"typo":1}"#).is_err());
    let mut c = Config::default();
    c.enabled_only.insert("not.a.rule".into());
    assert!(lint(&board(""), "x", c).is_err());
    let s = seg("same", (0., 0.), (1., 0.), 0.3, "F.Cu");
    assert!(Board::read(&board(&(s.clone() + &s))).is_err());
}
#[test]
fn coverage_is_not_a_pass_claim() {
    let r = report(r#"(arc (start 0 0) (mid 1 1) (end 2 0) (layer "F.Cu") (net "N") (width 0.3))"#);
    assert!(
        r.limitations
            .iter()
            .any(|s| s.contains("arcs (not analyzed): 1"))
    );
    assert!(
        r.coverage
            .iter()
            .any(|r| r.rule == "source.series_parallel" && r.status == "needs_input")
    );
}

fn pour(id: &str, net: &str, layer: &str, points: &str) -> String {
    format!(
        r#"(zone (uuid "{id}") (net "{net}") (layer "{layer}")
        (polygon (pts (xy -5 -5) (xy 5 -5) (xy 5 5) (xy -5 5)))
        (filled_polygon (layer "{layer}") (pts {points})))"#
    )
}
const FILL: &str = "(xy -2 -2) (xy 2 -2) (xy 2 2) (xy -2 2)";
#[test]
fn via_attachment_counts_saved_pours_and_unique_layers() {
    let s = via("v", 0., 0.) + &seg("s", (0., 0.), (3., 0.), 0.3, "F.Cu");
    assert_eq!(count(&report(&s), "via.low_attachment"), 1);
    assert_eq!(
        count(
            &report(&(s.clone() + &pour("z", "N", "B.Cu", FILL))),
            "via.low_attachment"
        ),
        0
    );
    // More copper on the existing layer does not add a second attachment layer.
    assert_eq!(
        count(
            &report(&(s + &pour("z", "N", "F.Cu", FILL))),
            "via.low_attachment"
        ),
        1
    );
    let s = via("v", 0., 0.) + &pour("a", "N", "F.Cu", FILL) + &pour("b", "N", "In1.Cu", FILL);
    assert_eq!(count(&report(&s), "via.low_attachment"), 0);
}
#[test]
fn via_attachment_respects_net_span_clearance_and_thermal_spokes() {
    let s = via("v", 0., 0.) + &seg("s", (0., 0.), (3., 0.), 0.3, "F.Cu");
    assert_eq!(
        count(
            &report(&(s.clone() + &pour("z", "OTHER", "B.Cu", FILL))),
            "via.low_attachment"
        ),
        1
    );
    let blind = s.replace("(layers \"F.Cu\" \"B.Cu\")", "(layers \"F.Cu\" \"In1.Cu\")");
    assert_eq!(
        count(
            &report(&(blind + &pour("z", "N", "B.Cu", FILL))),
            "via.low_attachment"
        ),
        1
    );
    // The zone outline encloses the via, but its actual fill has clearance.
    let gap = "(xy 0.4 -2) (xy 2 -2) (xy 2 2) (xy 0.4 2)";
    assert_eq!(
        count(
            &report(&(s.clone() + &pour("z", "N", "B.Cu", gap))),
            "via.low_attachment"
        ),
        1
    );
    // A narrow saved thermal spoke reaches the annulus, without containing the center.
    let spoke = "(xy 0.2 -0.05) (xy 2 -0.05) (xy 2 0.05) (xy 0.2 0.05)";
    assert_eq!(
        count(
            &report(&(s + &pour("z", "N", "B.Cu", spoke))),
            "via.low_attachment"
        ),
        0
    );
}
#[test]
fn via_attachment_does_not_count_fill_inside_drill() {
    let tiny = "(xy -0.02 -0.02) (xy 0.02 -0.02) (xy 0.02 0.02) (xy -0.02 0.02)";
    let s = via("v", 0., 0.)
        + &seg("s", (0., 0.), (3., 0.), 0.3, "F.Cu")
        + &pour("z", "N", "B.Cu", tiny);
    assert_eq!(count(&report(&s), "via.low_attachment"), 1);
}
#[test]
fn via_attachment_counts_arcs() {
    let s = via("v", 1., 0.)
        + &seg("s", (1., 0.), (3., 0.), 0.3, "F.Cu")
        + r#"(arc (uuid "arc") (start 1 0) (mid 0 1) (end -1 0) (width 0.3) (layer "B.Cu") (net "N"))"#;
    assert_eq!(count(&report(&s), "via.low_attachment"), 0);
}
#[test]
fn via_attachment_missing_fill_is_unknown_only_for_affected_net() {
    let s = via("v", 0., 0.)
        + r#"(zone (uuid "z") (net "N") (layer "B.Cu") (polygon (pts (xy -2 -2) (xy 2 -2) (xy 2 2))))"#;
    let r = report(&s);
    assert_eq!(count(&r, "via.low_attachment"), 0);
    assert_eq!(
        r.coverage
            .iter()
            .find(|x| x.rule == "via.low_attachment")
            .unwrap()
            .status,
        "partial_unsupported_geometry"
    );
    let other = s.replace(
        "(zone (uuid \"z\") (net \"N\")",
        "(zone (uuid \"z\") (net \"OTHER\")",
    );
    assert_eq!(count(&report(&other), "via.low_attachment"), 1);
}
#[test]
fn via_attachment_fill_edit_invalidates_review_without_changing_key() {
    let s = via("v", 0., 0.) + &pour("z", "N", "F.Cu", FILL);
    let a = report(&s);
    let b = report(&s.replace("xy 2 2", "xy 3 2"));
    let get = |r: pcb_lint::Report| {
        r.findings
            .into_iter()
            .find(|f| f.rule == "via.low_attachment")
            .unwrap()
    };
    let a = get(a);
    let b = get(b);
    assert_eq!(a.key, b.key);
    assert_ne!(a.evidence, b.evidence);
}
#[test]
fn via_attachment_multilayer_zone_and_void() {
    let z = format!(
        r#"(zone (uuid "z") (net "N") (layers "F.Cu" "B.Cu")
        (polygon (pts {FILL})) (filled_polygon (layer "F.Cu") (pts {FILL}))
        (filled_polygon (layer "B.Cu") (pts {FILL})))"#
    );
    assert_eq!(
        count(&report(&(via("v", 0., 0.) + &z)), "via.low_attachment"),
        0
    );
    // Concave cutout in the saved fill surrounds the via without touching it.
    let void = "(xy -2 -2) (xy 2 -2) (xy 2 2) (xy 0.5 2) (xy 0.5 -0.5) (xy -0.5 -0.5) (xy -0.5 2) (xy -2 2)";
    let s = via("v", 0., 0.)
        + &seg("s", (0., 0.), (3., 0.), 0.3, "F.Cu")
        + &pour("z", "N", "B.Cu", void);
    assert_eq!(count(&report(&s), "via.low_attachment"), 1);
    let invalid = pour("z", "N", "B.Cu", "(xy -2 -2) (xy 2 2) (xy -2 2) (xy 2 -2)");
    let r = report(&(via("v", 0., 0.) + &invalid));
    assert_eq!(count(&r, "via.low_attachment"), 0);
    assert_eq!(
        r.coverage
            .iter()
            .find(|x| x.rule == "via.low_attachment")
            .unwrap()
            .status,
        "partial_unsupported_geometry"
    );
}

fn excursion() -> String {
    seg("a", (0., 0.), (0., 3.), 0.3, "B.Cu")
        + &seg("b", (0., 3.), (2., 3.), 0.3, "B.Cu")
        + &seg("c", (2., 3.), (2., 0.), 0.3, "B.Cu")
        + &via("v1", 0., 0.)
        + &via("v2", 2., 0.)
        + &seg("d", (-1., 0.), (0., 0.), 0.3, "F.Cu")
        + &seg("e", (2., 0.), (3., 0.), 0.3, "F.Cu")
}
#[test]
fn excursion_flip_checks_target_layer_and_whole_path() {
    let s = excursion();
    assert_eq!(count(&report(&s), "route.layer_excursion"), 1);
    let crossing =
        seg("obstacle", (-1., 1.), (1., 1.), 0.3, "F.Cu").replace("net \"N\"", "net \"OTHER\"");
    assert_eq!(
        count(&report(&(s.clone() + &crossing)), "route.layer_excursion"),
        0
    );
    assert_eq!(
        count(
            &report(&(s.clone() + &crossing.replace("F.Cu", "In1.Cu"))),
            "route.layer_excursion"
        ),
        1
    );
    assert_eq!(
        count(
            &report(&(s + &crossing.replace("OTHER", "N"))),
            "route.layer_excursion"
        ),
        1
    );
}
#[test]
fn excursion_flip_checks_clearance_pads_vias_arcs_and_pours() {
    let obstacles=[
        seg("obstacle",(0.4,0.6),(0.4,2.4),0.3,"F.Cu").replace("net \"N\"","net \"OTHER\""),
        via("obstacle",0.,1.).replace("net \"N\"","net \"OTHER\""),
        r#"(footprint "test" (layer "F.Cu") (at 0 1) (uuid "part") (pad "1" smd rect (at 0 0) (size 0.5 0.5) (layers "F.Cu") (net "OTHER") (uuid "pad")))"#.into(),
        r#"(arc (uuid "arc") (start -1 1) (mid 0 2) (end 1 1) (width 0.3) (layer "F.Cu") (net "OTHER"))"#.into(),
        pour("z","OTHER","F.Cu",FILL),
    ];
    for obstacle in obstacles {
        assert_eq!(
            count(&report(&(excursion() + &obstacle)), "route.layer_excursion"),
            0,
            "{obstacle}"
        );
    }
}
#[test]
fn excursion_flip_checks_keepouts_and_unknown_fill() {
    let c:Config=serde_json::from_str(r#"{"intent":{"route":{"keepouts":[{"id":"keepout","layer":"F.Cu","polygon":[[ -0.5, 0.5 ],[ 0.5, 0.5 ],[ 0.5, 1.5 ],[ -0.5, 1.5 ]]}]}}}"#).unwrap();
    assert_eq!(
        count(
            &lint(&board(&excursion()), "x", c).unwrap(),
            "route.layer_excursion"
        ),
        0
    );
    let r = report(
        &(excursion()
            + r#"(zone (net "OTHER") (layer "F.Cu") (polygon (pts (xy -5 -5) (xy 5 -5) (xy 5 5))))"#),
    );
    assert_eq!(count(&r, "route.layer_excursion"), 0);
    assert_eq!(
        r.coverage
            .iter()
            .find(|x| x.rule == "route.layer_excursion")
            .unwrap()
            .status,
        "partial_unsupported_geometry"
    );
}
#[test]
fn via_attachment_kicad_slit_linked_hole_contours() {
    // Outer ring enters a hole, loops clockwise, then returns along the same bridge.
    let pts = "(xy -3 -3) (xy 3 -3) (xy 3 3) (xy -3 3) (xy -3 -3) (xy -0.5 -0.5) (xy -0.5 0.5) (xy 0.5 0.5) (xy 0.5 -0.5) (xy -0.5 -0.5) (xy -3 -3)";
    let z = pour("z", "N", "B.Cu", pts);
    let center = via("v", 0., 0.) + &seg("t", (0., 0.), (0.2, 0.), 0.3, "F.Cu");
    let r = report(&(center + &z));
    assert_eq!(count(&r, "via.low_attachment"), 1);
    assert_eq!(
        r.coverage
            .iter()
            .find(|c| c.rule == "via.low_attachment")
            .unwrap()
            .status,
        "evaluated"
    );
    let copper = via("v", 1., 1.) + &seg("t", (1., 1.), (1.2, 1.), 0.3, "F.Cu");
    assert_eq!(count(&report(&(copper + &z)), "via.low_attachment"), 0);
}
#[test]
fn excursion_native_keepouts_use_track_permission_and_board_coordinates() {
    let area = r#"(zone (uuid "keepout") (layer "F.Cu")
        (keepout (tracks not_allowed) (vias not_allowed))
        (polygon (pts (xy -0.5 0.5) (xy 0.5 0.5) (xy 0.5 1.5) (xy -0.5 1.5))))"#;
    assert_eq!(
        count(&report(&(excursion() + area)), "route.layer_excursion"),
        0
    );
    let footprint =
        format!(r#"(footprint "test" (uuid "fp") (layer "F.Cu") (at 100 200 90) {area})"#);
    assert_eq!(
        count(
            &report(&(excursion() + &footprint)),
            "route.layer_excursion"
        ),
        0
    );
    assert_eq!(
        count(
            &report(&(excursion() + &area.replace("tracks not_allowed", "tracks allowed"))),
            "route.layer_excursion"
        ),
        1
    );
    assert_eq!(
        count(
            &report(&(excursion() + &area.replace("F.Cu", "In1.Cu"))),
            "route.layer_excursion"
        ),
        1
    );
}

fn overshoot() -> String {
    via("turn", 4., 0.)
        + &seg("upper", (0., 0.), (4., 0.), 0.3, "F.Cu")
        + &seg("lower", (4., 0.), (0., 4.), 0.3, "B.Cu")
}
fn overshoot_report(body: &str, tuned: bool) -> pcb_lint::Report {
    let mut c = Config::default();
    c.intent.route.outline = vec![
        Point { x: -5., y: -5. },
        Point { x: 10., y: -5. },
        Point { x: 10., y: 10. },
        Point { x: -5., y: 10. },
    ];
    if tuned {
        c.intent.route.tuned_nets.push("N".into());
    }
    lint(&board(body), "via-test", c).unwrap()
}
#[test]
fn via_overshoot_shortens_without_bends_and_has_reviewable_coordinates() {
    let r = overshoot_report(&overshoot(), false);
    let f = r
        .findings
        .iter()
        .find(|f| f.rule == "via.overshoot")
        .unwrap();
    assert!(f.message.contains("saves"));
    assert_eq!(f.subjects.len(), 3);
    assert_eq!(
        count(&overshoot_report(&overshoot(), true), "via.overshoot"),
        0
    );
    assert_eq!(count(&report(&overshoot()), "via.overshoot"), 0); // no outline
    let straight = via("v", 0., 0.)
        + &seg("a", (-3., 0.), (0., 0.), 0.3, "F.Cu")
        + &seg("b", (0., 0.), (3., 0.), 0.3, "B.Cu");
    assert_eq!(
        count(&overshoot_report(&straight, false), "via.overshoot"),
        0
    );
}
#[test]
fn via_overshoot_blocked_on_inner_layer_or_via_only_keepout() {
    // All shorter candidate locations lie left of the original via at x=4.
    let blocker = pour(
        "inner",
        "OTHER",
        "In1.Cu",
        "(xy -3 -4) (xy 3 -4) (xy 3 8) (xy -3 8)",
    );
    assert_eq!(
        count(
            &overshoot_report(&(overshoot() + &blocker), false),
            "via.overshoot"
        ),
        0
    );
    let keepout = r#"(zone (uuid "via-only") (layer "F.Cu") (keepout (tracks allowed) (vias not_allowed)) (polygon (pts (xy -3 -4) (xy 3 -4) (xy 3 8) (xy -3 8))))"#;
    assert_eq!(
        count(
            &overshoot_report(&(overshoot() + keepout), false),
            "via.overshoot"
        ),
        0
    );
}
#[test]
fn via_overshoot_preserves_branches_and_plane_attachments() {
    let third = seg("third", (4., 0.), (6., 0.), 0.3, "F.Cu");
    assert_eq!(
        count(
            &overshoot_report(&(overshoot() + &third), false),
            "via.overshoot"
        ),
        0
    );
    let plane = pour(
        "plane",
        "N",
        "In1.Cu",
        "(xy 3 -1) (xy 5 -1) (xy 5 1) (xy 3 1)",
    );
    assert_eq!(
        count(
            &overshoot_report(&(overshoot() + &plane), false),
            "via.overshoot"
        ),
        0
    );
    let missing =
        r#"(zone (net "OTHER") (layer "F.Cu") (polygon (pts (xy -1 -1) (xy 1 -1) (xy 1 1))))"#;
    assert_eq!(
        count(
            &overshoot_report(&(overshoot() + missing), false),
            "via.overshoot"
        ),
        0
    );
}

fn local_dogleg() -> String {
    seg("s0", (-4., 0.), (0., 0.), 0.3, "F.Cu")
        + &seg("s1", (0., 0.), (2., 0.), 0.3, "F.Cu")
        + &seg("s2", (2., 0.), (2., 2.), 0.3, "F.Cu")
        + &seg("s3", (2., 2.), (4., 2.), 0.3, "F.Cu")
        + &seg("s4", (4., 2.), (8., 2.), 0.3, "F.Cu")
}
#[test]
fn local_shortcut_finds_subset_and_reports_fixed_endpoint_path() {
    let r = overshoot_report(&local_dogleg(), false);
    let findings: Vec<_> = r
        .findings
        .iter()
        .filter(|f| f.rule == "route.legal_shortcut")
        .collect();
    assert!(!findings.is_empty());
    assert!(findings.iter().any(|f| f.subjects.len() < 5));
    let mut subjects = std::collections::BTreeSet::new();
    for f in findings {
        assert!(f.message.contains("consecutive segments"));
        assert!(f.message.contains("→"));
        assert!(f.metrics["saved_mm"] > 1.);
        for id in &f.subjects {
            assert!(subjects.insert(id));
        }
    }
    assert_eq!(
        count(
            &overshoot_report(&local_dogleg(), true),
            "route.legal_shortcut"
        ),
        0
    );
}
#[test]
fn local_shortcut_keeps_intermediate_via_contacts() {
    let anchored = local_dogleg() + &via("anchor1", 2., 0.) + &via("anchor2", 2., 2.);
    assert_eq!(
        count(&overshoot_report(&anchored, false), "route.legal_shortcut"),
        0
    );
}
#[test]
fn local_shortcut_does_not_cut_foreign_copper_or_keepout() {
    let route =
        seg("a", (0., 0.), (2., 0.), 0.3, "F.Cu") + &seg("b", (2., 0.), (2., 2.), 0.3, "F.Cu");
    // Block the shorter diagonal, leaving the existing perimeter route clear.
    let obstacle = pour(
        "wall",
        "OTHER",
        "F.Cu",
        "(xy 0.4 0.4) (xy 1.6 0.4) (xy 1.6 1.6) (xy 0.4 1.6)",
    );
    assert_eq!(
        count(
            &overshoot_report(&(route.clone() + &obstacle), false),
            "route.legal_shortcut"
        ),
        0
    );
    let keepout = r#"(zone (uuid "keepout") (layer "F.Cu") (keepout (tracks not_allowed)) (polygon (pts (xy 0.4 0.4) (xy 1.6 0.4) (xy 1.6 1.6) (xy 0.4 1.6))))"#;
    assert_eq!(
        count(
            &overshoot_report(&(route + keepout), false),
            "route.legal_shortcut"
        ),
        0
    );
}

fn passive(reference: &str, x: f64, y: f64, angle: f64, layer: &str) -> String {
    format!(
        r#"(footprint "Resistor_SMD:R_0603" (uuid "fp-{reference}") (layer "{layer}") (at {x} {y} {angle})
    (property "Reference" "{reference}") (property "Value" "10k")
    (pad "1" smd rect (uuid "{reference}-1") (at -0.8 0) (size 0.8 0.9) (layers "{layer}") (net "N"))
    (pad "2" smd rect (uuid "{reference}-2") (at 0.8 0) (size 0.8 0.9) (layers "{layer}") (net "GND")))"#
    )
}
#[test]
fn passive_alignment_rows_columns_and_half_turns() {
    let row = passive("R1", 0., 0., 0., "F.Cu")
        + &passive("R2", 3., 0.3, 180., "F.Cu")
        + &passive("R3", 6., 0., 0., "F.Cu");
    let r = report(&row);
    let f = r
        .findings
        .iter()
        .find(|f| f.rule == "placement.passive_alignment")
        .unwrap();
    assert!(f.message.contains("row"));
    assert!(f.message.contains("R2 -0.300"));
    assert_eq!(f.subjects.len(), 3);
    let col = passive("C1", 0., 0., 90., "B.Cu")
        + &passive("C2", 0.3, 3., 270., "B.Cu")
        + &passive("C3", 0., 6., 90., "B.Cu");
    assert_eq!(count(&report(&col), "placement.passive_alignment"), 1);
}
#[test]
fn passive_alignment_excludes_unrelated_and_already_aligned_parts() {
    let base = passive("R1", 0., 0., 0., "F.Cu") + &passive("R2", 3., 0., 0., "F.Cu");
    for third in [
        passive("R3", 6., 0., 0., "F.Cu"),
        passive("R3", 6., 0.3, 90., "F.Cu"),
        passive("C3", 6., 0.3, 0., "F.Cu"),
        passive("R3", 6., 0.3, 0., "B.Cu"),
        passive("R3", 20., 0.3, 0., "F.Cu"),
        passive("R3", 6., 1., 0., "F.Cu"),
    ] {
        assert_eq!(
            count(
                &report(&(base.clone() + &third)),
                "placement.passive_alignment"
            ),
            0
        );
    }
    let body = base + &passive("R3", 6., 0.3, 0., "F.Cu");
    let mut c = Config::default();
    c.intent
        .passive_alignment
        .exclude_references
        .push("R3".into());
    assert_eq!(
        count(
            &lint(&board(&body), "x", c).unwrap(),
            "placement.passive_alignment"
        ),
        0
    );
}
#[test]
fn passive_alignment_pair_opt_in_and_invalid_policy() {
    let body = passive("R1", 0., 0., 0., "F.Cu") + &passive("R2", 3., 0.3, 0., "F.Cu");
    assert_eq!(count(&report(&body), "placement.passive_alignment"), 0);
    let mut c = Config::default();
    c.intent.passive_alignment.min_group = 2;
    assert_eq!(
        count(
            &lint(&board(&body), "x", c.clone()).unwrap(),
            "placement.passive_alignment"
        ),
        1
    );
    c.intent.passive_alignment.min_group = 1;
    assert!(lint(&board(&body), "x", c).is_err());
}
