use kct::{
    explain::{checks::BypassCapDistanceCheck, mistakes::MistakeCheck},
    schema::pcb::Pcb,
};
fn fp(reference: &str, x: f64, y: f64, rotation: f64, offset: f64) -> String {
    let mut out = format!(
        r#"(footprint "test" (layer "F.Cu") (at {x} {y} {rotation}) (property "Reference" "{reference}") (property "Value" "100nF") (pad "1" smd rect (at {offset} 0) (size 1 1) (layers "F.Cu") (net 1 "VCC")) (pad "2" smd rect (at 0 1) (size 1 1) (layers "F.Cu") (net 2 "GND"))"#
    );
    if reference.starts_with('U') {
        out.push_str(r#"(pad "3" smd rect (at 0 2) (size 1 1) (layers "F.Cu")) (pad "4" smd rect (at 0 3) (size 1 1) (layers "F.Cu"))"#);
    }
    out.push(')');
    out
}
fn check(parts: &str) -> Vec<kct::explain::mistakes::Mistake> {
    let pcb = Pcb::from_sexp(kct::sexp::parse(&format!(r#"(kicad_pcb (version 20240108) (layers (0 "F.Cu" signal) (31 "B.Cu" signal)) (net 1 "VCC") (net 2 "GND") {parts})"#)).unwrap()).unwrap();
    BypassCapDistanceCheck.check(&pcb).unwrap()
}
#[test]
fn dense_shared_supply_is_not_all_pairs() {
    let mut parts = String::new();
    for i in 0..100 {
        parts += &fp(&format!("U{i}"), i as f64 * 10., 0., 0., 0.);
        parts += &fp(&format!("C{i}"), i as f64 * 10. + 1., 0., 0., 0.);
    }
    for reference in ["TP1", "R1", "C999"] {
        parts += &fp(reference, 2000., 0., 0., 0.);
    }
    assert!(check(&parts).is_empty());
}
#[test]
fn reports_worst_pin_once_per_capacitor_net() {
    let parts = fp("U1", 0., 0., 0., 0.) + &fp("U2", 10., 0., 0., 0.) + &fp("C1", 30., 0., 0., 0.);
    let findings = check(&parts);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].components, ["C1", "U1"]);
}
#[test]
fn uses_rotated_power_pad_and_cap_pad_positions() {
    let parts = fp("U1", 0., 0., 90., 10.) + &fp("C1", 20., -10., 0., -19.);
    assert!(check(&parts).is_empty());
}
