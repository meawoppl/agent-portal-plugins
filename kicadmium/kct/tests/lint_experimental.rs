use kct::lint::{lint, Config};
#[test]
fn default_reports_only_evaluated_confidence() {
    let src = include_str!("fixtures/lint/advanced-bad.kicad_pcb");
    let default = lint(src, "board", Config::default()).unwrap();
    assert!(default
        .findings
        .iter()
        .all(|f| f.confidence != "not_evaluated"));
    let config = Config {
        include_experimental: true,
        ..Default::default()
    };
    let experimental = lint(src, "board", config).unwrap();
    assert!(experimental
        .findings
        .iter()
        .any(|f| f.confidence == "not_evaluated"
            && f.suggestion.contains("safe to ignore for the lint gate")));
    assert!(experimental.findings.len() > default.findings.len());
}
fn board(body: &str) -> String {
    format!(r#"(kicad_pcb (layers (0 "F.Cu" signal) (31 "B.Cu" signal)) {body})"#)
}
#[test]
fn neck_down_intent_is_bounded_to_net_pad_shape_and_length() {
    let src = board(
        r#"(footprint "Test" (at 0 0) (layer "F.Cu") (uuid "u") (property "Reference" "U1") (pad "1" smd rect (at 0 0) (size 0.4 0.4) (layers "F.Cu") (net "N") (uuid "p"))) (segment (start 0 0) (end 0.5 0) (width 0.15) (layer "F.Cu") (net "N") (uuid "a")) (segment (start -2 0) (end 0 0) (width 0.5) (layer "F.Cu") (net "N") (uuid "b"))"#,
    );
    let mut config: Config = serde_json::from_value(serde_json::json!({"enabled_only":["trace.width_transition"],"intent":{"neck_downs":[{"net":"N","pad_shapes":["rect"],"min_width_mm":0.1,"max_length_mm":0.6}]}})).unwrap();
    assert!(lint(&src, "b", config.clone()).unwrap().findings.is_empty());
    config.intent.neck_downs[0].max_length_mm = 0.1;
    assert!(!lint(&src, "b", config).unwrap().findings.is_empty());
}
#[test]
fn via_roles_apply_only_within_region_and_matching_net() {
    let src = board(
        r#"(via (at 0 0) (size 0.5) (drill 0.25) (layers "F.Cu" "B.Cu") (net "N") (uuid "a")) (via (at 0.6 0) (size 0.5) (drill 0.25) (layers "F.Cu" "B.Cu") (net "N") (uuid "b"))"#,
    );
    let mut config: Config = serde_json::from_value(serde_json::json!({"enabled_only":["via.cluster"],"intent":{"via_role_regions":[{"role":"stitching","region":{"id":"block","layer":"F.Cu","nets":["N"],"polygon":[{"x":-1,"y":-1},{"x":1,"y":-1},{"x":1,"y":1},{"x":-1,"y":1}]}}]}})).unwrap();
    assert!(lint(&src, "b", config.clone()).unwrap().findings.is_empty());
    config.intent.via_role_regions[0].region.nets = vec!["OTHER".into()];
    assert!(!lint(&src, "b", config).unwrap().findings.is_empty());
}
