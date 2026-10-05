//! End-to-end tests for the native autorouter (`kct route` / `route-auto`)
//! on upstream demo boards (boards/00-simple-led, boards/02-charlieplex-led).
//!
//! Upstream (Python + C++) reference on simple_led: 1 routable signal net
//! (LED_ANODE) after auto-pour, 100% completion on 2 layers, 8.72 mm.

use std::path::{Path, PathBuf};

use kct::router::core::{Autorouter, RouterConfig};
use kct::router::io::load_pcb_for_routing;
use kct::schema::pcb::Pcb;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/router")
        .join(name)
}

fn stage(dir: &Path, stem: &str) -> PathBuf {
    for ext in ["kicad_pcb", "kicad_pro"] {
        std::fs::copy(
            fixture(&format!("{stem}.{ext}")),
            dir.join(format!("{stem}.{ext}")),
        )
        .unwrap();
    }
    dir.join(format!("{stem}.kicad_pcb"))
}

fn run(args: &[&str]) -> i32 {
    kct::cli::run(args.iter().map(|s| s.to_string())).unwrap()
}

#[test]
fn route_simple_led_matches_upstream_completion() {
    let dir = tempfile::tempdir().unwrap();
    let pcb = stage(dir.path(), "simple_led");
    let out = dir.path().join("out.kicad_pcb");
    let code = run(&[
        "route",
        pcb.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--skip-drc",
        "--no-auto-pour",
        "--skip-nets",
        "GND,VCC",
        "-q",
    ]);
    assert_eq!(code, 0);
    let routed = Pcb::load(&out).unwrap();
    let led = routed.get_net_by_name("LED_ANODE").unwrap().number;
    let segs: Vec<_> = routed.segments_in_net(led).collect();
    assert!(!segs.is_empty());
    let len: f64 = segs.iter().map(|s| s.length()).sum();
    // Upstream routes LED_ANODE in 8.72 mm.
    assert!((len - 8.72).abs() < 0.5, "length {len}");
    // Artifact receipt (schema kicad-tools.route-artifacts.v1).
    let receipt: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("out.route.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["schema"], "kicad-tools.route-artifacts.v1");
    assert_eq!(receipt["route_exit_code"], 0);
    assert_eq!(receipt["files"]["board"]["state"], "present");
}

#[test]
fn charlieplex_routes_fully_and_clean() {
    let dir = tempfile::tempdir().unwrap();
    let pcb_path = stage(dir.path(), "charlieplex_3x3");
    let pcb = Pcb::load(&pcb_path).unwrap();
    let board = load_pcb_for_routing(&pcb, 2).unwrap();
    let cfg = RouterConfig {
        grid_resolution: 0.1,
        quiet: true,
        ..Default::default()
    };
    let mut router = Autorouter::new(board, cfg);
    router.route_all_negotiated(15);
    let stats = router.get_statistics();
    assert!(stats.nets_total >= 9);
    assert_eq!(stats.nets_routed, stats.nets_total, "{stats:?}");
    assert!(router.exact_violations().is_empty());
    assert!(router.conflicted_nets().is_empty());
    // Every emitted segment is 45-degree legal.
    for r in router.routes() {
        for s in &r.segments {
            let (dx, dy) = (s.x2 - s.x1, s.y2 - s.y1);
            assert!(
                kct::router::quantize::is_45_aligned(dx, dy, 0.01)
                    || kct::router::quantize::is_quantum_aligned(dx, dy)
            );
        }
    }
}

#[test]
fn route_cli_errors() {
    let dir = tempfile::tempdir().unwrap();
    let pcb = stage(dir.path(), "simple_led");
    let p = pcb.to_str().unwrap();
    // --nets and --skip-nets are mutually exclusive.
    assert_eq!(run(&["route", p, "--nets", "A", "--skip-nets", "B"]), 2);
    // Unknown --nets name.
    assert_eq!(run(&["route", p, "--nets", "NOPE", "-q", "--skip-drc"]), 1);
    // Missing file.
    assert_eq!(run(&["route", "/nonexistent.kicad_pcb"]), 1);
    // Bad choice -> argparse exit 2.
    assert_eq!(run(&["route", p, "--layers", "3"]), 2);
    // Dry run writes nothing.
    let out = dir.path().join("dry.kicad_pcb");
    assert_eq!(
        run(&["route", p, "-o", out.to_str().unwrap(), "--dry-run", "-q"]),
        0
    );
    assert!(!out.exists());
}

#[test]
fn route_auto_requires_output_or_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let pcb = stage(dir.path(), "simple_led");
    let p = pcb.to_str().unwrap();
    let before = std::fs::read_to_string(&pcb).unwrap();
    assert_eq!(run(&["route-auto", p, "--net", "LED_ANODE"]), 2);
    assert_eq!(std::fs::read_to_string(&pcb).unwrap(), before);
    let out = dir.path().join("auto.kicad_pcb");
    assert_eq!(
        run(&[
            "route-auto",
            p,
            "--net",
            "LED_ANODE",
            "-o",
            out.to_str().unwrap(),
            "--format",
            "json"
        ]),
        0
    );
    let routed = Pcb::load(&out).unwrap();
    let led = routed.get_net_by_name("LED_ANODE").unwrap().number;
    assert!(routed.segments_in_net(led).count() > 0);
    assert_eq!(
        run(&[
            "route-auto",
            p,
            "--net",
            "MISSING",
            "-o",
            out.to_str().unwrap()
        ]),
        1
    );
}
