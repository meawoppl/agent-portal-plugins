# Native kct integration

Kicadmium carries the ideas and command vocabulary pioneered by
[rjwalters/kicad-tools](https://github.com/rjwalters/kicad-tools) into the
workspace's native Rust `kct` crate. The upstream project is an inspiration and
compatibility reference, not a runtime dependency: Kicadmium does not create a
virtual environment, install Python packages, or delegate commands to an
external `kct` executable.

## Command boundary

Run automation through the single binary:

```console
kicadmium kct --json --cwd <repo> -- <arguments...>
kicadmium tool --json --cwd <repo> -- kicad-cli <arguments...>
```

Use `kicadmium kct --help` and the subcommand help as the source of truth. The
JSON envelope records the command and working directory for an auditable agent
handoff. `kicad-cli` remains an external dependency supplied by KiCad itself.

## Workflow

Native `kct` workflows cover structured schematic and board queries,
connectivity and sync checks, manufacturer-rule validation, manufacturing
readiness, parts/BOM enrichment, model provenance, placement, routing, zones,
stitching, and focused repairs. Availability is reported by the current
command's `--help`; never fall back to the historical Python tool.

Read-only inspection may run directly. Treat routing, placement, sync, repair,
zone, and stitch commands as design edits:

1. inspect the worktree and identify the active project;
2. establish a clean branch or explicit snapshot;
3. run the narrowest native command;
4. inspect the affected workbench view;
5. rerun native `kicadmium erc`, `drc`, `quality`, and applicable `kct`
   consistency checks;
6. republish and review fabrication artifacts when relevant.

The Checks tab combines KiCad's ERC/DRC with Kicadmium's Rust quality audits
and evidence-aware PCB lint. A clean native DRC is necessary but not sufficient
for manufacturing readiness.

## Workbench mapping

| Workbench area | Native automation |
| --- | --- |
| Schematic | `kct symbols`, `nets`, and schematic queries |
| PCB | board summary, net status, routing and focused repair |
| Checks | `kicadmium erc`, `drc`, `quality`, and `lint` |
| BOM | native BOM and parts workflows |
| Libraries | symbol, footprint, and model validation |
| Gerbers | `kicadmium export` plus the embedded Rust viewer |
| 3D/STEP | KiCad export plus native provenance checks |
| Analysis | board metrics and signal/power/routing analysis |
| Panelization | native panel commands and manufacturing checks |

This repository preserves attribution to upstream ideas while ensuring the
installed product remains one first-party Rust binary plus `kicad-cli`.

### Region inventory

`kicadmium kct -- query region BOARD.kicad_pcb X0 Y0 X1 Y1` lists intersecting
footprints, pads, tracks, arcs, vias and zones in millimetres. Copper crossing
the rectangle is included even when both endpoints lie outside it; vias expand
to every traversed copper layer. `--layers F.Cu,B.Cu` and `--nets GND,VCC`
filter results. `--format json` includes normalized coordinates, footprint
rotation/side and courtyard bounds, source hash, complete per-kind counts and
omission count. The default 50-item limit keeps text under 60 lines; use
`--limit 0` for all results. The query is read-only.

### Routing probes

`query via-sites BOARD --net GND --near 180,27 --radius 2 --size 0.6 --drill 0.3` returns distance-ranked, sampled through-via sites across every copper layer. Copper, holes, keepouts, board edges, and silk-text envelopes are obstacles. `--step` controls sampling (default 0.25 mm) and `--limit` bounds results.

`query corridor BOARD --net GND --layer F.Cu --from 175,27 --to 180,29 --width 0.3` reports a straight path and two 45-degree alternatives, with blockers for each. Both commands emit JSON and never edit the board.

Queries use conservative maxima across project, netclass, and custom-rule minima, including conditional/layer-specific custom constraints. This can reject routes that a more specific rule would allow. Unsupported custom constraints or incomplete geometry fail explicitly rather than claiming a legal candidate. A via-site search is a bounded grid search, not an exhaustive proof that no continuous-space solution exists.
