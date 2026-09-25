# kicad-tools Integration

`kicad-pcb` uses `rjwalters/kicad-tools` as its advanced KiCad automation
layer. The Portal plugin remains the UI, project discovery, live viewer, and
artifact browser; `kicad-tools` supplies deeper command-line workflows for
agents.

## Installation

The plugin installs `kicad-tools` into:

```text
kicad-pcb/.runtime/kicad-tools/
```

This keeps Python packages out of the user's global environment. The wrapper
resolves tools in this order:

1. `KICAD_PCB_KCT`
2. `.runtime/kicad-tools/bin/kct`
3. `kct` on `PATH`
4. `kicad-tools` on `PATH`

Install or refresh it with:

```console
bin/kicad-pcb setup --install-kicad-tools
```

The default package extra is `agent`, which expands to:

```text
kicad-tools[placement,parts,datasheet,report,native]
```

This covers the useful Portal workflows: routing, placement, parts/BOM,
datasheets, reports/readiness, manufacturer checks, and the native backend.
Use `--kicad-tools-extra all` only when a task explicitly needs upstream's full
optional set, including heavier GPU/research dependencies. Use
`--kicad-tools-extra base` for narrow CI or smoke environments.

## Command Boundary

All calls should go through the plugin wrapper:

```console
bin/kicad-pcb kct --json --cwd <repo> -- <kct arguments...>
bin/kicad-pcb tool --json --cwd <repo> -- <kct|kicad-cli|kikit> <arguments...>
```

The wrapper forwards everything after `--` verbatim to the selected upstream
tool and records the invoked command and cwd in JSON, which makes agent
summaries auditable. Do not add plugin aliases for specific upstream
subcommands unless Portal needs a durable UI affordance; otherwise the upstream
CLI should be allowed to evolve without changing this plugin.

## What The Plugin Uses

High-value `kicad-tools` capabilities:

- Read-only schematic and PCB queries: symbols, nets, pin positions, labels,
  summaries, connectivity, board metrics.
- Sync and LVS-style checks: schematic-only/PCB-only components, value drift,
  footprint drift, netlist mismatch, copper/net parity.
- Manufacturer rules: JLCPCB/PCBWay/Seeed/OSH Park profiles, `.kicad_dru`
  generation, rule comparison, DRC floors.
- Manufacturing readiness: checks, artifact hashes, BOM/CPL presence,
  upload-bundle completeness, human review records.
- Parts workflows: LCSC lookup, search, availability, BOM enrichment, and
  supplier identity validation.
- 3D model workflows: model substitution, LCSC model fetches, transform
  provenance, and render-based validation.
- Editing workflows: routing, route-auto, zones, stitching, clearance repair,
  footprint repair, ERC/DRC repair, placement analysis, and optimization.

## Skill Interaction

Agents should load the normal `pcb-workflow` skill for all KiCad work. Load
`kicad-tools-automation` before running `kct`, especially for mutation
commands.

Read-only commands can be used early to understand a board. Mutation commands
must follow the design-edit protocol:

1. inspect the working tree;
2. identify the board/project;
3. make or confirm a branch/snapshot;
4. run the narrowest `kct` command;
5. inspect the workbench visually;
6. run native ERC/DRC plus sync/readiness checks;
7. regenerate and verify fabrication artifacts when relevant.

## UI Mapping

The current workbench tabs map to `kicad-tools` like this:

| Workbench area | Native source | kicad-tools expansion |
| --- | --- | --- |
| Schematic | KiCad viewer | `symbols`, `nets`, `sch summary`, `sch connections` |
| PCB | KiCad viewer | `pcb summary`, `net-status`, routing/repair commands |
| Checks | `kicad-cli` ERC/DRC | `check`, `validate --sync`, manufacturer DRC floors |
| BOM | CSV discovery | `bom`, `parts availability`, BOM enrichment |
| Libraries | local parser | symbol/footprint/model drift and model substitutions |
| Gerbers | KiCad export + viewer | readiness artifact binding and submission review |
| 3D/STEP | KiCad GLB/STEP | LCSC model transforms and provenance |
| Analysis | placeholder | board metrics, signal/power/thermal/routing analysis |
| Panelization | placeholder | `panel` and manufacturing output checks |

The long-term goal is not to reimplement all of `kicad-tools` in the browser.
The plugin should expose the right command outputs and review artifacts beside
the live KiCad views.
