use kct::schema::pcb::Pcb;
use kct::validate::rules::silkscreen::{check_silk_overlap, iter_silk_geometries};

#[test]
fn board_silk_and_footprint_silk_share_coordinates_after_reload() {
    for (ox, oy) in [(0.0, 0.0), (22.0, 28.0), (-10.0, -20.0)] {
        let mut pcb = Pcb::parse_str(&format!(r#"(kicad_pcb
          (gr_rect (start {ox} {oy}) (end {} {}) (stroke (width 0.05) (type default)) (layer "Edge.Cuts"))
          (footprint "test" (layer "F.Cu") (at {} {})
            (fp_line (start 0 0) (end 4 0) (stroke (width 0.2) (type default)) (layer "F.SilkS")))
          (gr_line (start {} {}) (end {} {}) (stroke (width 0.2) (type default)) (layer "F.SilkS") (uuid "line"))
          (gr_text "LABEL" (at {} {}) (layer "F.SilkS") (uuid "text") (effects (font (size 1 1) (thickness 0.15)))))"#,
          ox + 100.0, oy + 100.0, ox + 10.0, oy + 10.0,
          ox + 10.0, oy + 10.0, ox + 14.0, oy + 10.0,
          ox + 50.0, oy + 50.0)).unwrap();
        for _ in 0..2 {
            let items = iter_silk_geometries(&pcb);
            let line = items.iter().find(|i| i.uuid == "line").unwrap();
            let text = items.iter().find(|i| i.uuid == "text").unwrap();
            assert_eq!(line.location, (10.0, 10.0));
            assert_eq!(text.location, (50.0, 50.0));
            assert_eq!(line.geom.bounds(), items[0].geom.bounds());
            assert_eq!(check_silk_overlap(&pcb).violations.len(), 1);
            pcb.reload_from_tree().unwrap();
        }
    }
}
