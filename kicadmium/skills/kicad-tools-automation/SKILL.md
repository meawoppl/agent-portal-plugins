# Native kct Automation

Use this skill when a KiCad task needs structured analysis or edits beyond
native `kicad-cli`: routing, placement, manufacturer rules, sync drift, BOM
enrichment, part lookup, readiness reports, repair commands, or agent-oriented
JSON output.

The native Rust implementation is inspired by
[rjwalters/kicad-tools](https://github.com/rjwalters/kicad-tools). That project
is not installed or executed at runtime.

## Tool boundary

Use the single Kicadmium binary:

```console
kicadmium doctor --json --cwd <repo>
kicadmium kct --json --cwd <repo> -- <kct arguments...>
kicadmium tool --json --cwd <repo> -- kicad-cli <arguments...>
```

Consult `kicadmium kct --help` and subcommand help before acting. Never write a
Python or shell helper, create a virtual environment, or fall back to an
external `kct`. If a native command is absent, report that precise capability
gap.

## Read before write

Start with read-only board/schematic summaries, symbols, nets, net status,
checks, readiness, and dry-run optimization when exposed by the current CLI.
Build a factual picture before moving parts or routing.

Treat routing, placement, sync, fixes, zones, stitching, and repair as code
edits:

1. inspect `git status --short`;
2. resolve the project and authoritative board from `.kicad-pcb.json`;
3. establish a clean branch or explicit snapshot;
4. run the narrowest `kicadmium kct` command;
5. inspect the changed file and relevant workbench views;
6. rerun verification.

Verification baseline:

```console
kicadmium erc --json --cwd <repo> --project <project-id>
kicadmium drc --json --cwd <repo> --project <project-id>
kicadmium quality --json --cwd <repo> --project <project-id>
kicadmium lint --json --cwd <repo> --project <project-id>
```

For fabrication, use Kicadmium's publish/export flow, confirm that outputs are
not stale, and visually review Schematic, PCB, Gerbers, 3D, BOM, Libraries, and
Checks. Record manufacturer profiles, part/model provenance, accepted findings,
and remaining uncertainty in the handoff.
