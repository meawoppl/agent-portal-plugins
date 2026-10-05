//! Ports of upstream tests/test_netclass_diagnostics.py and the
//! netclass_templates parts of tests/test_netclass.py.

use kct::core::netclass_diagnostics::{diagnose_netclasses, diagnose_netclasses_for_nets};
use kct::core::netclass_templates::{
    apply_design_template, get_available_design_types, get_design_template, get_netclass_summary,
    project_file, AUDIO_TEMPLATE, DESIGN_TYPE_TEMPLATES, POWER_SUPPLY_TEMPLATE, RF_TEMPLATE,
};
use serde_json::{json, Value};

fn project(patterns: Value) -> Value {
    json!({
        "net_settings": {
            "classes": [{"name": "Default"}, {"name": "Power"}],
            "netclass_patterns": patterns,
        }
    })
}

fn codes(report: &Value) -> Vec<String> {
    report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect()
}

fn nets(names: &[&str]) -> Value {
    json!(names)
}

#[test]
fn missing_target_and_duplicate_overlap_witness() {
    let data = project(json!([
        {"pattern": "USB_*", "netclass": "Missing"},
        {"pattern": "USB_*", "netclass": "Power"}
    ]));
    let r = diagnose_netclasses(&data, Some(&nets(&["USB_D", "USB", "OTHER"])));
    let names: Vec<&str> = r["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Default", "Power"]);
    assert_eq!(r["patterns"][0]["index"], 0);
    assert_eq!(r["patterns"][1]["index"], 1);
    assert_eq!(r["patterns"][0]["target_defined"], false);
    assert_eq!(r["patterns"][0]["matches"], json!(["USB", "USB_D"]));
    assert_eq!(r["patterns"][1]["matches"], json!(["USB", "USB_D"]));
    assert!(codes(&r).contains(&"undefined_class".to_string()));
    assert!(!r.to_string().contains("effective_class"));
}

#[test]
fn missing_settings_and_default_are_not_fabricated() {
    let r = diagnose_netclasses(&json!({}), None);
    assert_eq!(r["settings_status"], "missing");
    assert_eq!(r["classes"], json!([]));
    assert_eq!(r["default_status"], "absent");
    assert_eq!(
        diagnose_netclasses(&project(json!([])), None)["default_status"],
        "declared"
    );
}

#[test]
fn inventory_absent_empty_and_no_match() {
    let data = project(json!([{"pattern": "USB", "netclass": "Power"}]));
    assert_eq!(
        diagnose_netclasses(&data, None)["patterns"][0]["matches"],
        Value::Null
    );
    for inv in [nets(&[]), nets(&["USB_D"])] {
        let r = diagnose_netclasses(&data, Some(&inv));
        assert_eq!(r["patterns"][0]["matches"], json!([]));
        assert!(codes(&r).contains(&"no_matches".to_string()));
    }
}

#[test]
fn malformed_containers() {
    for data in [
        Value::Null,
        json!([]),
        json!(1),
        json!({"net_settings": null}),
        json!({"net_settings": []}),
        json!({"net_settings": {"classes": 4, "netclass_patterns": {}, "netclass_assignments": []}}),
    ] {
        let before = data.clone();
        assert!(!diagnose_netclasses(&data, None)["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(data, before);
    }
}

#[test]
fn malformed_siblings_duplicates_and_explicit_assignments() {
    let data = json!({
        "net_settings": {
            "classes": [null, {}, {"name": "Power"}, {"name": "Power"}],
            "netclass_patterns": [
                null,
                {"pattern": 2, "netclass": "Missing"},
                {"pattern": "USB", "netclass": "Power"}
            ],
            "netclass_assignments": {"USB": ["Power", "Missing", "Power", null], "VCC": "Power"}
        }
    });
    let r = diagnose_netclasses(&data, Some(&nets(&["USB"])));
    assert_eq!(r["classes"].as_array().unwrap().len(), 4);
    assert_eq!(r["patterns"].as_array().unwrap().len(), 3);
    assert_eq!(r["patterns"][2]["matches"], json!(["USB"]));
    let c = codes(&r);
    for code in [
        "duplicate_class",
        "undefined_class",
        "unsupported_assignment",
    ] {
        assert!(c.contains(&code.to_string()), "{code}");
    }
    let targets: Vec<Value> = (0..4)
        .map(|i| r["assignments"][i]["target"].clone())
        .collect();
    assert_eq!(
        targets,
        vec![
            json!("Power"),
            json!("Missing"),
            json!("Power"),
            Value::Null
        ]
    );
}

#[test]
fn deep_immutability_and_determinism() {
    let data = project(json!([{"pattern": "USB", "netclass": "Power", "extra": [1]}]));
    let before = data.clone();
    let r = diagnose_netclasses(&data, Some(&nets(&["Z", "USB", "USB"])));
    assert_eq!(r, diagnose_netclasses(&data, Some(&nets(&["USB", "Z"]))));
    let mut r2 = r.clone();
    r2["patterns"][0]["entry"]["extra"]
        .as_array_mut()
        .unwrap()
        .push(json!(2));
    assert_eq!(data, before);
}

#[test]
fn invalid_inventory_is_not_an_empty_inventory() {
    let r = diagnose_netclasses(
        &project(json!([{"pattern": "*", "netclass": "Power"}])),
        Some(&json!("USB")),
    );
    assert_eq!(r["inventory_status"], "invalid");
    assert_eq!(r["patterns"][0]["matches"], Value::Null);
    let r = diagnose_netclasses(&project(json!([])), Some(&json!(["USB", 3])));
    assert_eq!(r["inventory_status"], "invalid");
}

#[test]
fn native_oracle() {
    let corpus: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/netclass_diagnostics/oracle.json"
        ))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(corpus["kicad_version"], "10.0.5");
    let unsupported = ["USB.1", r"USB\*", "USB[0-9]", "^USB$", "/USB/", "a**"];
    let cases = corpus["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let pattern = case["pattern"].as_str().unwrap();
        let net = case["net"].as_str().unwrap();
        let r = diagnose_netclasses_for_nets(
            &project(json!([{"pattern": pattern, "netclass": "Power"}])),
            Some(&[net]),
        );
        let row = &r["patterns"][0];
        if unsupported.contains(&pattern) {
            assert_eq!(row["status"], "unsupported", "{case}");
            assert_eq!(row["matches"], Value::Null);
        } else {
            let expected = if case["matches"].as_bool().unwrap() {
                json!([net])
            } else {
                json!([])
            };
            assert_eq!(row["matches"], expected, "{case}");
        }
    }
}

#[test]
fn unsupported_pattern_retains_undefined_target_and_original_fields() {
    let data = project(json!([{"pattern": r"USB\*", "netclass": "Absent", "custom": 7}]));
    let r = diagnose_netclasses(&data, Some(&nets(&[])));
    assert_eq!(
        r["patterns"][0]["entry"],
        data["net_settings"]["netclass_patterns"][0]
    );
    let c = codes(&r);
    assert!(c.contains(&"unsupported_pattern".into()) && c.contains(&"undefined_class".into()));
    assert!(!c.contains(&"no_matches".into()));
}

#[test]
fn explicit_assignment_membership_empty_and_legacy_forms() {
    let mut data = project(json!([]));
    data["net_settings"]["netclass_assignments"] =
        json!({"USB": ["Power", "Power"], "OTHER": [], "OLD": "Power"});
    let r = diagnose_netclasses(&data, Some(&nets(&[])));
    for i in 0..3 {
        assert_eq!(r["assignments"][i]["net_present"], false);
    }
    assert_eq!(r["assignments"][2]["entry"], json!([]));
    assert_eq!(r["assignments"][3]["status"], "unsupported");
    assert_eq!(
        diagnose_netclasses(&data, None)["assignments"][0]["net_present"],
        Value::Null
    );
}

#[test]
fn legacy_class_assignments_cannot_silently_disappear() {
    let mut data = project(json!([]));
    data["net_settings"]["classes"][0]["nets"] = json!(["USB"]);
    assert!(codes(&diagnose_netclasses(&data, None)).contains(&"unsupported_assignment".into()));
}

#[test]
fn invalid_pattern_and_target_entries_are_retained() {
    for patterns in [
        json!([{}, {"pattern": "USB", "netclass": []}]),
        json!(["bad", 2]),
    ] {
        let data = project(patterns.clone());
        let before = data.clone();
        let r = diagnose_netclasses(&data, Some(&nets(&["USB"])));
        let entries: Vec<Value> = r["patterns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["entry"].clone())
            .collect();
        assert_eq!(Value::Array(entries), patterns);
        assert!(codes(&r).contains(&"invalid_target".into()));
        assert_eq!(data, before);
    }
}

// ------------------------------------------------------------------ templates

#[test]
fn design_type_templates() {
    let types = get_available_design_types();
    for t in ["audio", "power_supply", "digital", "mixed_signal", "rf"] {
        assert!(types.contains(&t));
    }
    let audio = get_design_template("audio").unwrap();
    assert_eq!(audio.name, "audio");
    assert!(!audio.netclasses.is_empty());
    assert!(get_design_template("invalid_type")
        .unwrap_err()
        .to_string()
        .contains("Unknown design type"));
    let names: Vec<&str> = AUDIO_TEMPLATE.netclasses.iter().map(|n| n.name).collect();
    for n in ["Power", "Ground", "Audio", "I2S"] {
        assert!(names.contains(&n));
    }
    assert!(POWER_SUPPLY_TEMPLATE
        .netclasses
        .iter()
        .any(|n| n.name == "HighCurrent"));
    assert!(RF_TEMPLATE.netclasses.iter().any(|n| n.name == "RF"));
    for (name, t) in DESIGN_TYPE_TEMPLATES.iter() {
        for nc in t.netclasses {
            if nc.name != "Default" {
                assert!(!nc.patterns.is_empty(), "{name}.{}", nc.name);
            }
        }
    }
}

#[test]
fn apply_design_template_adds_classes_and_patterns() {
    let mut data = json!({});
    apply_design_template(&mut data, "audio", true).unwrap();
    let classes = project_file::get_netclass_definitions(&mut data).clone();
    let names: Vec<&str> = classes
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for n in ["Default", "Power", "Audio", "I2S"] {
        assert!(names.contains(&n));
    }
    let default = classes.iter().find(|c| c["name"] == "Default").unwrap();
    assert_eq!(default["track_width"], 0.2);
    assert_eq!(default["clearance"], 0.15);
    let patterns = project_file::get_netclass_patterns(&mut data).clone();
    assert!(patterns.iter().any(|p| p["netclass"] == "Power"));

    // Re-applying does not duplicate patterns or classes.
    let count = patterns.len();
    apply_design_template(&mut data, "audio", true).unwrap();
    assert_eq!(project_file::get_netclass_patterns(&mut data).len(), count);
    assert_eq!(
        project_file::get_netclass_definitions(&mut data).len(),
        classes.len()
    );

    let mut data = json!({});
    apply_design_template(&mut data, "power_supply", true).unwrap();
    let classes = project_file::get_netclass_definitions(&mut data);
    let hc = classes.iter().find(|c| c["name"] == "HighCurrent").unwrap();
    assert_eq!(hc["track_width"], 0.8);
}

#[test]
fn netclass_summary() {
    let mut data = json!({});
    apply_design_template(&mut data, "audio", true).unwrap();
    let summary = get_netclass_summary(&mut data);
    assert!(summary.len() > 1);
    for row in &summary {
        for k in ["name", "track_width", "clearance", "pattern_count"] {
            assert!(row.get(k).is_some());
        }
    }
    let power = summary.iter().find(|r| r["name"] == "Power").unwrap();
    assert_eq!(power["pattern_count"], 6);
}
