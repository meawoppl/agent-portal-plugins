//! `kct check --format json --drc-only [--mfr X]` and `kct detect-mistakes
//! --format json` parity with upstream kicad-tools on every fixture board.
//!
//! Goldens in `fixtures/check/` were captured from the upstream Python CLI
//! (`<key>.<variant>.json`, exit codes in `index.json`); the `file` field is
//! normalised to `<FIXTURES>/<relative path>`.
//!
//! Intentional divergence: without `--mfr`, kicadmium enforces a board's own
//! `.kicad_pro`/`.kicad_dru` minima over the auto-selected profile (see
//! `manufacturers::project_rules`), so the `default` goldens of fixtures with
//! project rules (conformance issue5398, issue-5362 witness) are native.

use std::path::PathBuf;

use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn index() -> Vec<Value> {
    let text = std::fs::read_to_string(fixtures().join("check/index.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn first_difference(path: &str, a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<&String> = x.keys().collect();
            let ky: Vec<&String> = y.keys().collect();
            if kx != ky {
                return Some(format!("{path}: keys {kx:?} != {ky:?}"));
            }
            x.iter()
                .find_map(|(k, v)| first_difference(&format!("{path}.{k}"), v, &y[k]))
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: len {} != {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (p, q))| first_difference(&format!("{path}[{i}]"), p, q))
        }
        _ => (a != b).then(|| format!("{path}: {a} != {b}")),
    }
}

fn run_variant(variant: &str, extra: &[&str]) {
    let tmp = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    for entry in index() {
        let rel = entry["board"].as_str().unwrap();
        let key = entry["key"].as_str().unwrap();
        let board = fixtures().join(rel);
        let out = tmp.path().join(format!("{key}.json"));
        let mut args: Vec<String> = vec!["check".into(), board.display().to_string()];
        args.extend(
            ["--format", "json", "--drc-only"]
                .iter()
                .map(|s| s.to_string()),
        );
        args.extend(extra.iter().map(|s| s.to_string()));
        args.extend(["--output".to_string(), out.display().to_string()]);
        let code = kct::cli::run(args).unwrap();
        let want_code = entry["codes"][variant].as_i64().unwrap();
        if i64::from(code) != want_code {
            failures.push(format!("{rel} [{variant}]: exit {code} != {want_code}"));
        }
        let mut got: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        got["file"] = Value::String(format!("<FIXTURES>/{rel}"));
        let gold_path = fixtures().join(format!("check/{key}.{variant}.json"));
        let want: Value =
            serde_json::from_str(&std::fs::read_to_string(gold_path).unwrap()).unwrap();
        if let Some(d) = first_difference("$", &got, &want) {
            failures.push(format!("{rel} [{variant}]: {d}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn check_default_mfr_matches_upstream() {
    run_variant("default", &[]);
}

#[test]
fn check_jlcpcb_matches_upstream() {
    run_variant("jlcpcb", &["--mfr", "jlcpcb"]);
}

#[test]
fn check_pcbway_matches_upstream() {
    run_variant("pcbway", &["--mfr", "pcbway"]);
}

#[test]
fn detect_mistakes_matches_upstream() {
    use kct::explain::mistakes::detect_mistakes_with_coverage;
    let mut failures = Vec::new();
    for entry in index() {
        let rel = entry["board"].as_str().unwrap();
        let key = entry["key"].as_str().unwrap();
        let pcb = kct::schema::pcb::Pcb::load(fixtures().join(rel)).unwrap();
        let (mistakes, coverage) = detect_mistakes_with_coverage(&pcb);
        let gold_path = fixtures().join(format!("check/{key}.mistakes.json"));
        let want: Value =
            serde_json::from_str(&std::fs::read_to_string(gold_path).unwrap()).unwrap();
        let got_m: Value = serde_json::from_str(&kct::pyjson::dumps_indent(
            &kct::pyjson::Json::Arr(mistakes.iter().map(|m| m.to_dict()).collect()),
            1,
        ))
        .unwrap();
        let got_c: Value = serde_json::from_str(&kct::pyjson::dumps_indent(
            &kct::pyjson::Json::Arr(coverage.iter().map(|c| c.to_dict()).collect()),
            1,
        ))
        .unwrap();
        for (name, got, want) in [
            ("mistakes", &got_m, &want["mistakes"]),
            ("coverage", &got_c, &want["coverage"]),
        ] {
            if let Some(d) = first_difference(&format!("$.{name}"), got, want) {
                failures.push(format!("{rel}: {d}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
