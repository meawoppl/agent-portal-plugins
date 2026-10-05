//! Saved-golden checks for the wave D repair commands (optimize-traces,
//! fix-vias incl. `--relocate-in-pad`, fix-silkscreen, repair-clearance,
//! fix-drc) against upstream kicad-tools.
//!
//! Goldens in `fixtures/wave_d/` were captured once from the upstream Python
//! CLI: `<case>.stdout` (work dir normalised to `<WORK>`), `<case>.code`, and
//! the written board as `<case>.out.kicad_pcb` (or `<case>.out.sha256` for
//! large boards). DRC inputs were captured with `kicad-cli pcb drc --format
//! json`. Each case runs the native command in a child process (so stdout
//! can be captured) on a copy of the input under the same file names.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const CHILD_ENV: &str = "KCT_WAVE_D_CHILD_ARGS";
const BEGIN: &str = "<<<KCT-STDOUT-BEGIN>>>";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn gold(name: &str) -> PathBuf {
    fixtures().join("wave_d").join(name)
}

/// Child entry point: runs `kct::cli::run` with the JSON-encoded args.
#[test]
fn kct_child_entry() {
    let Ok(raw) = std::env::var(CHILD_ENV) else {
        return;
    };
    let args: Vec<String> = serde_json::from_str(&raw).unwrap();
    use std::io::Write;
    print!("{BEGIN}");
    std::io::stdout().flush().unwrap();
    let code = kct::cli::run(args).unwrap_or_else(|e| {
        eprintln!("Error: {e:#}");
        1
    });
    std::io::stdout().flush().unwrap();
    std::process::exit(code);
}

fn run_child(args: &[String]) -> (i32, String) {
    let exe = std::env::current_exe().unwrap();
    let out = Command::new(exe)
        .args([
            "kct_child_entry",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, serde_json::to_string(args).unwrap())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let body = stdout
        .split_once(BEGIN)
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| {
            panic!(
                "child produced no marker: {stdout}\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
    (out.status.code().unwrap_or(-1), body)
}

fn sha(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn mask_uuids(text: &str) -> String {
    let re = regex::Regex::new(r#"\(uuid "?[0-9a-fA-F-]{36}"?\)"#).unwrap();
    re.replace_all(text, "(uuid <UUID>)").into_owned()
}

fn first_line_diff(got: &str, want: &str) -> String {
    for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
        if g != w {
            return format!("line {}:\n  got:  {g}\n  want: {w}", i + 1);
        }
    }
    format!(
        "length differs: got {} lines, want {}",
        got.lines().count(),
        want.lines().count()
    )
}

/// One golden case: `board` is copied to `<WORK>/<case>.kicad_pcb` (plus a
/// sibling `.kicad_pro` if present); `args` may use `{IN}`, `{OUT}`, `{WORK}`.
fn case(case: &str, board: &str, args: &[&str], board_golden: Option<&str>) {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path();
    let src = fixtures().join(board);
    let input = work.join(format!("{case}.kicad_pcb"));
    std::fs::copy(&src, &input).unwrap();
    let pro = src.with_extension("kicad_pro");
    if pro.exists() {
        std::fs::copy(&pro, work.join(format!("{case}.kicad_pro"))).unwrap();
    }
    for drc in ["charlie-drc.json", "matchgroup-drc.json"] {
        std::fs::copy(gold(drc), work.join(drc)).unwrap();
    }
    let output = work.join(format!("{case}.out.kicad_pcb"));
    let w = work.display().to_string();
    let argv: Vec<String> = args
        .iter()
        .map(|a| {
            a.replace("{IN}", &input.display().to_string())
                .replace("{OUT}", &output.display().to_string())
                .replace("{WORK}", &w)
        })
        .collect();
    let (code, stdout) = run_child(&argv);
    // Debug aid: KCT_WAVE_D_DUMP=<dir> keeps the native outputs.
    if let Ok(dump) = std::env::var("KCT_WAVE_D_DUMP") {
        let dump = Path::new(&dump);
        std::fs::create_dir_all(dump).unwrap();
        std::fs::write(dump.join(format!("{case}.stdout")), &stdout).unwrap();
        if output.exists() {
            std::fs::copy(&output, dump.join(format!("{case}.out.kicad_pcb"))).unwrap();
        }
    }
    // kicadmium's optimize-traces adds an explicit `--in-place` write
    // authorization (design-edit policy); its JSON echo is the one
    // intentional key difference.
    let stdout: String = stdout
        .replace(&w, "<WORK>")
        .split_inclusive('\n')
        .filter(|l| !l.starts_with("  \"in_place\": "))
        .collect();

    let want_code: i32 = std::fs::read_to_string(gold(&format!("{case}.code")))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let want_stdout = std::fs::read_to_string(gold(&format!("{case}.stdout"))).unwrap();
    let mut problems = Vec::new();
    if code != want_code {
        problems.push(format!("exit {code} != {want_code}"));
    }
    if stdout != want_stdout {
        problems.push(format!(
            "stdout differs: {}",
            first_line_diff(&stdout, &want_stdout)
        ));
    }
    let gname = board_golden.unwrap_or(case);
    let want_board = gold(&format!("{gname}.out.kicad_pcb"));
    let want_sha = gold(&format!("{gname}.out.sha256"));
    if want_board.exists() {
        match std::fs::read_to_string(&output) {
            Ok(got) => {
                let want = std::fs::read_to_string(&want_board).unwrap();
                // Newly created copper gets fresh random UUIDs on both sides.
                let (got, want) = (mask_uuids(&got), mask_uuids(&want));
                if got != want {
                    problems.push(format!("board differs: {}", first_line_diff(&got, &want)));
                }
            }
            Err(_) => problems.push("board not written".into()),
        }
    } else if want_sha.exists() {
        if !output.exists() {
            problems.push("board not written".into());
        } else {
            let want = std::fs::read_to_string(&want_sha).unwrap();
            if sha(&output) != want.trim() {
                problems.push("board sha256 differs".into());
            }
        }
    } else if output.exists() {
        problems.push("board written but upstream wrote none".into());
    }
    let want_svg = gold(&format!("{case}.svg"));
    if want_svg.exists() {
        match std::fs::read_to_string(work.join(format!("{case}.svg"))) {
            Ok(got) => {
                let want = std::fs::read_to_string(&want_svg).unwrap();
                if got != want {
                    problems.push(format!("svg differs: {}", first_line_diff(&got, &want)));
                }
            }
            Err(_) => problems.push("svg not written".into()),
        }
    }
    assert!(problems.is_empty(), "{case}: {}", problems.join("\n"));
}

#[test]
fn place_silk_refs_json_noop() {
    case(
        "psr_charlie",
        CHARLIE,
        &["place-silk-refs", "{IN}", "-o", "{OUT}", "--format", "json"],
        None,
    );
}

#[test]
fn place_silk_refs_text_render() {
    case(
        "psr_charlie_text",
        CHARLIE,
        &[
            "place-silk-refs",
            "{IN}",
            "-o",
            "{OUT}",
            "--allow-rotate",
            "--render",
            "{WORK}/psr_charlie_text.svg",
        ],
        Some("psr_charlie"),
    );
}

#[test]
fn place_silk_refs_moves_json() {
    case(
        "psr_matchgroup",
        MATCHGROUP,
        &[
            "place-silk-refs",
            "{IN}",
            "-o",
            "{OUT}",
            "--format",
            "json",
            "--mfr",
            "jlcpcb",
            "--clearance",
            "0.2",
        ],
        None,
    );
}

#[test]
fn place_silk_refs_moves_text_render() {
    case(
        "psr_matchgroup_text",
        MATCHGROUP,
        &[
            "place-silk-refs",
            "{IN}",
            "-o",
            "{OUT}",
            "--allow-rotate",
            "--clearance",
            "0.25",
            "--render",
            "{WORK}/psr_matchgroup_text.svg",
        ],
        None,
    );
}

#[test]
fn place_silk_refs_unplaceable_dry_run() {
    case(
        "psr_matchgroup_stuck",
        MATCHGROUP,
        &[
            "place-silk-refs",
            "{IN}",
            "--dry-run",
            "--max-offset",
            "0",
            "--format",
            "json",
        ],
        None,
    );
}

#[test]
fn place_silk_refs_summary_and_dry_text() {
    case(
        "psr_pin1",
        "pin1_marker.kicad_pcb",
        &[
            "place-silk-refs",
            "{IN}",
            "-o",
            "{OUT}",
            "--format",
            "summary",
        ],
        None,
    );
    case(
        "psr_charlie_dry",
        CHARLIE,
        &[
            "place-silk-refs",
            "{IN}",
            "--dry-run",
            "--max-offset",
            "2",
            "--step",
            "0.5",
        ],
        None,
    );
}

const CHARLIE: &str =
    "historical_demo_boards/02-charlieplex-led/output/charlieplex_3x3_routed.kicad_pcb";
const MATCHGROUP: &str =
    "historical_demo_boards/07-matchgroup-test/output/matchgroup_test_routed.kicad_pcb";

#[test]
fn optimize_traces_json() {
    case(
        "ot_charlie",
        CHARLIE,
        &[
            "optimize-traces",
            "{IN}",
            "-o",
            "{OUT}",
            "--format",
            "json",
            "-q",
        ],
        None,
    );
}

#[test]
fn optimize_traces_text() {
    case(
        "ot_charlie_text",
        CHARLIE,
        &["optimize-traces", "{IN}", "-o", "{OUT}"],
        Some("ot_charlie"),
    );
}

#[test]
fn fix_vias_json() {
    case(
        "fv_charlie",
        CHARLIE,
        &[
            "fix-vias",
            "{IN}",
            "--mfr",
            "jlcpcb",
            "--drill",
            "0.35",
            "--diameter",
            "0.7",
            "-o",
            "{OUT}",
            "--format",
            "json",
        ],
        None,
    );
}

#[test]
fn fix_vias_text() {
    case(
        "fv_charlie_text",
        CHARLIE,
        &[
            "fix-vias",
            "{IN}",
            "--mfr",
            "jlcpcb",
            "--drill",
            "0.35",
            "--diameter",
            "0.7",
            "-o",
            "{OUT}",
        ],
        Some("fv_charlie"),
    );
}

#[test]
fn fix_vias_relocate_in_pad_blocked() {
    case(
        "fv_relocate",
        "via_relocation/blocked_slide.kicad_pcb",
        &[
            "fix-vias",
            "{IN}",
            "--mfr",
            "jlcpcb",
            "--relocate-in-pad",
            "-o",
            "{OUT}",
            "--format",
            "json",
        ],
        None,
    );
}

#[test]
fn fix_vias_relocate_in_pad_hole_floor() {
    case(
        "fv_relocate_hf",
        "via_relocation/hole_floor.kicad_pcb",
        &[
            "fix-vias",
            "{IN}",
            "--mfr",
            "jlcpcb",
            "--relocate-in-pad",
            "-o",
            "{OUT}",
            "--format",
            "json",
        ],
        None,
    );
}

#[test]
fn fix_silkscreen_json_noop() {
    case(
        "fs_charlie",
        CHARLIE,
        &[
            "fix-silkscreen",
            "{IN}",
            "--mfr",
            "jlcpcb",
            "--min-width",
            "0.2",
            "-o",
            "{OUT}",
            "--format",
            "json",
        ],
        None,
    );
}

#[test]
fn fix_silkscreen_text_scales_refs() {
    case(
        "fs_matchgroup",
        MATCHGROUP,
        &[
            "fix-silkscreen",
            "{IN}",
            "--mfr",
            "jlcpcb",
            "--min-width",
            "0.2",
            "-o",
            "{OUT}",
        ],
        None,
    );
}

#[test]
fn repair_clearance_nothing_to_repair() {
    case(
        "rc_charlie",
        CHARLIE,
        &[
            "repair-clearance",
            "{IN}",
            "--drc-report",
            "{WORK}/charlie-drc.json",
            "--mfr",
            "jlcpcb",
            "-o",
            "{OUT}",
            "--format",
            "json",
        ],
        None,
    );
}

#[test]
fn repair_clearance_nudges() {
    case(
        "rc_matchgroup",
        MATCHGROUP,
        &[
            "repair-clearance",
            "{IN}",
            "--drc-report",
            "{WORK}/matchgroup-drc.json",
            "--mfr",
            "jlcpcb",
            "-o",
            "{OUT}",
            "--format",
            "json",
        ],
        None,
    );
}

#[test]
fn fix_drc_no_repairable() {
    case(
        "fd_charlie",
        CHARLIE,
        &[
            "fix-drc",
            "{IN}",
            "--drc-report",
            "{WORK}/charlie-drc.json",
            "--mfr",
            "jlcpcb",
            "-o",
            "{OUT}",
            "--format",
            "json",
            "--no-local-reroute",
        ],
        None,
    );
}

#[test]
fn fix_drc_repairs() {
    case(
        "fd_matchgroup",
        MATCHGROUP,
        &[
            "fix-drc",
            "{IN}",
            "--drc-report",
            "{WORK}/matchgroup-drc.json",
            "--mfr",
            "jlcpcb",
            "-o",
            "{OUT}",
            "--format",
            "json",
            "--no-local-reroute",
        ],
        None,
    );
}

#[test]
fn fix_vias_relocate_in_pad_moves_json() {
    case(
        "fv_free_slide_json",
        "wave_d/free_slide.kicad_pcb",
        &[
            "fix-vias",
            "{IN}",
            "--mfr",
            "jlcpcb",
            "--relocate-in-pad",
            "-o",
            "{OUT}",
            "--format",
            "json",
        ],
        None,
    );
}

#[test]
fn fix_vias_relocate_in_pad_moves_text() {
    case(
        "fv_free_slide_text",
        "wave_d/free_slide.kicad_pcb",
        &[
            "fix-vias",
            "{IN}",
            "--mfr",
            "jlcpcb",
            "--relocate-in-pad",
            "-o",
            "{OUT}",
        ],
        Some("fv_free_slide_json"),
    );
}
