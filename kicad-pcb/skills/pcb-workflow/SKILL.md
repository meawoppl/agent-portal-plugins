# KiCad PCB Workflow

Use this skill when a repo contains KiCad, Gerber, BOM, schematic, footprint, or
other electronics design artifacts and the KiCad PCB Portal plugin is available.

## Workflow

1. Inspect `.portal/plugins.toml` first. Treat an explicit KiCad PCB suggestion
   as project intent.
2. Run `agent-portal plugin open kicad-pcb` when visual PCB context would help.
   The surface should appear beside chat with tabs for schematic, PCB, Gerbers,
   3D, STEP, BOM, libraries, analysis, panelization, and checks.
3. Run `agent-portal plugin doctor kicad-pcb` or the plugin `doctor` command
   before promising native KiCad checks.
4. Use script commands as the automation boundary. Prefer:
   - `bin/kicad-pcb doctor --json --cwd <repo>`;
   - `bin/kicad-pcb drc --json --cwd <repo>`;
   - `bin/kicad-pcb erc --json --cwd <repo>`;
   - `bin/kicad-pcb export jlcpcb --cwd <repo> --out <artifact-dir>`;
   - `bin/kicad-pcb kct --json --cwd <repo> -- <kct arguments...>` for
     routing, readiness, manufacturer rules, parts lookup, sync drift, or
     repair workflows from `kicad-tools`.
5. Use the KiCad PCB surface for visual claims. Prefer pointing at board,
   schematic, layer, net, BOM, or manufacturing artifacts over relying only on
   prose.
6. Before saying PCB work is complete, run the available check path:
   - DRC for board changes;
   - ERC for schematic changes when supported;
   - BOM/fabrication export checks for release/manufacturing changes.
7. Put generated manufacturing artifacts in the session artifact directory or
   the repo's configured fabrication output directory.
8. Ask before publishing designs or manufacturing outputs to an external
   service.

## Completion Standard

A good PCB turn ends with:

- changed files summarized by board/schematic/manufacturing category;
- checks run and their result;
- artifacts generated or a clear reason they were not;
- a short visual summary if the surface revealed layout-relevant state.

## Tooling Limits

The Portal plugin surface can browse project files without KiCad, but real
ERC/DRC, Gerber/drill export, BOM export from schematic settings, panelization,
generated 3D board previews, and advanced `kicad-tools` automation require
local tooling. On Linux x86_64, run
`bin/kicad-pcb setup --install-kicad --install-kicad-tools` to install managed
tooling into the plugin's `.runtime/` directory. If usable tooling is still
unavailable afterward, report that as the blocker and do not claim fabrication
outputs have been validated.

Use the `kicad-tools-automation` skill before running `kct` mutation commands
such as routing, placement optimization, sync-netlist, repair, zone fill, or
stitching. Those commands are design edits and require the same review standard
as manual schematic/PCB changes.
