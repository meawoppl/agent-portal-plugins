//! Parity of `validate::connectivity::ConnectivityValidator` with upstream
//! kicad-tools: the label-free physical pad partition
//! (`extract_pad_occurrences`) and `validate().to_dict()` / `summary()`.
//! Goldens: `fixtures/connectivity/goldens.json`, captured from upstream.

use std::path::PathBuf;

use kct::validate::connectivity::ConnectivityValidator;
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn connectivity_matches_upstream() {
    let text = std::fs::read_to_string(fixtures().join("connectivity/goldens.json")).unwrap();
    let cases: serde_json::Map<String, Value> = serde_json::from_str(&text).unwrap();
    assert!(cases.len() > 20);
    for (board, want) in &cases {
        let mut v = ConnectivityValidator::from_path(&fixtures().join(board)).unwrap();
        let (groups, _) = v.extract_pad_occurrences();
        let got: Value = serde_json::to_value(&groups).unwrap();
        assert_eq!(got, want["groups"], "{board}: pad partition");
        let res = v.validate(false);
        let dict: Value = serde_json::from_str(&kct::pyjson::dumps(&res.to_dict())).unwrap();
        assert_eq!(dict, want["validate"], "{board}: validate()");
        assert_eq!(res.summary(), want["summary"].as_str().unwrap(), "{board}: summary");
    }
}
