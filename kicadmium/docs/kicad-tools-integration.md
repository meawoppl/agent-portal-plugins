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

### Placement preflight

`kicadmium kct -- place-check BOARD --plan moves.yaml --quality-profile pcb-profile.yaml` evaluates move operations without writing a board. It builds the proposed placements in memory, checks neighboring courtyards, copper clearances and silkscreen, and checks moved ICs/capacitors against the pins served on their power nets. `decoupling.max_pin_distance_mm` is read from the profile (default 2 mm). Output contains findings, checked footprint count, and `written: false`; exit 2 means findings need attention.

Geometry checks keep nearby footprints/tracks/vias, including crossing segments, and retain zone geometry. Decoupling selection uses the complete board so a distant alternative capacitor cannot disappear from the analysis. This placement preflight supplements full post-edit DRC; copper-changing plans use `edit --dry-run`.
