//! `kct::recovery` against goldens captured from upstream
//! `kicad_tools.recovery` (`fixtures/recovery/golden.json`, generated with
//! PYTHONHASHSEED=0): strategy application (mirror pinned to KiCad's own
//! flip fixture, rotate, move, move-multiple, reorder-pins), safety checks,
//! board bounds, strategy generation for every failure cause with and
//! without a movement budget, and pattern matching.
//!
//! Upstream orders some net lists by Python `set` iteration; those lists are
//! compared as sorted sets.

use std::path::PathBuf;

use kct::pyjson::Json;
use kct::recovery::patterns::PatternMatcher;
use kct::recovery::*;
use kct::schema::pcb::Pcb;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

const CHARLIE: &str =
    "historical_demo_boards/02-charlieplex-led/output/charlieplex_3x3_routed.kicad_pcb";
const FRONT: &str = "mirror_flip/mirror_front.kicad_pcb";

fn golden() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixtures().join("recovery/golden.json")).unwrap())
        .unwrap()
}

/// Numbers compared by value (Python int vs float); `affected_nets` sorted.
fn norm(v: &Value) -> Value {
    match v {
        Value::Number(n) => Value::from(n.as_f64().unwrap()),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    let mut v = norm(v);
                    if k == "affected_nets" {
                        if let Value::Array(a) = &mut v {
                            a.sort_by_key(|x| x.to_string());
                        }
                    }
                    (k.clone(), v)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn to_value(j: &Json) -> Value {
    serde_json::from_str(&kct::pyjson::dumps(j)).unwrap()
}

fn obj(pairs: &[(&str, Json)]) -> Json {
    let mut d = Json::obj();
    for (k, v) in pairs {
        d.set(k, v.clone());
    }
    d
}

fn strat(kind: StrategyType, actions: Vec<Action>) -> ResolutionStrategy {
    let mut s = ResolutionStrategy::new(kind, Difficulty::Medium, 1.0);
    s.actions = actions;
    s
}

fn run_apply(board: &str, s: &ResolutionStrategy) -> Value {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("b.kicad_pcb");
    std::fs::copy(fixtures().join(board), &path).unwrap();
    let mut pcb = Pcb::load(&path).unwrap();
    let app = StrategyApplicator::new();
    let safe = app.is_safe_to_apply(s, &pcb).unwrap();
    let r = app.apply_strategy(&mut pcb, s).unwrap();
    pcb.save(Some(&path)).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    serde_json::json!({
        "safe": safe, "success": r.success, "moved": r.components_moved,
        "message": r.message, "text": text,
    })
}

fn check_apply(g: &Value, name: &str, got: Value, failures: &mut Vec<String>) {
    let want = &g[&format!("apply:{name}")];
    for k in ["safe", "success", "moved", "message"] {
        if got[k] != want[k] {
            failures.push(format!("{name}.{k}: got {} want {}", got[k], want[k]));
        }
    }
    let text = got["text"].as_str().unwrap();
    if let Some(w) = want.get("text") {
        if text != w.as_str().unwrap() {
            let line = text
                .lines()
                .zip(w.as_str().unwrap().lines())
                .enumerate()
                .find(|(_, (a, b))| a != b)
                .map(|(i, (a, b))| format!("line {}: got {a:?} want {b:?}", i + 1))
                .unwrap_or_else(|| "length differs".into());
            failures.push(format!("{name}: board text differs: {line}"));
        }
    } else {
        let sha: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if sha != want["sha256"].as_str().unwrap() {
            failures.push(format!("{name}: board sha256 differs"));
        }
    }
}

#[test]
fn applicator_matches_upstream() {
    let g = golden();
    let mut failures = Vec::new();
    let mirror = strat(
        StrategyType::MirrorComponent,
        vec![Action::new("mirror", "U1", Json::obj())],
    );
    check_apply(&g, "mirror_front", run_apply(FRONT, &mirror), &mut failures);
    let rotate = strat(
        StrategyType::RotateComponent,
        vec![Action::new(
            "rotate",
            "U1",
            obj(&[("rotation_delta", Json::Int(90))]),
        )],
    );
    check_apply(&g, "rotate_front", run_apply(FRONT, &rotate), &mut failures);

    let pcb = Pcb::load(fixtures().join(CHARLIE)).unwrap();
    let (f0, f1) = (&pcb.footprints()[0], &pcb.footprints()[1]);
    let mv = |r: &str, x: f64, y: f64| {
        Action::new(
            "move",
            r,
            obj(&[("x", Json::Float(x)), ("y", Json::Float(y))]),
        )
    };
    let cases = vec![
        (
            "move_charlie",
            strat(
                StrategyType::MoveComponent,
                vec![mv(&f0.reference, f0.position.0 + 2.5, f0.position.1 - 1.25)],
            ),
        ),
        (
            "move_far",
            strat(
                StrategyType::MoveComponent,
                vec![mv(&f0.reference, f0.position.0 + 25.0, f0.position.1)],
            ),
        ),
        (
            "move_multi",
            strat(
                StrategyType::MoveMultiple,
                vec![
                    mv(&f0.reference, f0.position.0 + 1.0, f0.position.1),
                    Action::new(
                        "move",
                        "NOPE",
                        obj(&[("x", Json::Int(1)), ("y", Json::Int(2))]),
                    ),
                    mv(&f1.reference, f1.position.0, f1.position.1 + 1.5),
                ],
            ),
        ),
        (
            "reorder_charlie",
            strat(
                StrategyType::ReorderPins,
                vec![Action::new(
                    "reorder_pins",
                    f1.reference.as_str(),
                    obj(&[(
                        "pad_map",
                        obj(&[(f1.pads[0].number.as_str(), Json::Str("NEW_NET".into()))]),
                    )]),
                )],
            ),
        ),
        (
            "reorder_missing",
            strat(
                StrategyType::ReorderPins,
                vec![Action::new(
                    "reorder_pins",
                    f1.reference.as_str(),
                    obj(&[("pad_map", obj(&[("999", Json::Str("X".into()))]))]),
                )],
            ),
        ),
        (
            "rotate_charlie",
            strat(
                StrategyType::RotateComponent,
                vec![Action::new(
                    "rotate",
                    f1.reference.as_str(),
                    obj(&[("rotation_delta", Json::Int(180))]),
                )],
            ),
        ),
        (
            "reroute_unsupported",
            strat(
                StrategyType::RerouteNet,
                vec![Action::new("reroute", "GND", Json::obj())],
            ),
        ),
    ];
    for (name, s) in cases {
        check_apply(&g, name, run_apply(CHARLIE, &s), &mut failures);
    }
    let bounds = StrategyApplicator::get_board_bounds(&pcb).unwrap();
    if norm(&to_value(&bounds.to_dict())) != norm(&g["bounds"]) {
        failures.push(format!("bounds: {:?}", bounds));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn strategy_generation_and_patterns_match_upstream() {
    let g = golden();
    let pcb = Pcb::load(fixtures().join(CHARLIE)).unwrap();
    let (f0, f1) = (&pcb.footprints()[0], &pcb.footprints()[1]);
    let (cx, cy) = f0.position;
    let (dx, dy) = f1.position;
    let blockers = vec![
        BlockingElement::new(
            "component",
            Some(&f0.reference),
            None,
            Rectangle::new(cx - 1.0, cy - 0.5, cx + 1.0, cy + 0.5),
            true,
        ),
        BlockingElement::new(
            "component",
            Some(&f1.reference),
            None,
            Rectangle::new(dx - 1.0, dy - 1.0, dx + 1.0, dy + 1.0),
            true,
        ),
        BlockingElement::new(
            "trace",
            None,
            Some("ROW1"),
            Rectangle::new(cx, cy, cx + 3.0, cy + 0.2),
            false,
        ),
        BlockingElement::new(
            "via",
            None,
            Some("COL2"),
            Rectangle::new(cx, cy, cx + 0.6, cy + 0.6),
            false,
        ),
    ];
    let gen = StrategyGenerator::new();
    let matcher = PatternMatcher::new();
    let mut failures = Vec::new();
    for &cause in FailureCause::ALL {
        for mm in [None, Some(3.0)] {
            let mut fa = FailureAnalysis::new(
                cause,
                0.8,
                (cx + 0.5, cy + 0.25),
                Rectangle::new(cx - 2.0, cy - 2.0, cx + 2.0, cy + 2.0),
            );
            fa.blocking_elements = blockers.clone();
            fa.congestion_score = 0.9;
            fa.net = Some("ROW2".into());
            fa.best_attempt = Some(PathAttempt {
                start: (0.0, 0.0),
                end: (1.0, 1.0),
                reached: 0.5,
                failure_point: Some((0.5, 0.5)),
                failure_reason: Some("blocked".into()),
            });
            let key = format!(
                "{}:{}",
                cause.value(),
                mm.map_or("None".to_string(), |m: f64| format!("{m:.1}"))
            );
            let want = &g["generate"][&key];
            let strategies: Vec<Value> = gen
                .generate_strategies(&pcb, &fa, mm)
                .iter()
                .map(|s| to_value(&s.to_dict()))
                .collect();
            let patterns: Vec<Value> = matcher
                .match_patterns(&fa)
                .iter()
                .map(|m| to_value(&m.to_dict()))
                .collect();
            for (part, got) in [
                ("analysis", to_value(&fa.to_dict())),
                ("strategies", Value::Array(strategies)),
                ("patterns", Value::Array(patterns)),
            ] {
                if norm(&got) != norm(&want[part]) {
                    failures.push(format!("{key}.{part}:\n got  {got}\n want {}", want[part]));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
