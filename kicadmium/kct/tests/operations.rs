//! `kct::operations` against goldens captured from upstream
//! `kicad_tools.operations` (`fixtures/operations/golden.json`): net tracing
//! (including CPython-hash `Net_XXXX` names), `find_net`, symbol
//! replacement with/without a library (lib_symbols swap, pin
//! reconciliation, wire adjustment), and pin-mapping comparison.
//! Schematic/library inputs are the constants from upstream
//! `tests/test_operations.py`.

use std::path::{Path, PathBuf};

use kct::operations::{compare_symbols, find_net, replace_symbol_lib_id, trace_nets};
use kct::schema::schematic::Schematic;
use serde_json::{json, Value};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn golden() -> serde_json::Map<String, Value> {
    let text = std::fs::read_to_string(fixtures().join("operations/golden.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn r4(v: f64) -> f64 {
    kct::pyjson::py_round(v, 4)
}

fn mask_uuids(text: &str) -> String {
    let re = regex::Regex::new(r#"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"#)
        .unwrap();
    re.replace_all(text, "<UUID>").into_owned()
}

#[test]
fn trace_and_find_nets_match_upstream() {
    let g = golden();
    let mut failures = Vec::new();
    for rel in [
        "simple_rc.kicad_sch",
        "projects/test_project.kicad_sch",
        "projects/hierarchical_main.kicad_sch",
    ] {
        let sch = Schematic::load(fixtures().join(rel)).unwrap();
        let got: Vec<Value> = trace_nets(&sch)
            .iter()
            .map(|n| {
                json!({
                    "name": n.name,
                    "pins": n.pin_count(),
                    "wires": n.wires.len(),
                    "has_label": n.has_label,
                    "connections": n.connections.iter().map(|c| json!([c.kind, c.reference, r4(c.point.0), r4(c.point.1)])).collect::<Vec<_>>(),
                })
            })
            .collect();
        let want = &g[&format!("trace:{rel}")];
        if &Value::Array(got.clone()) != want {
            failures.push(format!(
                "trace {rel}:\n got  {}\n want {want}",
                Value::Array(got)
            ));
        }
        for (k, want) in g
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("find:{rel}:")))
        {
            let label = k.rsplit(':').next().unwrap();
            let got = match find_net(&sch, label) {
                None => Value::Null,
                Some(n) => json!({"name": n.name, "wires": n.wires.len()}),
            };
            if &got != want {
                failures.push(format!("{k}: got {got} want {want}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn replace_symbol_lib_id_matches_upstream() {
    let g = golden();
    let ops = fixtures().join("operations");
    let mut failures = Vec::new();
    let mut checked = 0;
    for (key, want) in g.iter().filter(|(k, _)| k.starts_with("replace:")) {
        let mut parts = key.splitn(4, ':');
        parts.next();
        let sch_name = parts.next().unwrap();
        let lib = parts.next().unwrap();
        let new_id = parts.next().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let sch = tmp.path().join(sch_name);
        std::fs::copy(ops.join(sch_name), &sch).unwrap();
        let lib_path = (lib != "None").then(|| ops.join(lib));
        let res = replace_symbol_lib_id(
            &sch,
            "U1",
            new_id,
            Some("NEWVAL"),
            None,
            false,
            lib_path.as_deref(),
        );
        checked += 1;
        match (res, want.get("error")) {
            (Err(_), Some(_)) => {}
            (Err(e), None) => failures.push(format!("{key}: unexpected error {e:#}")),
            (Ok(_), Some(_)) => failures.push(format!("{key}: expected error {}", want["message"])),
            (Ok(r), None) => {
                let got = json!({
                    "reference": r.reference,
                    "old_lib_id": r.old_lib_id,
                    "new_lib_id": r.new_lib_id,
                    "old_pin_count": r.old_pin_count,
                    "new_pin_count": r.new_pin_count,
                    "preserved_properties": r.preserved_properties,
                    "changes_made": r.changes_made,
                    "lib_symbol_updated": r.lib_symbol_updated,
                    "pin_type_changes": r.pin_type_changes.iter().map(|c| json!({
                        "pin_number": c.pin_number, "pin_name": c.pin_name,
                        "old_type": c.old_type, "new_type": c.new_type})).collect::<Vec<_>>(),
                    "wires_adjusted": r.wires_adjusted,
                });
                let mut w = want.clone();
                let want_text = w.as_object_mut().unwrap().remove("text").unwrap();
                if got != w {
                    failures.push(format!("{key}:\n got  {got}\n want {w}"));
                }
                let text = std::fs::read_to_string(&sch).unwrap();
                if mask_uuids(&text) != mask_uuids(want_text.as_str().unwrap()) {
                    let (gt, wt) = (mask_uuids(&text), mask_uuids(want_text.as_str().unwrap()));
                    let line = gt
                        .lines()
                        .zip(wt.lines())
                        .enumerate()
                        .find(|(_, (a, b))| a != b)
                        .map(|(i, (a, b))| format!("line {}: got {a:?} want {b:?}", i + 1))
                        .unwrap_or_else(|| "length differs".into());
                    failures.push(format!("{key}: file text differs: {line}"));
                }
            }
        }
    }
    assert!(checked > 40);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn compare_symbols_matches_upstream() {
    let g = golden();
    let ops = fixtures().join("operations");
    let mut failures = Vec::new();
    for (key, want) in g.iter().filter(|(k, _)| k.starts_with("compare:")) {
        let mut parts = key.splitn(3, ':');
        parts.next();
        let (a, b) = (parts.next().unwrap(), parts.next().unwrap());
        let got = match compare_symbols(&ops.join(a), &ops.join(b)) {
            Ok(r) => serde_json::from_str::<Value>(&kct::pyjson::dumps(&r.to_dict())).unwrap(),
            Err(e) => json!({"error": e.to_string()}),
        };
        if &got != want {
            failures.push(format!("{key}:\n got  {got}\n want {want}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn pin_normalization_and_categories() {
    use kct::operations::Pin;
    let p = |n: &str| Pin::new("1", n, "Input");
    assert_eq!(p("~{RESET}").normalized_name(), "RESET");
    assert_eq!(p("PVDD_39").normalized_name(), "PVDD");
    assert_eq!(p("IN+/A").normalized_name(), "INP_A");
    assert_eq!(p("PVDD").function_category(), "power_positive");
    assert_eq!(p("PGND").function_category(), "power_ground");
    assert_eq!(p("BSTA").function_category(), "bootstrap");
    assert_eq!(p("NC").function_category(), "no_connect");
    assert_eq!(p("XYZ").function_category(), "other");
    let _ = Path::new("");
}
