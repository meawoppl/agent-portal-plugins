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
