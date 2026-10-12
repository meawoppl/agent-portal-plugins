use kct::{region::Bounds, regional_check::run};
fn board(x: f64) -> String {
    format!(
        r#"(kicad_pcb (layers (0 "F.Cu" signal) (31 "B.Cu" signal)) (net 1 "N") (segment (start 0 0) (end 0.05 0) (width 0.1) (layer "F.Cu") (net 1 "N") (uuid "near")) (segment (start {x} 100) (end 110 100) (width 0.3) (layer "F.Cu") (net 1 "N") (uuid "far")))"#
    )
}
#[test]
fn region_cache_reuses_unchanged_geometry_and_invalidates_inputs() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("board.kicad_pcb");
    std::fs::write(&path, board(100.)).unwrap();
    let b = Bounds {
        x0: -1.,
        y0: -1.,
        x1: 1.,
        y1: 1.,
    };
    let a = run(&path, b, 2., "lint", Default::default()).unwrap();
    assert_eq!(a["cache_hit"], false);
    assert_eq!(a["full_board_gate"], false);
    assert_eq!(
        run(&path, b, 2., "lint", Default::default()).unwrap()["cache_hit"],
        true
    );
    std::fs::write(&path, board(101.)).unwrap();
    let changed = run(&path, b, 2., "lint", Default::default()).unwrap();
    assert_eq!(changed["cache_hit"], true);
    assert_ne!(changed["board_sha256"], a["board_sha256"]);
    std::fs::write(path.with_extension("kicad_pro"), "{}").unwrap();
    assert_eq!(
        run(&path, b, 2., "lint", Default::default()).unwrap()["cache_hit"],
        false
    );
    let before = std::fs::read(&path).unwrap();
    let cfg = kct::lint::Config {
        min_trace_mm: 0.4,
        ..Default::default()
    };
    assert_eq!(run(&path, b, 2., "lint", cfg).unwrap()["cache_hit"], false);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
#[test]
fn local_check_is_bounded_on_large_sparse_board() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("large.kicad_pcb");
    let mut source = board(100.);
    source.pop();
    for i in 0..3000 {
        source += &format!(
            r#"(segment (start {} 100) (end {} 101) (width 0.3) (layer "F.Cu") (net 1 "N") (uuid "far{i}"))"#,
            200 + i,
            200 + i
        );
    }
    source.push(')');
    std::fs::write(&path, source).unwrap();
    let start = std::time::Instant::now();
    let report = run(
        &path,
        Bounds {
            x0: -1.,
            y0: -1.,
            x1: 1.,
            y1: 1.,
        },
        2.,
        "lint",
        Default::default(),
    )
    .unwrap();
    assert!(report["findings"].as_array().unwrap().len() < 20);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "local check exceeded five seconds on 3000 outside segments"
    );
}
