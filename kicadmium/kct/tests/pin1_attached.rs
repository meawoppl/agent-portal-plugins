use kct::{
    pin1::{attach, Marker},
    schema::pcb::Pcb,
    sexp::parse,
};
fn board(side: &str) -> Pcb {
    Pcb::from_sexp(parse(&format!(r#"(kicad_pcb
 (footprint "IC" (layer "{side}.Cu") (at 10 20 90) (uuid "fp") (property "Reference" "U2")
 (pad "1" smd rect (at 1 0) (size 1 1) (layers "{side}.Cu")))
 (gr_circle (center 10 18) (end 10.2 18) (stroke (width 0.1) (type default)) (fill solid) (layer "{side}.SilkS") (uuid "dot")))"#)).unwrap()).unwrap()
}
fn markers() -> Vec<Marker> {
    vec![Marker {
        reference: "U2".into(),
        uuid: "dot".into(),
    }]
}
#[test]
fn migration_is_idempotent_and_follows_move_and_rotation() {
    for side in ["F", "B"] {
        let mut p = board(side);
        let r = attach(&mut p, &markers(), 2.5).unwrap();
        assert!((r[0].distance_mm - 1.).abs() < 1e-9);
        assert!(p.sexp().get("gr_circle").is_none());
        let circle = p.sexp().get("footprint").unwrap().get("fp_circle").unwrap();
        assert!((circle.get("center").unwrap().float_at(0).unwrap() - 2.).abs() < 1e-9);
        let original = circle.clone();
        assert!(attach(&mut p, &markers(), 2.5).unwrap()[0].already_attached);
        *p.sexp_mut()
            .get_mut("footprint")
            .unwrap()
            .get_mut("at")
            .unwrap() = parse("(at 70 80 180)").unwrap();
        p.reload_from_tree().unwrap();
        assert_eq!(
            p.sexp().get("footprint").unwrap().get("fp_circle").unwrap(),
            &original
        );
        assert!(attach(&mut p, &markers(), 2.5).unwrap()[0].already_attached);
    }
}
#[test]
fn invalid_batch_does_not_partially_mutate() {
    let mut p = board("F");
    let original = p.sexp().clone();
    let mut m = markers();
    m.push(Marker {
        reference: "U999".into(),
        uuid: "other".into(),
    });
    assert!(attach(&mut p, &m, 2.5).is_err());
    assert_eq!(p.sexp(), &original);
    assert!(attach(&mut p, &markers(), 0.5).is_err());
    assert_eq!(p.sexp(), &original);
}
#[test]
fn attachment_uses_sheet_frame_on_offset_board() {
    let mut pcb = board("F");
    pcb.sexp_mut().push(parse(r#"(gr_rect (start 5 5) (end 100 100) (stroke (width 0.05) (type default)) (fill none) (layer "Edge.Cuts"))"#).unwrap());
    pcb.reload_from_tree().unwrap();
    let result = attach(&mut pcb, &markers(), 2.5).unwrap();
    assert!((result[0].distance_mm - 1.).abs() < 1e-9);
}
