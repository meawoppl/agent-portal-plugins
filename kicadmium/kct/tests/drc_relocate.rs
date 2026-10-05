//! Ports of upstream `tests/test_relocate_drill_clearance.py` and the repair
//! half of `tests/test_different_net_short.py`, plus `drc.fixer` coverage.

use std::collections::BTreeSet;

use kct::cli::relocate_in_pad_vias::{check_clearance, collect_smd_pads_by_net, collect_tht_pads};
use kct::drc::different_net_short::{find_different_net_shorts, repair_different_net_shorts};
use kct::drc::relocate_drill_clearance::{relocate_drill_clearance, try_relocate, violating_pairs};
use kct::manufacturers::DesignRules;
use kct::schema::pcb::*;

fn tier1() -> DesignRules {
    kct::manufacturers::rules("jlcpcb-tier1", 4, 1.0).unwrap()
}

fn via(pcb: &mut Pcb, x: f64, y: f64, size: f64, drill: f64, net: &str) {
    pcb.add_via(
        x,
        y,
        ViaOptions {
            size,
            drill,
            net: Some(net.into()),
            dedupe: false,
            ..Default::default()
        },
    )
    .unwrap();
}

fn trace(pcb: &mut Pcb, a: (f64, f64), b: (f64, f64), width: f64, layer: &str, net: &str) {
    pcb.add_trace(
        a,
        b,
        TraceOptions {
            width,
            layer: layer.into(),
            net: Some(net.into()),
            ..Default::default()
        },
    )
    .unwrap();
}

const SIZE: f64 = 0.3;
const DRILL: f64 = 0.15;

fn stack_board() -> Pcb {
    let mut pcb = Pcb::create(CreateOptions::size(40.0, 40.0)).unwrap();
    via(&mut pcb, 20.0, 20.0, SIZE, DRILL, "OSC_OUT");
    via(&mut pcb, 20.0, 20.5, SIZE, DRILL, "NRST");
    via(&mut pcb, 20.0, 21.0, SIZE, DRILL, "GND");
    trace(&mut pcb, (20.0, 20.5), (20.5, 20.5), 0.127, "B.Cu", "NRST");
    pcb
}

fn bonded(pcb: &Pcb, net: i64, old: (f64, f64), new: (f64, f64)) -> bool {
    let near = |p: (f64, f64), q: (f64, f64)| (p.0 - q.0).hypot(p.1 - q.1) < 1e-3;
    pcb.segments_in_net(net).any(|s| {
        (near(s.start, old) || near(s.end, old)) && (near(s.start, new) || near(s.end, new))
    })
}

#[test]
fn relocates_middle_via_of_pitch_stack() {
    let mut pcb = stack_board();
    let rules = tier1();
    assert_eq!(
        violating_pairs(pcb.vias(), rules.min_hole_to_hole_mm).len(),
        2
    );
    let result = relocate_drill_clearance(&mut pcb, &rules, None, false).unwrap();
    assert!(result.changed());
    assert_eq!(result.moved.len(), 1);
    assert_eq!(result.moved[0].net_name, "NRST");
    assert!(result.unresolved.is_empty());
    assert!(violating_pairs(pcb.vias(), rules.min_hole_to_hole_mm).is_empty());
    let m = &result.moved[0];
    for v in pcb.vias() {
        if v.uuid == m.uuid {
            continue;
        }
        let gap =
            (v.position.0 - m.new_x).hypot(v.position.1 - m.new_y) - DRILL / 2.0 - v.drill / 2.0;
        assert!(gap >= rules.min_hole_to_hole_mm - 1e-6);
    }
}

#[test]
fn relocated_via_stays_connected_via_stub() {
    let mut pcb = stack_board();
    let result = relocate_drill_clearance(&mut pcb, &tier1(), None, false).unwrap();
    let m = &result.moved[0];
    assert!(!m.stub_layers.is_empty());
    assert!(bonded(&pcb, m.net, (m.old_x, m.old_y), (m.new_x, m.new_y)));
}

#[test]
fn pass_reads_active_floor_not_hardcoded() {
    let mut narrow = tier1();
    narrow.min_hole_to_hole_mm = 0.30;
    let mut pcb = stack_board();
    assert!(!relocate_drill_clearance(&mut pcb, &narrow, None, false)
        .unwrap()
        .changed());
    let mut wide = tier1();
    wide.min_hole_to_hole_mm = 0.50;
    let mut pcb = stack_board();
    assert!(relocate_drill_clearance(&mut pcb, &wide, None, false)
        .unwrap()
        .changed());
}

#[test]
fn dry_run_reports_without_mutating_and_matches_actual() {
    let mut pcb = stack_board();
    let before = pcb.sexp().to_compact_string();
    let preview = relocate_drill_clearance(&mut pcb, &tier1(), None, true).unwrap();
    assert!(preview.changed());
    assert_eq!(pcb.sexp().to_compact_string(), before);
    let mut copy = pcb.clone();
    let actual = relocate_drill_clearance(&mut copy, &tier1(), None, false).unwrap();
    assert_eq!(preview, actual);
}

#[test]
fn net_scoping_restricts_moves() {
    let mut pcb = stack_board();
    let nets: BTreeSet<String> = ["GND".to_string(), "OSC_OUT".to_string()].into();
    let result = relocate_drill_clearance(&mut pcb, &tier1(), Some(&nets), false).unwrap();
    assert!(result.moved.iter().all(|m| nets.contains(&m.net_name)));
}

#[test]
fn boxed_in_target_is_reported() {
    let mut pcb = Pcb::create(CreateOptions::size(40.0, 40.0)).unwrap();
    via(&mut pcb, 20.0, 20.0, SIZE, DRILL, "TARGET");
    for (i, (dx, dy)) in kct::cli::relocate_in_pad_vias::PLANE_DIRECTIONS
        .iter()
        .enumerate()
    {
        via(
            &mut pcb,
            20.0 + dx * 0.85,
            20.0 + dy * 0.85,
            SIZE,
            DRILL,
            &format!("BLK{i}"),
        );
    }
    let rules = tier1();
    let pads = collect_smd_pads_by_net(&pcb);
    let tht = collect_tht_pads(&pcb);
    let r = try_relocate(
        &mut pcb,
        0,
        &pads,
        &tht,
        rules.min_clearance_mm,
        rules.min_hole_to_hole_mm,
        false,
    )
    .unwrap();
    assert!(r.is_none());
}

#[test]
fn dense_field_never_introduces_a_new_violation() {
    let mut pcb = Pcb::create(CreateOptions::size(40.0, 40.0)).unwrap();
    let mut n = 0;
    for gx in 0..5 {
        for gy in 0..5 {
            via(
                &mut pcb,
                18.0 + gx as f64 * 0.35,
                18.0 + gy as f64 * 0.35,
                SIZE,
                DRILL,
                &format!("N{n}"),
            );
            n += 1;
        }
    }
    let rules = tier1();
    let before = violating_pairs(pcb.vias(), rules.min_hole_to_hole_mm).len();
    assert!(before > 0);
    let result = relocate_drill_clearance(&mut pcb, &rules, None, false).unwrap();
    assert!(violating_pairs(pcb.vias(), rules.min_hole_to_hole_mm).len() <= before);
    assert!(!result.unresolved.is_empty());
    let pads = collect_smd_pads_by_net(&pcb);
    let tht = collect_tht_pads(&pcb);
    for m in &result.moved {
        let (i, v) = pcb
            .vias()
            .iter()
            .enumerate()
            .find(|(_, v)| v.uuid == m.uuid)
            .unwrap();
        let reason = check_clearance(
            &pcb,
            Some(i),
            v,
            v.position.0,
            v.position.1,
            &pads,
            &tht,
            rules.min_clearance_mm,
            rules.min_hole_to_hole_mm,
            None,
        )
        .unwrap();
        assert!(reason.is_none(), "{reason:?}");
    }
}

#[test]
fn drill_relocation_rejects_crossing_stub() {
    let mut pcb = Pcb::create(CreateOptions::size(30.0, 30.0)).unwrap();
    via(&mut pcb, 10.0, 10.0, 0.3, 0.15, "SIG");
    trace(&mut pcb, (10.0, 10.0), (12.0, 10.0), 0.127, "F.Cu", "SIG");
    trace(&mut pcb, (10.0, 10.0), (10.1, 10.0), 0.127, "B.Cu", "SIG");
    trace(&mut pcb, (11.0, 8.0), (11.0, 12.0), 0.127, "B.Cu", "OTHER");
    let r = try_relocate(
        &mut pcb,
        0,
        &Default::default(),
        &Vec::new(),
        0.127,
        0.5,
        false,
    )
    .unwrap();
    assert!(r.is_some());
    let sig = pcb.get_net_by_name("SIG").unwrap().number;
    for s in pcb.segments_in_net(sig).filter(|s| s.layer == "B.Cu") {
        let d = kct::core::geometry::segment_to_segment_distance(
            s.start.0, s.start.1, s.end.0, s.end.1, 11.0, 8.0, 11.0, 12.0,
        );
        assert!(d - 0.127 >= 0.127 - 1e-6, "B.Cu stub crosses OTHER");
    }
}

// --------------------------------------------------------- different-net shorts

const SSIZE: f64 = 0.6;
const SDRILL: f64 = 0.3;

fn board05_style_shorts() -> Pcb {
    let mut pcb = Pcb::create(CreateOptions::size(60.0, 60.0)).unwrap();
    via(&mut pcb, 20.0, 20.0, SSIZE, SDRILL, "NRST");
    via(&mut pcb, 20.3, 20.0, SSIZE, SDRILL, "OSC_IN");
    trace(&mut pcb, (20.0, 20.0), (17.0, 20.0), 0.2, "B.Cu", "NRST");
    trace(&mut pcb, (20.3, 20.0), (23.0, 20.0), 0.2, "B.Cu", "OSC_IN");
    trace(
        &mut pcb,
        (40.0, 40.0),
        (46.0, 40.0),
        0.2,
        "In2.Cu",
        "PWM_CH",
    );
    via(&mut pcb, 43.0, 40.0, SSIZE, SDRILL, "OSC_OUT");
    trace(&mut pcb, (43.0, 40.0), (43.0, 45.0), 0.2, "F.Cu", "OSC_OUT");
    pcb
}

#[test]
fn repair_eliminates_shorts_and_bonds() {
    let mut pcb = board05_style_shorts();
    assert!(!find_different_net_shorts(&pcb, 0.0).is_empty());
    let result = repair_different_net_shorts(&mut pcb, &tier1(), 0.0, false).unwrap();
    assert!(result.changed());
    assert!(result.unresolved.is_empty(), "{}", result.summary());
    assert!(find_different_net_shorts(&pcb, 0.0).is_empty());
    for m in &result.moved {
        let net = pcb.get_net_by_name(&m.net_name).unwrap().number;
        assert!(bonded(&pcb, net, (m.old_x, m.old_y), (m.new_x, m.new_y)));
    }
}

#[test]
fn repair_dry_run_does_not_mutate() {
    let mut pcb = board05_style_shorts();
    let before = pcb.sexp().to_compact_string();
    let result = repair_different_net_shorts(&mut pcb, &tier1(), 0.0, true).unwrap();
    assert!(!result.moved.is_empty());
    assert_eq!(pcb.sexp().to_compact_string(), before);
    assert!(!find_different_net_shorts(&pcb, 0.0).is_empty());
}

#[test]
fn boxed_in_short_left_in_place_and_reported() {
    let mut pcb = Pcb::create(CreateOptions::size(6.0, 6.0)).unwrap();
    via(&mut pcb, 3.0, 3.0, SSIZE, SDRILL, "VICTIM");
    for k in 0..16 {
        let ang = 2.0 * std::f64::consts::PI * k as f64 / 16.0;
        via(
            &mut pcb,
            3.0 + 0.62 * ang.cos(),
            3.0 + 0.62 * ang.sin(),
            SSIZE,
            SDRILL,
            &format!("BLOCK{k}"),
        );
    }
    assert!(!find_different_net_shorts(&pcb, 0.0).is_empty());
    let result = repair_different_net_shorts(&mut pcb, &tier1(), 0.0, false).unwrap();
    assert!(!result.unresolved.is_empty());
    assert!(!find_different_net_shorts(&pcb, 0.0).is_empty());
}

// ------------------------------------------------------------------ drc.fixer

#[test]
fn fixer_finds_and_deletes_near_geometry() {
    use kct::drc::fixer::DRCFixer;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("b.kicad_pcb");
    let mut pcb = board05_style_shorts();
    pcb.save(Some(&path)).unwrap();
    let mut fixer = DRCFixer::new(&path).unwrap();
    assert!(fixer.net_names.contains_key("NRST"));
    // Board-relative 20,20 -> absolute via the board origin used on save.
    let v = &fixer.find_vias_near(0.0, 0.0, 1e9, Some("NRST"))[0];
    let (vx, vy) = (v.x, v.y);
    let segs = fixer.find_segments_near(vx, vy, 0.35, Some("B.Cu"), None);
    assert_eq!(segs.len(), 2);
    assert!(fixer.delete_segment(&segs[0]));
    assert_eq!(fixer.deleted_count, 1);
    let n = fixer.delete_net_traces("OSC_OUT");
    assert_eq!(n, 2);
    assert_eq!(fixer.summary(), "DRC Fixer: deleted 3 elements");
    let out = dir.path().join("out.kicad_pcb");
    fixer.save(Some(&out)).unwrap();
    let re = Pcb::load(&out).unwrap();
    assert_eq!(re.segments().len(), pcb.segments().len() - 2);
    assert_eq!(re.vias().len(), pcb.vias().len() - 1);
}
