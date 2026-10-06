# kicadmium

> The heavy metal your PCBs were missing.

Cadmium is a heavy metal; kicadmium is the one your boards needed. It reads
your whole board, tells you your vias are ugly, and it's right. kicadmium is a
single Rust binary that wraps a real KiCad install with a browser workbench
(schematic, PCB, Gerbers, 3D, BOM, libraries, checks), fabrication export,
layout-quality audits, and a pile of agent skills. It does not ship KiCad.
Bring your own and turn it up.

## Quick start

As an Agent Portal plugin:

```sh
agent-portal plugin install github:meawoppl/agent-portal-plugins//kicadmium
```

Install builds the binary with `scripts/setup.sh`. The portal gives plugin
commands a private `HOME`, so the script points rustup, cargo, trunk and
sccache at the account's existing caches (otherwise every install downloads a
toolchain and the crate index afresh) and uses `sccache` as the rustc wrapper
when it is installed. A reinstall then recompiles only what changed: about a
minute when nothing did, two with a `kct` change, versus twenty cold.
`KICADMIUM_SETUP_PLAIN=1` disables all of that.

From a checkout of this directory:

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
kicadmium lint --json --cwd .            # = kct lint run on the project board
kicadmium export gerbers --cwd . --out build/gerbers
kicadmium export jlcpcb  --cwd . --out build/jlcpcb
kicadmium setup                          # report the KiCad install he found
kicadmium kct --help                     # native Rust PCB automation
kicadmium kct  --cwd . -- readiness . --format json
kicadmium tool --cwd . -- kicad-cli version
# canonical board lint: adjacent policy, evidence-bound exceptions, CI bundle
kicadmium kct -- lint run board.kicad_pcb --fail-on error
kicadmium kct -- lint waive board.kicad_pcb FINDING_KEY --reason "intentional" --reviewer agent
kicadmium kct -- lint ci boards/*.kicad_pcb --out lint-results --fail-on error
```

Environment: `KICADMIUM_KICAD_CLI`/`KICAD_CLI` pick `kicad-cli`;
`KICADMIUM_LIBRARY_WORKERS` and `KICADMIUM_LIBRARY_CACHE_MB` tune library
renders. Native `kct` is part of the Kicadmium binary and needs no runtime
installation.

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
`kct/` (the native kicad-tools port, including the `kct lint` engine with 101
read-only, evidence-aware review rules). The linter is a
heuristic reviewer, not native ERC/DRC and not permission to edit your board.
Its canonical lifecycle is `kicadmium kct -- lint`; see
[docs/lint.md](docs/lint.md) for board-local exceptions, routing gates, and CI
artifacts.

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
- [punkfab/circuit-skills](https://github.com/punkfab/circuit-skills), for its simulate-then-lay-out workflow and its record of verification gates that gave false passes. Ideas only: the repository has no licence, so none of its code or text is used.
- [parisxmas/fastroute](https://github.com/parisxmas/fastroute), a Rust port of Freerouting, for its unrouted-connection diagnosis, best-so-far checkpoints and benchmark boards. It is GPL-3.0-or-later, so if kicadmium uses it, it runs as a separate, discovered program; none of its code is linked or vendored.
