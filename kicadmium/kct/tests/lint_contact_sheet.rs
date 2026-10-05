use kct::lint::{contact_sheet, lint, Config};
use serde_json::Value;
const SOURCE: &str = r#"(kicad_pcb
(version 20240108) (generator "test")
(layers (0 "F.Cu" signal) (31 "B.Cu" signal))
(net 1 "N")
(segment (start 10 20) (end 10.1 20) (width 0.2) (layer "F.Cu") (net 1) (uuid "trace-1"))
(segment (start 100 200) (end 100.1 200) (width 0.2) (layer "B.Cu") (net 1) (uuid "trace-2"))
)"#;
fn report() -> kct::lint::Report {
    let mut c = Config::default();
    c.enabled_only.insert("trace.short_segment".into());
    lint(SOURCE, "test", c).unwrap()
}
fn data(html: &str) -> Value {
    serde_json::from_str(
        html.split("<script type=\"application/json\" id=\"data\">")
            .nth(1)
            .unwrap()
            .split("</script>")
            .next()
            .unwrap(),
    )
    .unwrap()
}
#[test]
fn crops_are_local_and_keys_are_preserved() {
    let r = report();
    assert_eq!(r.findings.len(), 2);
    let h = contact_sheet::render(SOURCE, &r).unwrap();
    let d = data(&h);
    assert_eq!(d["findings"].as_array().unwrap().len(), 2);
    for c in d["findings"].as_array().unwrap() {
        assert_eq!(c["spatial"], true);
        assert_eq!(c["highlighted"].as_array().unwrap().len(), 1);
        let f = &c["finding"];
        let x = f["at"]["x"].as_f64().unwrap();
        let y = f["at"]["y"].as_f64().unwrap();
        let crop = c["crop"].as_array().unwrap();
        let v: Vec<_> = crop.iter().map(|x| x.as_f64().unwrap()).collect();
        assert!(x >= v[0] && x <= v[0] + v[2] && y >= v[1] && y <= v[1] + v[3]);
        assert!(v[2] < 10. && v[3] < 10.);
        assert!(r.findings.iter().any(|original| original.key == f["key"]));
    }
    assert!(h.contains("fill=\"#1a1b26\"")); // explicit opaque exported image backdrop
}
#[test]
fn unlocated_findings_are_labeled_overviews() {
    let mut r = report();
    r.findings[0].subjects = vec!["contract:external-fact".into()];
    let h = contact_sheet::render(SOURCE, &r).unwrap();
    let d = data(&h);
    assert!(d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["spatial"] == false));
    assert!(h.contains("no spatial subject available"));
}
#[test]
fn markup_and_template_tokens_cannot_escape_data() {
    let mut r = report();
    r.board_id = "%%DATA%% </script><img src=x onerror=alert(1)>".into();
    r.findings[0].message = "</script><script>alert('x')</script> %%OBJECTS%%".into();
    let h = contact_sheet::render(SOURCE, &r).unwrap();
    assert!(!h.contains("<img src=x"));
    assert!(!h.contains("<script>alert('x')"));
    assert_eq!(data(&h)["board"], r.board_id);
    assert!(data(&h)["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["finding"]["message"] == r.findings[0].message));
}
#[test]
fn ignored_and_flagged_states_are_retained() {
    let mut r = report();
    r.findings[0].state = "ignored".into();
    r.findings[1].state = "flagged".into();
    let d = data(&contact_sheet::render(SOURCE, &r).unwrap());
    let states: Vec<_> = d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["finding"]["state"].as_str().unwrap())
        .collect();
    assert!(states.contains(&"ignored") && states.contains(&"flagged"));
}
#[test]
fn zero_findings_and_source_mismatch() {
    let mut r = report();
    r.findings.clear();
    assert_eq!(
        data(&contact_sheet::render(SOURCE, &r).unwrap())["findings"],
        serde_json::json!([])
    );
    assert!(contact_sheet::render(&(SOURCE.to_owned() + " "), &r).is_err());
}
