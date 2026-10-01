//! Ports of upstream tests/test_net_class_map_sidecar_parity.py (shared name
//! contract) and tests/test_project.py (path resolution, create, cross
//! reference).

use std::path::PathBuf;

use kct::project::{
    CrossReferenceResult, MismatchedComponent, OrphanedFootprint, Project, RoutingResult,
    UnplacedSymbol,
};
use kct::sidecars::{
    first_existing_net_class_map_sidecar, net_class_map_sidecar_candidates,
    net_class_map_sidecar_names, NET_CLASS_MAP_SIDECAR_BASENAME,
};

// ------------------------------------------------------------------ sidecars

#[test]
fn shared_name_contract() {
    assert_eq!(
        net_class_map_sidecar_names("board_v24"),
        ["board_v24.net_class_map.json", "net_class_map.json"]
    );
    assert_eq!(NET_CLASS_MAP_SIDECAR_BASENAME, "net_class_map.json");
    assert_eq!(net_class_map_sidecar_names(""), ["net_class_map.json"]);
}

#[test]
fn check_cmd_candidate_order_and_dedup() {
    let tmp = tempfile::tempdir().unwrap();
    let pcb_dir = tmp.path().join("boards/demo");
    std::fs::create_dir_all(&pcb_dir).unwrap();
    let pcb = pcb_dir.join("demo_routed.kicad_pcb");
    assert_eq!(
        net_class_map_sidecar_candidates(&pcb),
        vec![
            pcb_dir.join("demo_routed.net_class_map.json"),
            pcb_dir.join("net_class_map.json"),
            pcb_dir.join("output/demo_routed.net_class_map.json"),
            pcb_dir.join("output/net_class_map.json"),
            tmp.path()
                .join("boards/output/demo_routed.net_class_map.json"),
            tmp.path().join("boards/output/net_class_map.json"),
        ]
    );
    // boards/NN/output/<pcb>: dir 1 and dir 3 collapse.
    let out = tmp.path().join("boards/05/output");
    let pcb = out.join("demo_routed.kicad_pcb");
    assert_eq!(
        net_class_map_sidecar_candidates(&pcb),
        vec![
            out.join("demo_routed.net_class_map.json"),
            out.join("net_class_map.json"),
            out.join("output/demo_routed.net_class_map.json"),
            out.join("output/net_class_map.json"),
        ]
    );
}

#[test]
fn first_existing_helper() {
    let tmp = tempfile::tempdir().unwrap();
    let (near, far) = (tmp.path().join("near"), tmp.path().join("far"));
    std::fs::create_dir(&near).unwrap();
    std::fs::create_dir(&far).unwrap();
    std::fs::write(far.join(NET_CLASS_MAP_SIDECAR_BASENAME), "{}").unwrap();
    assert_eq!(first_existing_net_class_map_sidecar([&near], "demo"), None);
    assert_eq!(
        first_existing_net_class_map_sidecar([&near, &far], "demo"),
        Some(far.join(NET_CLASS_MAP_SIDECAR_BASENAME))
    );
    std::fs::write(near.join(NET_CLASS_MAP_SIDECAR_BASENAME), "{}").unwrap();
    let stem_keyed = near.join("demo.net_class_map.json");
    std::fs::write(&stem_keyed, "{}").unwrap();
    assert_eq!(
        first_existing_net_class_map_sidecar([&near], "demo"),
        Some(stem_keyed)
    );
    // Exact stem: no un-suffixing.
    assert_eq!(
        first_existing_net_class_map_sidecar([&near], "demo_routed"),
        Some(near.join(NET_CLASS_MAP_SIDECAR_BASENAME))
    );
}

// ------------------------------------------------------------------ project data types

#[test]
fn result_types() {
    let r = CrossReferenceResult::default();
    assert_eq!(r.matched, 0);
    assert!(r.is_clean());
    let r = CrossReferenceResult {
        matched: 10,
        ..Default::default()
    };
    assert!(r.is_clean());
    let u = UnplacedSymbol {
        reference: "U1".into(),
        value: "v".into(),
        lib_id: "l".into(),
        footprint_name: "f".into(),
    };
    let o = |r: &str, x: f64| OrphanedFootprint {
        reference: r.into(),
        value: "10k".into(),
        footprint_name: "fp".into(),
        position: (x, 0.0),
    };
    assert!(!CrossReferenceResult {
        unplaced: vec![u.clone()],
        ..Default::default()
    }
    .is_clean());
    assert!(!CrossReferenceResult {
        orphaned: vec![o("R1", 0.0)],
        ..Default::default()
    }
    .is_clean());
    let m = MismatchedComponent {
        reference: "R1".into(),
        schematic_value: "10k".into(),
        pcb_value: "4.7k".into(),
        schematic_footprint: "fp1".into(),
        pcb_footprint: "fp2".into(),
        ..Default::default()
    };
    assert!(m.mismatches.is_empty());
    assert!(!CrossReferenceResult {
        mismatched: vec![m],
        ..Default::default()
    }
    .is_clean());
    let s = CrossReferenceResult {
        matched: 5,
        unplaced: vec![u],
        orphaned: vec![o("R1", 0.0), o("R2", 10.0)],
        mismatched: vec![],
    }
    .summary();
    assert_eq!(
        s,
        serde_json::json!({"matched": 5, "unplaced": 1, "orphaned": 2, "mismatched": 0})
    );
    let rr = RoutingResult {
        routed_nets: 3,
        total_nets: 4,
        total_segments: 0,
        total_vias: 0,
        total_length_mm: 0.0,
    };
    assert_eq!(rr.success_rate(), 0.75);
}

// ------------------------------------------------------------------ Project

fn none() -> Option<PathBuf> {
    None
}

#[test]
fn project_naming_and_directory() {
    let p = Project::default();
    assert!(p.project_file.is_none() && p.schematic_path.is_none() && p.pcb_path.is_none());
    assert_eq!(p.name(), "unnamed");
    assert!(p.directory().is_none());
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    assert_eq!(
        Project::new(none(), none(), Some(t.join("my_project.kicad_pro"))).name(),
        "my_project"
    );
    assert_eq!(
        Project::new(none(), Some(t.join("board.kicad_pcb")), none()).name(),
        "board"
    );
    assert_eq!(
        Project::new(Some(t.join("circuit.kicad_sch")), none(), none()).name(),
        "circuit"
    );
    assert_eq!(
        Project::new(none(), none(), Some(t.join("subdir/test.kicad_pro"))).directory(),
        Some(t.join("subdir"))
    );
    assert_eq!(
        Project::new(none(), Some(t.join("x.kicad_pcb")), none()).directory(),
        Some(t.to_path_buf())
    );
    let r = Project::new(
        Some(t.join("test.kicad_sch")),
        Some(t.join("test.kicad_pcb")),
        none(),
    )
    .repr();
    assert!(r.contains("Project") && r.contains("test.kicad_sch") && r.contains("test.kicad_pcb"));
}

fn touch(dir: &std::path::Path, names: &[(&str, &str)]) {
    for (n, c) in names {
        std::fs::write(dir.join(n), c).unwrap();
    }
}

#[test]
fn project_load_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    touch(
        t,
        &[
            ("test.kicad_pro", "{}"),
            ("test.kicad_sch", ""),
            ("test.kicad_pcb", ""),
        ],
    );
    let p = Project::load(t.join("test.kicad_pro")).unwrap();
    assert_eq!(p.project_file, Some(t.join("test.kicad_pro")));
    assert_eq!(p.schematic_path, Some(t.join("test.kicad_sch")));
    assert_eq!(p.pcb_path, Some(t.join("test.kicad_pcb")));

    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    touch(t, &[("test.kicad_pro", "{}"), ("test.kicad_pcb", "")]);
    let p = Project::load(t.join("test.kicad_pro")).unwrap();
    assert!(p.schematic_path.is_none());

    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    touch(t, &[("test.kicad_pro", "{}"), ("test.kicad_sch", "")]);
    let p = Project::load(t.join("test.kicad_pro")).unwrap();
    assert!(p.pcb_path.is_none());
    assert!(p.warnings.iter().any(|w| w.contains("No .kicad_pcb")));

    let e = Project::load(t.join("nonexistent.kicad_pro")).unwrap_err();
    assert_eq!(
        e.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn project_load_siblings_and_boards() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    touch(
        t,
        &[
            ("foo.kicad_pro", "{}"),
            ("foo.kicad_sch", ""),
            ("foo_v2.kicad_pcb", ""),
        ],
    );
    let p = Project::load(t.join("foo.kicad_pro")).unwrap();
    assert_eq!(p.pcb_path, Some(t.join("foo_v2.kicad_pcb")));
    assert!(p.warnings.iter().any(|w| w.contains("foo_v2.kicad_pcb")));

    touch(t, &[("foo_v1.kicad_pcb", "")]);
    let msg = Project::load(t.join("foo.kicad_pro"))
        .unwrap_err()
        .to_string();
    for s in [
        "foo_v1.kicad_pcb",
        "foo_v2.kicad_pcb",
        "foo.kicad_pro",
        "--pcb",
    ] {
        assert!(msg.contains(s), "{msg}");
    }

    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    touch(
        t,
        &[
            (
                "foo.kicad_pro",
                r#"{"boards": [{"file": "explicit.kicad_pcb"}]}"#,
            ),
            ("foo.kicad_sch", ""),
            ("foo.kicad_pcb", ""),
            ("explicit.kicad_pcb", ""),
        ],
    );
    let p = Project::load(t.join("foo.kicad_pro")).unwrap();
    assert_eq!(
        p.pcb_path,
        Some(std::fs::canonicalize(t.join("explicit.kicad_pcb")).unwrap())
    );

    for pro in [
        r#"{"boards": []}"#,
        r#"{"boards": [{"file": "ghost.kicad_pcb"}]}"#,
        "{not valid json",
        "",
        r#"{"meta": {"filename": "foo.kicad_pro"}}"#,
    ] {
        touch(t, &[("foo.kicad_pro", pro)]);
        std::fs::remove_file(t.join("explicit.kicad_pcb")).ok();
        let p = Project::load(t.join("foo.kicad_pro")).unwrap();
        assert_eq!(p.pcb_path, Some(t.join("foo.kicad_pcb")), "{pro}");
        assert_eq!(p.schematic_path, Some(t.join("foo.kicad_sch")));
        if pro.starts_with("{not") {
            assert!(p.warnings.iter().any(|w| w.contains("Could not parse")));
        }
    }
}

#[test]
fn project_from_pcb() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    touch(
        t,
        &[
            ("board.kicad_pcb", ""),
            ("board.kicad_sch", ""),
            ("board.kicad_pro", "{}"),
        ],
    );
    let p = Project::from_pcb(t.join("board.kicad_pcb"));
    assert_eq!(p.schematic_path, Some(t.join("board.kicad_sch")));
    assert_eq!(p.project_file, Some(t.join("board.kicad_pro")));

    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), &[("board.kicad_pcb", "")]);
    let p = Project::from_pcb(tmp.path().join("board.kicad_pcb"));
    assert!(p.schematic_path.is_none() && p.project_file.is_none());
}

const SCH: &str = r##"(kicad_sch (version 20231120) (generator "test")
  (lib_symbols)
  (symbol (lib_id "Device:R") (at 100 100 0)
    (property "Reference" "R1" (at 100 90 0))
    (property "Value" "10k" (at 100 110 0))
    (property "Footprint" "Resistor_SMD:R_0402_1005Metric" (at 100 100 0)))
  (symbol (lib_id "Device:R") (at 120 100 0)
    (property "Reference" "R2" (at 100 90 0))
    (property "Value" "1k" (at 100 110 0))
    (property "Footprint" "" (at 100 100 0)))
  (symbol (lib_id "power:GND") (at 100 100 0)
    (property "Reference" "#PWR01" (at 100 90 0))
    (property "Value" "GND" (at 100 110 0)))
)"##;

const PCB: &str = r#"(kicad_pcb (version 20240108) (generator "test")
  (net 0 "")
  (footprint "Resistor_SMD:R_0402_1005Metric" (layer "F.Cu") (at 100 100)
    (property "Reference" "R1" (at 0 0 0) (layer "F.SilkS"))
    (property "Value" "4.7k" (at 0 0 0) (layer "F.Fab")))
  (footprint "Resistor_SMD:R_0603_1608Metric" (layer "F.Cu") (at 150 100)
    (property "Reference" "R99" (at 0 0 0) (layer "F.SilkS"))
    (property "Value" "10k" (at 0 0 0) (layer "F.Fab")))
)"#;

#[test]
fn cross_reference_detects_mismatch_unplaced_orphaned() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    touch(t, &[("test.kicad_sch", SCH), ("test.kicad_pcb", PCB)]);
    let mut p = Project::new(
        Some(t.join("test.kicad_sch")),
        Some(t.join("test.kicad_pcb")),
        none(),
    );
    let r = p.cross_reference().unwrap();
    assert_eq!(r.matched, 1);
    assert_eq!(r.mismatched.len(), 1);
    assert_eq!(r.mismatched[0].reference, "R1");
    assert_eq!(r.mismatched[0].mismatches, ["value"]);
    assert_eq!(r.unplaced.len(), 1);
    assert_eq!(r.unplaced[0].reference, "R2");
    assert_eq!(r.orphaned.len(), 1);
    assert_eq!(r.orphaned[0].reference, "R99");
    assert_eq!(r.orphaned[0].position, (150.0, 100.0));
    assert_eq!(p.find_unplaced_symbols().unwrap().len(), 1);
    assert_eq!(p.find_orphaned_footprints().unwrap().len(), 1);

    let mut missing = Project::new(none(), Some(t.join("test.kicad_pcb")), none());
    assert!(missing.cross_reference().unwrap().is_clean());
}

#[test]
fn project_create() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let mut p = Project::create("my_board", t, 100.0, 80.0).unwrap();
    for ext in ["kicad_pro", "kicad_sch", "kicad_pcb"] {
        assert!(t.join(format!("my_board.{ext}")).exists());
    }
    assert_eq!(p.name(), "my_board");
    assert_eq!(p.project_file, Some(t.join("my_board.kicad_pro")));
    assert!(p.schematic().unwrap().unwrap().has_tag("kicad_sch"));
    assert!(p.pcb().unwrap().unwrap().has_tag("kicad_pcb"));

    Project::create("custom_board", t, 50.0, 30.0).unwrap();
    let text = std::fs::read_to_string(t.join("custom_board.kicad_pcb")).unwrap();
    assert!(text.contains("(end 50.0 30.0)"));
    let root = kct::parse(&text).unwrap();
    assert_eq!(
        kct::core::board_outline::board_outline_bounds(&root).unwrap(),
        Some((0.0, 0.0, 50.0, 30.0))
    );

    let nested = t.join("new/nested/dir");
    Project::create("test", &nested, 100.0, 80.0).unwrap();
    assert!(nested.join("test.kicad_pro").exists());

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(t.join("my_board.kicad_pro")).unwrap())
            .unwrap();
    assert!(data.get("meta").is_some() && data.get("project").is_some());
}
