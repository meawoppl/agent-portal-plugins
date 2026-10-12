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

Install builds the binary with `scripts/setup.sh`. Cargo writes to
`target/install-build`; only after a successful build does Rust atomically
activate the complete executable at `bin/kicadmium`. Every portal command uses
that stable path, so the previous executable remains callable throughout a
rebuild. `doctor --json` reports `runtime.installation.rebuild_in_progress`.
Concurrent installs are refused via `.install-lock/pid`; after an interrupted
install, check that PID is no longer running before removing the lock.

The portal gives plugin
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

### Fast regional feedback

`kicadmium drc --project ID --region X0 Y0 X1 Y1 --margin 2` runs native local copper/zone-clearance and courtyard checks. `kicadmium kct -- lint run BOARD --region X0 Y0 X1 Y1 --margin 2` runs a declared set of local trace/via/pad rules. Both print structured results with `engine: native-local`, explicit coverage, and `requires_full_verification: true`.

The regional snapshot retains intersecting objects and a guard margin, including crossing segments and neighboring pads. Content-addressed caches in `.kct-region-cache` include scoped geometry, bounds, margin, rule configuration, adjacent project/custom rules, policy bytes and engine version. Edits wholly outside the snapshot reuse local results; the reported board hash still reflects the current full board. Source boards are never written.

Regional checks are a partial fast-feedback path, not full KiCad DRC, schematic parity, global connectivity, or complete lint. Conditional custom DRC rules fail explicitly in native regional DRC. Always run full `verify` before commit; omitting `--region` preserves the existing full-check behavior.
### Regional before/after rendering

`kicadmium render --project ID --region 170 20 15 12 --layers F.Cu,F.SilkS,Edge.Cuts --zones net=VIN_5V --baseline HEAD -o region.png` produces a labeled comparison directly from board geometry. The region is x/y/width/height in millimetres; omit `--baseline` for the working board alone. A baseline can also be a board file. `--zones none|filled|net=NAME` controls existing fill polygons, `--highlight-nets GND,VIN_5V` highlights copper, and `--annotation` outlines the region. SVG and PNG have an opaque Tokyo-Night background. Baselines are read with Git, never checked out; rendering does not refill or save a board. Unfilled zones have no fill polygons to show. Publish the result with `agent-portal show region.png`.
### Supervised workbench (Linux user systemd)

Run the installed `bin/kicadmium surface ensure --cwd /path/to/project --port 8765` to start or reuse a workbench outside the Portal session's cgroup. It prints the local URL after the listener starts. Forward that port with `agent-portal forward 8765` for remote access. Processes restart automatically on failure and survive Portal service restarts; the Portal forwarding URL itself is session-owned and may need to be reopened by Portal.

Successful plugin activation restarts registered workbench units against the newly activated stable executable. Unit names are deterministic for each project directory and port, so repeated `ensure` calls reuse the same process. Inspect logs with `journalctl --user -u 'kicadmium-surface-*'`. This requires a running Linux user systemd manager; transient services last for that manager's lifetime, not across machine reboots. `serve` remains available for other platforms and Portal-managed hosting.

The Portal manifest starts the supervised surface automatically and keeps a lightweight launcher attached with `--wait`. On macOS the equivalent service is bootstrapped into the current GUI user's launchd domain with `KeepAlive`; installation restarts those registered jobs too. Neither platform requires a root/system service. Readiness requires a successful kicadmium `/healthz` response, and a port occupied outside the selected unit is rejected.
PCB region annotations collect current board context before queuing: intersecting footprints, copper nets by layer, regional lint/quality findings, and a SHA-256 board revision. The text attachment is capped below 40 lines with an explicit omitted count. Context collection reuses the quality build cache, evaluates the board's lint policy without writing it, and rejects a board change during collection. A collection failure keeps the annotation draft available for retry.
### One-shot verification

`kicadmium verify --project ID --baseline HEAD --json` runs full DRC (including schematic parity), ERC, lint, lint-exception stale checks, quality, and declared derived-document checks. Both working inputs and Git baselines are checked in disposable snapshots; a baseline board file can also be supplied with current project context. No board swapping or source saves occur.

Output uses `kicadmium.verify/v1`, with per-stage before/after counts, added/removed findings, and groups by rule and nets. Omit `--json` for a concise summary (seven lines when unchanged). `--gate error|warning|never` controls newly introduced finding failures; an incomplete stage always fails. Without a baseline, all current findings are considered new. Ignored lint findings do not fail the gate.

Caches include the complete selected source snapshot, executable hash, KiCad version and project identity. Git baselines cache by immutable commit; expiring lint decisions invalidate their cached report at expiry. Failed/incomplete stages are not cached. Full verification is the pre-commit gate; regional checks remain partial feedback.
