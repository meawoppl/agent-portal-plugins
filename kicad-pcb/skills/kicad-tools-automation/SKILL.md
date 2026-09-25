# kicad-tools Automation

Use this skill when a KiCad task needs analysis or edits beyond native
`kicad-cli`: routing, placement, manufacturer rules, sync drift, BOM
enrichment, part lookup, readiness reports, repair commands, or structured
query output for an agent.

## Tool Boundary

Prefer the plugin wrapper instead of calling `kct` directly:

```console
bin/kicad-pcb kct --json --cwd <repo> -- <kct arguments...>
```

The wrapper resolves the plugin-managed `kct` from `.runtime/kicad-tools` or
`KICAD_PCB_KCT`, records the command, and keeps execution rooted in the active
repo.

Before using `kct`, run:

```console
bin/kicad-pcb doctor --json --cwd <repo>
```

If `tools.kct` is false, run:

```console
bin/kicad-pcb setup --install-kicad-tools
```

## Read-Only Analysis

These commands are safe first moves because they should not alter board files:

```console
bin/kicad-pcb kct --json --cwd <repo> -- symbols <board>.kicad_sch --format json
bin/kicad-pcb kct --json --cwd <repo> -- nets <board>.kicad_sch --format json
bin/kicad-pcb kct --json --cwd <repo> -- sch summary <board>.kicad_sch --format json
bin/kicad-pcb kct --json --cwd <repo> -- pcb summary <board>.kicad_pcb --format json
bin/kicad-pcb kct --json --cwd <repo> -- validate --sync <board>.kicad_pcb --format json
bin/kicad-pcb kct --json --cwd <repo> -- readiness . --format json
```

Use these to build a factual picture before moving parts or routing.

## Routing And Repair

Treat routing, placement, sync, fix, zone, stitch, and repair commands as code
edits. Do not run them casually on a dirty tree.

1. Inspect `git status --short`.
2. Identify the active board from `.kicad-pcb.json` or the workbench selector.
3. Create or confirm an explicit branch/snapshot.
4. Run the narrowest command that matches the request, for example:

```console
bin/kicad-pcb kct --json --cwd <repo> -- route <board>.kicad_pcb --strategy negotiated
bin/kicad-pcb kct --json --cwd <repo> -- route-auto <board>.kicad_pcb
bin/kicad-pcb kct --json --cwd <repo> -- pcb sync-netlist <board>.kicad_pcb
bin/kicad-pcb kct --json --cwd <repo> -- zones <subcommand> <board>.kicad_pcb
bin/kicad-pcb kct --json --cwd <repo> -- stitch <board>.kicad_pcb
bin/kicad-pcb kct --json --cwd <repo> -- fix-drc <board>.kicad_pcb
bin/kicad-pcb kct --json --cwd <repo> -- fix-erc <board>.kicad_sch
```

After any mutation, verify:

```console
bin/kicad-pcb erc --json --cwd <repo> --project <project-id>
bin/kicad-pcb drc --json --cwd <repo> --project <project-id>
bin/kicad-pcb kct --json --cwd <repo> -- validate --sync <board>.kicad_pcb --format json
bin/kicad-pcb export jlcpcb --cwd <repo> --project <project-id> --out <artifact-dir>
```

Then review the workbench Schematic, PCB, Gerbers, 3D, BOM, Libraries, and
Checks tabs before claiming the design is ready.

## Manufacturer Readiness

Use `kct` manufacturer/readiness workflows when preparing real fabrication:

```console
bin/kicad-pcb kct --json --cwd <repo> -- mfr compare
bin/kicad-pcb kct --json --cwd <repo> -- init <project>.kicad_pro --mfr jlcpcb
bin/kicad-pcb kct --json --cwd <repo> -- audit <project>.kicad_pro --mfr jlcpcb --format json
bin/kicad-pcb kct --json --cwd <repo> -- readiness . --format json
```

If generated `.kicad_dru` files or fab outputs change, include them in the
review summary and say which manufacturer profile drove the change.

## BOM, Parts, And 3D Models

Use `kct parts` and model tooling to enrich the plugin's BOM/Libraries tabs:

```console
bin/kicad-pcb kct --json --cwd <repo> -- parts lookup C123456
bin/kicad-pcb kct --json --cwd <repo> -- parts search "100nF 0402" --in-stock
bin/kicad-pcb kct --json --cwd <repo> -- parts availability <board>.kicad_sch --quantity 100
```

When changing 3D models or model transforms, record provenance: board, refdes,
date, command, screenshot/render reviewed, and why the transform is correct.

## Completion Standard

A kicad-tools-backed PCB turn is complete only when the final response names:

- the `kct` command(s) used;
- files changed by category: schematic, PCB, libraries, fab artifacts, docs;
- ERC/DRC/sync/readiness results;
- workbench tabs visually reviewed;
- remaining manufacturing caveats, especially unverified parts, footprints,
  rotations, or 3D models.
