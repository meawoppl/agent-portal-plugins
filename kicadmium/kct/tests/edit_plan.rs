use kct::{
    edit_plan::{apply, Plan},
    schema::pcb::Pcb,
    sexp::parse,
};
fn board() -> Pcb {
    Pcb::from_sexp(parse(r#"(kicad_pcb (layers (0 "F.Cu" signal) (31 "B.Cu" signal)) (net 1 "N") (footprint "IC" (at 10 10 33) (layer "F.Cu") (uuid "u") (property "Reference" "U1") (pad "1" smd rect (at 0.1234567 0.5) (size 1 1) (layers "F.Cu") (net 1 "N"))))"#).unwrap()).unwrap()
}
#[test]
fn exact_pad_snapping_and_collinear_merge() {
    let p = board();
    let pad = p.get_pad_position("U1", "1").unwrap();
    let plan:Plan=serde_json::from_value(serde_json::json!({"version":1,"operations":[{"op":"add-track","net":"N","layer":"F.Cu","width":0.25,"path":["U1.1",[pad.0+1.,pad.1],[pad.0+2.,pad.1]]}]})).unwrap();
    let edited = apply(&p, &plan).unwrap();
    assert_eq!(edited.segments().len(), 1);
    assert_eq!(edited.segments()[0].start, pad);
    assert!(p.segments().is_empty());
}
#[test]
fn single_plan_moves_then_resolves_pad_and_adds_via_zone() {
    let p = board();
    let plan:Plan=serde_yaml::from_str("version: 1\noperations:\n- {op: move, reference: U1, at: [20, 30], rotation: 90}\n- {op: add-via, net: N, at: U1.1, size: 0.6, drill: 0.3}\n- {op: add-zone, net: N, layer: B.Cu, priority: 2, rect: [1, 2, 3, 4]}\n").unwrap();
    let edited = apply(&p, &plan).unwrap();
    assert_eq!(
        edited.vias()[0].position,
        edited.get_pad_position("U1", "1").unwrap()
    );
    assert_eq!(edited.zones().len(), 1);
}
#[test]
fn invalid_batch_preserves_original() {
    let p = board();
    let original = p.sexp().clone();
    let plan:Plan=serde_yaml::from_str("version: 1\noperations:\n- {op: move, reference: U1, at: [20, 30]}\n- {op: add-via, net: unknown, at: [1, 2], size: 0.6, drill: 0.3}\n").unwrap();
    assert!(apply(&p, &plan).is_err());
    assert_eq!(p.sexp(), &original);
}
#[test]
fn coordinates_stay_in_sheet_frame_with_offset_outline() {
    let mut p = board();
    p.sexp_mut().push(parse(r#"(gr_rect (start 100 100) (end 200 200) (stroke (width 0.05) (type default)) (fill none) (layer "Edge.Cuts"))"#).unwrap());
    p.reload_from_tree().unwrap();
    let plan:Plan=serde_yaml::from_str("version: 1\noperations:\n- {op: move, reference: U1, at: [120, 130], rotation: 0}\n- {op: add-via, net: N, at: [125, 135], size: 0.6, drill: 0.3}\n").unwrap();
    let edited = apply(&p, &plan).unwrap();
    let fp = edited.sexp().get("footprint").unwrap();
    assert_eq!(fp.get("at").unwrap().float_at(0), Some(120.));
    let via = edited.sexp().get("via").unwrap();
    assert_eq!(via.get("at").unwrap().float_at(0), Some(125.));
}
