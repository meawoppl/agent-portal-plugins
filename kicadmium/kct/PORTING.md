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
| F | native Rust route, route-auto, benchmark, bench, calibrate | kc-claude (subagent) |
| G | parts, datasheet, suggest, mfr (+ `manufacturers`), init, footprint, panel, export, create-pcb, build, spec, pipeline, config, doctor, clean | kc-codex |
| H | ipc, reason, interactive, run | shell-native agent workflows |

`kct --help` marks commands still `[not yet ported]`; they exit 3. Wave G is
native: manufacturer presets/DRU, parametric footprints, live LCSC parts and
cache, datasheet acquisition/PDF analysis, suggestions, project/spec setup,
panelization, manufacturing export, cleanup/doctor/config, and the repair/build
orchestrators. These implementations invoke `kicad-cli` only for KiCad-native
exports and checks; no Python interpreter or wrapper remains in their path.

### Known practical divergence

`optimize-placement` currently uses a deterministic force-directed optimizer
with HPWL, overlap, and board-boundary penalties. Upstream uses CMA-ES. The
native command is useful and reproducible on real boards today; CMA-ES remains
a future fidelity improvement rather than a claim of algorithmic parity.

### `kct run` migration

Upstream `kct run FILE.py -- ARGS...` executes arbitrary Python in the
kicad-tools interpreter. Kicadmium cannot preserve that ABI without shipping
Python, so native `kct run` intentionally accepts a versioned JSON or YAML
workflow instead. A workflow contains `steps`, each with a native `command`,
`args`, and optional `continue_on_error`; `${1}` etc. expand trailing CLI
arguments and `${NAME}` expands values from the workflow's `env` map.
`--dry-run` prints every resolved command without executing it. This covers
sequencing, arguments, variables, and error policy, but does not execute Python
language constructs or imports; those must become native kct commands.
