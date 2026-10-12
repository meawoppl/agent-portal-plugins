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
### Pin-1 dots that follow placement

Migrate loose board circles once with `kicadmium kct -- pin1 attach BOARD --markers markers.json --dry-run`, then `--in-place` (or `--output BOARD`). The JSON is an array of explicit associations, for example `[{"reference":"U2","uuid":"the-board-circle-uuid"}]`. The command validates the silk side and distance to pad 1/A1 (`--max-distance`, default 2.5 mm), converts board coordinates to footprint-local coordinates, and preserves the circle UUID, radius, stroke and fill. Repeated migration is idempotent. Invalid batches make no changes.

The resulting `fp_circle` is owned by the footprint. KiCad and native placement edits move and rotate it automatically; there is no marker-position file to keep synchronized. Existing pin-1 quality checks inspect footprint graphics and report a missing marker when no eligible mark lies within the configured search radius. Use the board as the authoritative marker geometry.
### Derived documentation

Declare project-root-relative outputs in the project's `.kicad-pcb.json` entry:

```json
"derivedFiles": [
  {"path":"docs/final-placements.json","kind":"placements"},
  {"path":"docs/testpoints.json","kind":"testpoints"},
  {"path":"docs/testpoints.csv","kind":"testpoints"},
  {"path":"docs/testpoints.md","kind":"testpoints"},
  {"path":"docs/pin1-markers.json","kind":"pin1-markers"}
]
```

Run `kicadmium kct -- docs sync --project ID` after editing. `--check` is read-only and returns 1 for stale outputs. Placement records carry reference, value, position, rotation and side; test-point records add label and nets. JSON preserves additional authored fields by reference while updating authoritative geometry fields. CSV and Markdown are wholly generated. Footprint-owned pin-1 silk circles near pad 1/A1 produce marker records in board and local coordinates. Removed footprints disappear from generated records. Unrecognized board-level circles are not guessed to be pin-1 markers.
### Native edit plans

`kicadmium kct -- edit BOARD --plan plan.yaml --dry-run` evaluates a complete candidate without modifying the board. Use `--in-place` or `--output` to save. New native copper-clearance findings block writes unless `--allow-violations` is explicit. This is a preflight; run full KiCad DRC after editing, especially after zone changes (zones are created unfilled).

```yaml
version: 1
operations:
  - {op: move, reference: U14, at: [179.2, 29.75], rotation: 90}
  - op: add-track
    net: GND
    layer: F.Cu
    width: 0.3
    path: [U14.5, [179.2, 29.75], C63.1]
  - {op: add-via, net: GND, at: C63.1, size: 0.6, drill: 0.3}
  - {op: add-zone, net: GND, layer: B.Cu, priority: 1, rect: [175, 25, 180, 30]}
```

Pad references resolve after preceding moves, using full precision and checking the requested net. Collinear forward runs within each added path are merged. `delete-tracks` takes `net`, `layer`, and a `path` of endpoint pairs, with optional `tolerance` in mm (default 0.000001); reversed endpoints match too. Every requested segment must exist. Footprint-owned text and graphics stay relative during moves. All operations validate before the candidate is saved; concurrent source changes abort the write.
