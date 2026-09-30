# kicadmium

> KiCad's toxic uncle everyone warned you about.

He shows up uninvited, reads your whole board, tells you your vias are ugly,
and he's right. kicadmium is a single Rust binary that wraps a real KiCad
install with a browser workbench (schematic, PCB, Gerbers, 3D/STEP, BOM,
libraries, checks), fabrication export, layout-quality audits, and a pile of
agent skills. It does not ship KiCad. Bring your own; he'll judge it.

## Quick start

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked

cargo run -p backend -- serve --port 48888 --cwd /path/to/hardware/repo
# -> http://127.0.0.1:48888/
```

## CLI

```sh
kicadmium doctor --json --cwd .          # what tools/projects he found
kicadmium drc --json --cwd . --project board-a
kicadmium erc --json --cwd .
kicadmium quality --json --cwd .         # kct rules + in-house audits vs qualityProfile
kicadmium lint --json --cwd .            # 101 evidence-aware heuristic checks
kicadmium export gerbers --cwd . --out build/gerbers
kicadmium export jlcpcb  --cwd . --out build/jlcpcb
kicadmium setup                          # report the KiCad install he found
kicadmium kct  --cwd . -- readiness . --format json
kicadmium tool --cwd . -- kicad-cli version
# integrated read-only heuristic review (also exposed as a library)
pcb-lint lint board.kicad_pcb --board-id board-a --output lint.json
```

Environment: `KICADMIUM_KICAD_CLI`/`KICAD_CLI` pick kicad-cli,
`KICADMIUM_HOME` holds managed runtimes,
`KICADMIUM_LIBRARY_WORKERS` / `KICADMIUM_LIBRARY_CACHE_MB` tune library renders.

## JLCPCB placement corrections

When JLCPCB's part model does not match the KiCad footprint, record the
correction on the part with optional `JLCPCB Rotation Offset` and
`JLCPCB Position Offset` fields. Put them on the symbol (KiCad propagates them
to the footprint on *Update PCB from Schematic*) or directly on the footprint;
hide them on a Fab layer so silkscreen stays clean.

| Field | Value | Meaning |
|-------|-------|---------|
| `JLCPCB Rotation Offset` | degrees, e.g. `-90` | added to KiCad's rotation; `-90` is a quarter turn clockwise |
| `JLCPCB Position Offset` | `x,y` in mm, e.g. `0,-2.75` | footprint-local offset, KiCad +Y down, before rotation/flip |

Corrections stay attached to the footprint as the part moves, rotates, or
flips. `export jlcpcb` logs every corrected reference and writes
`<board>-placement-corrections.json` beside the CPL; the Libraries tab shows a
`JLC corr.` badge for corrected parts. The old `manufacturer.placementOffsets`
or `docs/jlcpcb-placement-offsets.json` table is still read as a deprecated
fallback for parts without fields, and part fields win over table entries.

## Layout

See [AGENTS.md](AGENTS.md). Rust workspace: `backend/` (Axum + CLI),
`frontend/` (Yew and embedded viewers), `shared/` (typed protocols), and
`pcb-lint/` (101 read-only, evidence-aware review rules). The linter is a
heuristic reviewer, not native ERC/DRC and not permission to edit your board.

The design rules we stole on purpose—and the places where we refuse to fake
certainty—are in [docs/design-principles.md](docs/design-principles.md).

## Inspirations

Ported out of the Agent Portal `kicad-pcb` plugin and built in the pattern of
[single-binary-rust-website](https://github.com/meawoppl/single-binary-rust-website).
It steals ideas shamelessly from:

- [Copperhead](https://github.com/copperheadhq/copperhead), for treating electronics design as an agent-native engineering workspace.
- [i2cjak/Backplane](https://github.com/i2cjak/Backplane), for truthful multi-view hardware inspection, revision-aware UI state, and its broader agent-driven electronics conventions.
- [i2cjak/Backplane_KiCad](https://github.com/i2cjak/Backplane_KiCad), Backplane's focused KiCad IPC fork.
- [rjwalters/kicad-tools](https://github.com/rjwalters/kicad-tools), for machine-readable inspection, validation, manufacturing, and mutation contracts.
- Thea Flowers' [KiCanvas](https://github.com/theacodes/kicanvas) and [Gingerbread](https://github.com/wntrblm/Gingerbread), for making KiCad designs genuinely useful in the browser and treating PCB output as a creative, inspectable medium.
- [American-Embedded/kistack](https://github.com/American-Embedded/kistack), for the practical KiCad agent workflows vendored here.
- [pastebom.com](https://github.com/meawoppl/pastebom.com), for reusable Rust PCB extraction and Gerber-viewing machinery.
