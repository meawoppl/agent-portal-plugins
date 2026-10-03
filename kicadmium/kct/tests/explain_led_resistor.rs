//! LedSeriesResistorCheck on a shared series resistor.
//!
//! Fixture: bp-test `direction-led-tester`, where R1 (1k) feeds an
//! anti-parallel pair D1/D2 through /LED_DRV (R1.2, D1.2, D2.1). The
//! junction has three terminals, which the upstream heuristic rejected,
//! reporting both LEDs as missing a series resistor.

use std::path::{Path, PathBuf};

use kct::explain::checks::LedSeriesResistorCheck;
use kct::explain::mistakes::MistakeCheck;
use kct::schema::pcb::Pcb;

fn board() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/validate_sync/direction-led-tester.kicad_pcb")
}

fn flagged(pcb: &Path) -> Vec<String> {
    let pcb = Pcb::load(pcb).unwrap();
    let mut refs: Vec<String> = LedSeriesResistorCheck
        .check(&pcb)
        .unwrap()
        .into_iter()
        .flat_map(|m| m.components)
        .collect();
    refs.sort();
    refs
}

/// Copy the board with one textual replacement applied.
fn mutated(dir: &Path, needle: &str, with: &str) -> PathBuf {
    let text = std::fs::read_to_string(board()).unwrap();
    assert_eq!(text.matches(needle).count(), 1, "{needle}");
    let out = dir.join("board.kicad_pcb");
    std::fs::write(&out, text.replace(needle, with)).unwrap();
    out
}

#[test]
fn shared_series_resistor_covers_antiparallel_leds() {
    assert!(flagged(&board()).is_empty());
}

#[test]
fn leds_without_a_resistor_are_still_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let pcb = mutated(
        tmp.path(),
        "(property \"Reference\" \"R1\"",
        "(property \"Reference\" \"U9\"",
    );
    assert_eq!(flagged(&pcb), vec!["D1", "D2"]);
}

#[test]
fn junction_shared_with_other_parts_is_not_a_series_resistor() {
    let tmp = tempfile::tempdir().unwrap();
    // J1 pad 2 moved onto the resistor/LED junction.
    let pcb = mutated(
        tmp.path(),
        "(net \"/SIG-\")\n\t\t\t(pinfunction \"2_2\")\n\t\t\t(pintype \"passive\")\n\t\t\t(uuid \"c2c3330b",
        "(net \"/LED_DRV\")\n\t\t\t(pinfunction \"2_2\")\n\t\t\t(pintype \"passive\")\n\t\t\t(uuid \"c2c3330b",
    );
    assert_eq!(flagged(&pcb), vec!["D1", "D2"]);
}
