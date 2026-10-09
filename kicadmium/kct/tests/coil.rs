//! `kct coil spacing` / `kct coil resistance` on synthetic boards.

use std::path::Path;

fn board(body: &str) -> String {
    format!(
        "(kicad_pcb (version 20260206) (generator \"test\") (general (thickness 1.6)) \
         (layers (0 \"F.Cu\" signal) (4 \"In1.Cu\" signal) (2 \"B.Cu\" signal) (44 \"Edge.Cuts\" user)) \
         (net 0 \"\") (net 1 \"A\") (net 2 \"B\") \
         (gr_line (start 0 0) (end 100 0) (layer \"Edge.Cuts\") (width 0.1)) \
         (gr_line (start 100 0) (end 100 100) (layer \"Edge.Cuts\") (width 0.1)) \
         (gr_line (start 100 100) (end 0 100) (layer \"Edge.Cuts\") (width 0.1)) \
         (gr_line (start 0 100) (end 0 0) (layer \"Edge.Cuts\") (width 0.1)) \
         {body})"
    )
}
fn seg(a: (f64, f64), b: (f64, f64), w: f64, layer: &str, net: i64) -> String {
    format!(
        "(segment (start {} {}) (end {} {}) (width {w}) (layer \"{layer}\") (net {net}))",
        a.0, a.1, b.0, b.1
    )
}
fn arc(a: (f64, f64), m: (f64, f64), b: (f64, f64), w: f64, layer: &str, net: i64) -> String {
    format!(
        "(arc (start {} {}) (mid {} {}) (end {} {}) (width {w}) (layer \"{layer}\") (net {net}))",
        a.0, a.1, m.0, m.1, b.0, b.1
    )
}
fn via(at: (f64, f64), size: f64, drill: f64, net: i64) -> String {
    format!(
        "(via (at {} {}) (size {size}) (drill {drill}) (layers \"F.Cu\" \"B.Cu\") (net {net}))",
        at.0, at.1
    )
}
fn run(dir: &Path, body: &str, args: &[&str]) -> (i32, serde_json::Value) {
    let pcb = dir.join("t.kicad_pcb");
    std::fs::write(&pcb, board(body)).unwrap();
    let out = dir.join("report.json");
    let mut argv: Vec<String> = vec!["coil".into(), args[0].into(), pcb.display().to_string()];
    argv.extend(args[1..].iter().map(|s| s.to_string()));
    argv.push("--output".into());
    argv.push(out.display().to_string());
    let code = kct::cli::run(argv).unwrap();
    (
        code,
        serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap(),
    )
}
fn net<'a>(r: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    r["nets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["net"] == name)
        .unwrap()
}

#[test]
fn separate_chains_of_one_net_measure_their_gap() {
    let dir = tempfile::tempdir().unwrap();
    // Net A: two parallel 0.5 mm tracks 0.9 mm apart (gap 0.4, fine).
    // Net B: 0.6 mm apart (gap 0.1, a violation at the 0.2 mm limit).
    let body = seg((10., 10.), (30., 10.), 0.5, "F.Cu", 1)
        + &seg((10., 10.9), (30., 10.9), 0.5, "F.Cu", 1)
        + &seg((10., 50.), (30., 50.), 0.5, "F.Cu", 2)
        + &seg((10., 50.6), (30., 50.6), 0.5, "F.Cu", 2);
    let (code, r) = run(dir.path(), &body, &["spacing", "--min-gap", "0.2"]);
    assert_eq!(code, 1, "net B violates");
    assert!(!r["ok"].as_bool().unwrap());
    let a = net(&r, "A");
    assert!(a["ok"].as_bool().unwrap());
    assert!((a["min_gap_mm"].as_f64().unwrap() - 0.4).abs() < 1e-6);
    let b = net(&r, "B");
    assert!(!b["ok"].as_bool().unwrap());
    assert!((b["min_gap_mm"].as_f64().unwrap() - 0.1).abs() < 1e-6);
    assert_eq!(
        b["layers"][0]["violations"].as_u64().unwrap(),
        1,
        "one location cell"
    );
}

#[test]
fn a_u_turn_is_its_own_neighbour_but_the_legs_are_not() {
    let dir = tempfile::tempdir().unwrap();
    // Two 20 mm legs 0.9 mm apart joined by a half-circle arc (r = 0.45):
    // one chain. Samples near the arc are path neighbours (excluded); the
    // legs are far apart along the path, so the gap is 0.9 - 0.5 = 0.4.
    let body = seg((10., 10.), (30., 10.), 0.5, "F.Cu", 1)
        + &arc((30., 10.), (30.45, 10.45), (30., 10.9), 0.5, "F.Cu", 1)
        + &seg((30., 10.9), (10., 10.9), 0.5, "F.Cu", 1);
    let (code, r) = run(dir.path(), &body, &["spacing", "--min-gap", "0.2"]);
    assert_eq!(code, 0);
    let a = net(&r, "A");
    assert_eq!(a["layers"][0]["chains"].as_u64().unwrap(), 1);
    assert!(
        (a["min_gap_mm"].as_f64().unwrap() - 0.4).abs() < 1e-3,
        "{}",
        a["min_gap_mm"]
    );
    // Shrink the allowed path separation below the turn's length and the
    // samples either side of the arc count against each other.
    let (_, r) = run(
        dir.path(),
        &body,
        &["spacing", "--min-gap", "0.2", "--path-separation", "0.1"],
    );
    assert!(net(&r, "A")["min_gap_mm"].as_f64().unwrap() < 0.4);
}

#[test]
fn vias_measure_against_copper_they_do_not_land_on() {
    let dir = tempfile::tempdir().unwrap();
    // A 0.8 mm via at (20, 10) terminates the first track (excluded) and
    // sits 1.0 mm from a second same-net track: gap = 1.0 - 0.4 - 0.25 = 0.35.
    let body = seg((10., 10.), (20., 10.), 0.5, "F.Cu", 1)
        + &seg((15., 11.), (25., 11.), 0.5, "F.Cu", 1)
        + &via((20., 10.), 0.8, 0.4, 1);
    let (_, r) = run(dir.path(), &body, &["spacing", "--min-gap", "0.2"]);
    let a = net(&r, "A");
    let v = a["vias"]["min_gap_mm"].as_f64().unwrap();
    assert!((v - 0.35).abs() < 1e-6, "via gap {v}");
    // The track-to-track gap between the two chains is 1.0 - 0.5 = 0.5.
    assert!((a["layers"][0]["min_gap_mm"].as_f64().unwrap() - 0.5).abs() < 1e-6);
}

#[test]
fn resistance_from_length_width_copper_and_vias() {
    let dir = tempfile::tempdir().unwrap();
    // 100 mm × 1 mm × 35 µm at 20 °C: 1.68e-8 × 0.1 / (1e-3 × 35e-6) = 0.048 Ω.
    // One via, 1.6 mm board, 0.3 mm drill, 25 µm plating: 1.141e-3 Ω.
    let body = seg((0., 10.), (100., 10.), 1.0, "F.Cu", 1) + &via((100., 10.), 0.6, 0.3, 1);
    let (code, r) = run(dir.path(), &body, &["resistance"]);
    assert_eq!(code, 0);
    let a = net(&r, "A");
    assert!((a["tracks_ohm"].as_f64().unwrap() - 0.048).abs() < 1e-6);
    assert!((a["vias_ohm"].as_f64().unwrap() - 1.1408e-3).abs() < 1e-6);
    assert!((a["length_mm"].as_f64().unwrap() - 100.0).abs() < 1e-9);
    assert_eq!(a["via_count"], 1);
    // 70 °C scales copper by 1 + 0.00393 × 50; half-ounce copper doubles the track term.
    let (_, r) = run(
        dir.path(),
        &body,
        &[
            "resistance",
            "--temp",
            "70",
            "--copper-um-layer",
            "F.Cu=17.5",
        ],
    );
    let a = net(&r, "A");
    assert!((a["tracks_ohm"].as_f64().unwrap() - 0.048 * 2.0 * 1.1965).abs() < 1e-5);
    // Arc length counts as swept length, not the chord: a half circle of r = 10.
    let body = arc((0., 10.), (10., 20.), (20., 10.), 1.0, "F.Cu", 1);
    let (_, r) = run(dir.path(), &body, &["resistance"]);
    assert!(
        (net(&r, "A")["length_mm"].as_f64().unwrap() - std::f64::consts::PI * 10.0).abs() < 1e-6
    );
}
