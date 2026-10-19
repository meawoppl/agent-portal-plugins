use kct::explain::checks::{DifferentialPairSkewCheck, ViaInPadCheck};
use kct::explain::mistakes::MistakeCheck;
use kct::schema::pcb::Pcb;

#[test]
fn rotated_split_pads_only_report_real_copper_once() {
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb
      (footprint "TO252" (layer "F.Cu") (at 20 20 90)
        (pad "2" smd rect (at 4 0 90) (size 4 2) (layers "F.Cu"))
        (pad "2" smd rect (at 4 0 90) (size 4 2) (layers "F.Cu")))
      (via (at 24 20) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu"))
      (via (at 20 16) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu")))"#,
    )
    .unwrap();
    let findings = ViaInPadCheck.check(&pcb).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].location, Some((20.0, 16.0)));
}

#[test]
fn usb_requires_a_declared_budget_before_failing() {
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (net 1 "USB_D+") (net 2 "USB_D-")
      (segment (start 0 0) (end 32.2 0) (width 0.2) (layer "F.Cu") (net 1))
      (segment (start 0 1) (end 38.4 1) (width 0.2) (layer "F.Cu") (net 2)))"#,
    )
    .unwrap();
    let unknown = DifferentialPairSkewCheck.check(&pcb).unwrap();
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].severity, "info");
    assert!(unknown[0]
        .explanation
        .contains("no High-Speed limit was applied"));
    let limit = |mm| vec![("USB_D+".into(), "USB_D-".into(), mm)];
    let hs = DifferentialPairSkewCheck::check_with_limits(&pcb, &limit(2.0)).unwrap();
    assert_eq!(hs[0].severity, "error");
    assert!(
        DifferentialPairSkewCheck::check_with_limits(&pcb, &limit(20.0))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn circular_pad_does_not_include_bounding_box_corners() {
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb
      (footprint "test" (layer "F.Cu") (at 10 10)
        (pad "1" smd circle (at 0 0) (size 2 2) (layers "F.Cu")))
      (via (at 10.9 10.9) (size 0.1) (drill 0.05) (layers "F.Cu" "B.Cu")))"#,
    )
    .unwrap();
    assert!(ViaInPadCheck.check(&pcb).unwrap().is_empty());
}

#[test]
fn unsupported_custom_pad_reports_incomplete_instead_of_rectangle_collision() {
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb
      (footprint "test" (layer "F.Cu") (at 10 10)
        (pad "1" smd custom (at 0 0) (size 2 2) (layers "F.Cu"))))"#,
    )
    .unwrap();
    assert!(ViaInPadCheck.check(&pcb).is_err());
}

#[test]
fn declared_pairs_need_not_follow_net_naming_heuristics() {
    let pcb = Pcb::parse_str(
        r#"(kicad_pcb (net 1 "A") (net 2 "B")
      (segment (start 0 0) (end 3 0) (width 0.2) (layer "F.Cu") (net 1))
      (segment (start 0 1) (end 10 1) (width 0.2) (layer "F.Cu") (net 2)))"#,
    )
    .unwrap();
    let result =
        DifferentialPairSkewCheck::check_with_limits(&pcb, &[("A".into(), "B".into(), 2.0)])
            .unwrap();
    assert_eq!(result[0].severity, "error");
}
