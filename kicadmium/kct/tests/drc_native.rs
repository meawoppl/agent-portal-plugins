//! Parity of the native DRC helpers with upstream kicad-tools
//! (`drc.different_net_short`, `drc.incremental` pure-Python path).
//! Goldens: `fixtures/drc_native/goldens.json`, captured from upstream.

use std::path::PathBuf;

use kct::drc::different_net_short::find_different_net_shorts;
use kct::drc::incremental::IncrementalDRC;
use kct::schema::pcb::Pcb;
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn shorts_and_incremental_match_upstream() {
    let text = std::fs::read_to_string(fixtures().join("drc_native/goldens.json")).unwrap();
    let cases: Vec<Value> = serde_json::from_str(&text).unwrap();
    assert!(cases.len() > 20);
    let rules = kct::manufacturers::rules("jlcpcb", 2, 1.0).unwrap();
    for case in &cases {
        let board = case["board"].as_str().unwrap();
        let pcb = Pcb::load(fixtures().join(board)).unwrap();

        let shorts = find_different_net_shorts(&pcb, 0.2);
        let want = case["shorts"].as_array().unwrap();
        assert_eq!(shorts.len(), want.len(), "{board}: short count");
        for (i, (s, w)) in shorts.iter().zip(want).enumerate() {
            if s.x != w["x"].as_f64().unwrap() {
                eprintln!("{board} #{i}: rust {s:?} upstream {w}");
            }
            assert_eq!(s.kind, w["kind"].as_str().unwrap(), "{board}");
            assert_eq!(s.net_a_name, w["a"].as_str().unwrap(), "{board}");
            assert_eq!(s.net_b_name, w["b"].as_str().unwrap(), "{board}");
            assert_eq!(s.layer, w["layer"].as_str().unwrap(), "{board}");
            assert_eq!(s.x, w["x"].as_f64().unwrap(), "{board}");
            assert_eq!(s.y, w["y"].as_f64().unwrap(), "{board}");
            assert_eq!(s.gap, w["gap"].as_f64().unwrap(), "{board}");
        }

        let mut drc = IncrementalDRC::new(&pcb, rules.clone());
        let mut got: Vec<_> = drc
            .full_check()
            .into_iter()
            .map(|v| {
                (
                    v.items.clone(),
                    v.location,
                    v.message.clone(),
                    v.actual_value,
                )
            })
            .collect();
        got.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1 .0.total_cmp(&b.1 .0))
                .then(a.1 .1.total_cmp(&b.1 .1))
        });
        let want = case["incremental"].as_array().unwrap();
        assert_eq!(got.len(), want.len(), "{board}: incremental count");
        for (g, w) in got.iter().zip(want) {
            let items: Vec<String> = w["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i.as_str().unwrap().to_string())
                .collect();
            assert_eq!(g.0, items, "{board}");
            assert_eq!(g.1 .0, w["location"][0].as_f64().unwrap(), "{board}");
            assert_eq!(g.1 .1, w["location"][1].as_f64().unwrap(), "{board}");
            assert_eq!(g.2, w["message"].as_str().unwrap(), "{board}");
            assert_eq!(g.3, w["actual"].as_f64(), "{board}");
        }

        if let Some(d) = case["delta"].as_object() {
            let reference = d["ref"].as_str().unwrap();
            let fp = pcb.get_footprint(reference).unwrap();
            let delta = drc.check_move(reference, fp.position.0 + 1.5, fp.position.1 - 0.75);
            assert_eq!(
                delta.new_violations.len() as u64,
                d["new"].as_u64().unwrap(),
                "{board}"
            );
            assert_eq!(
                delta.resolved_violations.len() as u64,
                d["resolved"].as_u64().unwrap(),
                "{board}"
            );
            assert_eq!(delta.summary(), d["summary"].as_str().unwrap(), "{board}");
        }
    }
}
