use kct::{
    derived_docs::{plan, records, DerivedFile, Kind},
    schema::pcb::Pcb,
    sexp::parse,
};
use serde_json::json;
fn board() -> Pcb {
    Pcb::from_sexp(parse(r#"(kicad_pcb (footprint "TestPoint" (layer "B.Cu") (at 20 30 90) (property "Reference" "TP1") (property "Value" "SDA") (pad "1" smd circle (at 0 0) (size 1 1) (layers "B.Cu") (net 1 "SDA"))))"#).unwrap()).unwrap()
}
#[test]
fn records_preserve_authored_fields_and_refresh_geometry() {
    let old = json!([{"reference":"TP1","purpose":"Probe I2C","position":[0,0]}]);
    let r = records(&board(), Kind::Testpoints, &old).unwrap();
    assert_eq!(r[0]["purpose"], "Probe I2C");
    assert_eq!(r[0]["position"], json!([20., 30.]));
    assert_eq!(r[0]["side"], "back");
    assert_eq!(r[0]["nets"], json!(["SDA"]));
}
#[test]
fn stale_check_is_read_only_and_converges() {
    let d = tempfile::tempdir().unwrap();
    let files = vec![DerivedFile {
        path: "placements.json".into(),
        kind: Kind::Placements,
    }];
    let p = plan(d.path(), &board(), &files).unwrap();
    assert!(p[0].stale);
    assert!(!p[0].path.exists());
    std::fs::write(&p[0].path, &p[0].content).unwrap();
    assert!(!plan(d.path(), &board(), &files).unwrap()[0].stale);
}
#[test]
fn refuses_escape_and_invalid_existing_json() {
    let d = tempfile::tempdir().unwrap();
    assert!(plan(
        d.path(),
        &board(),
        &[DerivedFile {
            path: "../bad.json".into(),
            kind: Kind::Placements
        }]
    )
    .is_err());
    std::fs::write(d.path().join("bad.json"), "{}").unwrap();
    assert!(plan(
        d.path(),
        &board(),
        &[DerivedFile {
            path: "bad.json".into(),
            kind: Kind::Placements
        }]
    )
    .is_err());
}
