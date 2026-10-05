use super::*;

#[test]
fn parses_and_queries() {
    let doc = parse(r#"(kicad_sch (version 20231120) (symbol (lib_id "Device:R") (property "Reference" "R1") (at 1.5 -2 90)))"#).unwrap();
    assert!(doc.has_tag("kicad_sch"));
    let sym = doc
        .find_where("symbol", &[("lib_id", "Device:R".into())])
        .unwrap();
    assert_eq!(sym.property("Reference"), Some("R1"));
    assert_eq!(sym.at(), Some((1.5, -2.0, 90.0)));
    assert_eq!(doc.get("version").unwrap().int_at(0), Some(20231120));
}

#[test]
fn numbers_round_trip_exactly() {
    let doc = parse("(at 0.1000 -0 1e-3)").unwrap();
    assert_eq!(doc.to_compact_string(), "(at 0.1000 -0 1e-3)");
}

#[test]
fn quoting_is_preserved() {
    let doc =
        parse(r#"(x (generator_version "9.0") (pad "1" smd roundrect) (tracks not_allowed))"#)
            .unwrap();
    assert_eq!(
        doc.to_compact_string(),
        r#"(x (generator_version "9.0") (pad "1" smd roundrect) (tracks not_allowed))"#
    );
}

#[test]
fn constructed_atoms_follow_kicad_quoting() {
    let node = SExp::list(
        "pad",
        [SExp::atom("1"), SExp::atom("smd"), SExp::atom("R1 x")],
    );
    assert_eq!(node.to_compact_string(), r#"(pad 1 smd "R1 x")"#);
    let node = SExp::list("layer", [SExp::atom("F.Cu")]);
    assert_eq!(node.to_compact_string(), r#"(layer "F.Cu")"#);
}

#[test]
fn escapes() {
    let doc = parse(r#"(t "a\"b\\c\nd")"#).unwrap();
    assert_eq!(doc.string_at(0), Some("a\"b\\c\nd"));
    assert_eq!(doc.to_compact_string(), r#"(t "a\"b\\c\nd")"#);
}

#[test]
fn numeric_list_heads() {
    let doc = parse(r#"(layers (0 "F.Cu" signal) (31 "B.Cu" signal))"#).unwrap();
    let names: Vec<_> = doc
        .children
        .iter()
        .map(|c| c.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["0", "31"]);
}

#[test]
fn pretty_matches_kicad_shape() {
    let doc = parse(r#"(kicad_pcb (version 20240108) (net 0 "") (segment (start 0 0) (end 1 0) (width 0.25) (layer "F.Cu") (net 1)))"#).unwrap();
    let text = doc.to_kicad_string();
    assert_eq!(
        text,
        "(kicad_pcb\n\t(version 20240108)\n\t(net 0 \"\")\n\t(segment\n\t\t(start 0 0)\n\t\t(end 1 0)\n\t\t(width 0.25)\n\t\t(layer \"F.Cu\")\n\t\t(net 1)\n\t)\n)"
    );
    assert_eq!(parse(&text).unwrap(), doc);
}

#[test]
fn edits() {
    let mut doc = parse(r#"(footprint "R" (property "Value" "1k") (layer "F.Cu"))"#).unwrap();
    assert!(doc.set_property("Value", "10k"));
    doc.set_child_value("layer", "B.Cu");
    assert_eq!(
        doc.to_compact_string(),
        r#"(footprint "R" (property "Value" "10k") (layer "B.Cu"))"#
    );
    assert!(doc.remove_child("layer"));
    assert!(doc.get("layer").is_none());
}

#[test]
fn errors_have_positions() {
    let err = parse("(a\n  (b").unwrap_err();
    assert_eq!(err.line, 2);
}

#[test]
fn float_formatting_matches_python_g() {
    assert_eq!(format::float(1.0), "1");
    assert_eq!(format::float(0.25), "0.25");
    assert_eq!(format::float(1.0 / 3.0), "0.333333");
    assert_eq!(format::float(123456.7), "123457");
    assert_eq!(format::float(1234567.5), "1.23457e+06");
    assert_eq!(format::float(0.00001234), "1.234e-05");
}
