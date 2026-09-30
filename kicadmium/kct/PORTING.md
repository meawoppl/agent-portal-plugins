# kct: porting rjwalters/kicad-tools to Rust

Upstream reference: https://github.com/rjwalters/kicad-tools (MIT, see
`LICENSE-kicad-tools`), checked out at `~/repos/kicad-tools`, main `37665de5`
(v0.22.0+97). Port target: every `kct` command, in Rust, no Python.

## Rules

- Module map mirrors upstream: `kicad_tools/<pkg>/<mod>.py` -> `kct/src/<pkg>/<mod>.rs`.
  Keep upstream names for types/functions where reasonable so ports can be
  cross-checked.
- Commands live in `kct/src/cli/<command>.rs` (dashes -> underscores) and expose
  `pub fn run(args: Vec<OsString>, g: &Globals) -> Result<i32>`; register by
  setting `run: Some(<cmd>::run)` in `cli/mod.rs`'s `COMMANDS` row.
- Parse args with clap via `cli::parse_args::<Args>(name, args)`; keep upstream
  flag names, defaults, and output formats (`--format json|text|...`). JSON
  output must match upstream key names so existing consumers keep working.
- Port upstream tests alongside (`kct/tests/<area>.rs`, fixtures under
  `kct/tests/fixtures/`, copied from upstream `tests/fixtures`).
- No subprocesses to Python. Shelling out to `kicad-cli` is allowed where
  upstream does (KiCad itself is out of scope).
- Mutating commands write through `fsutil::atomic_write` and keep untouched
  s-expression text byte-exact (`Document::save`).

## Waves and owners

| Wave | Commands | Owner |
| --- | --- | --- |
| foundation | `sexp`, `units`, `fsutil`, `cli` registry, `core`, `schema` | kc-claude |
| A | check, drc, erc, explain, detect-mistakes | kc-claude |
| B | symbols, nets, netlist, sch, bom, lib, validate, sync | kc-codex |
| C | pcb, analyze, net-status, board-metrics, audit, readiness, report, estimate, fleet, render, screenshot | kc-claude (subagent) |
| D | optimize-traces, validate-footprints, fix-footprints, fix-vias, fix-silkscreen, place-silk-refs, repair-clearance, fix-drc, fix-erc | kc-claude |
| E | zones, stitch, creepage, creepage-export-rules, impedance, constraints, placement, optimize-placement, decisions, optim | kc-claude (subagent) |
| F | route, route-auto, benchmark, bench, calibrate, build-native | kc-claude (subagent) |
| G | parts, datasheet, suggest, mfr (+ `manufacturers`), init, footprint, panel, export, create-pcb, build, spec, pipeline, config, doctor, clean | kc-codex |
| H | mcp, ipc, reason, interactive, run | decide last |

`kct --help` marks every command still `[not yet ported]`; they exit 3.
