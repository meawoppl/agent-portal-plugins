# Backplane PCB Workflow

Use this skill when a repo contains KiCad, Gerber, BOM, schematic, footprint, or
other electronics design artifacts and the Backplane Portal plugin is available.

## Workflow

1. Inspect `.portal/plugins.toml` first. Treat an explicit Backplane suggestion
   as project intent.
2. Run `agent-portal plugin open backplane` when visual PCB context would help.
   The surface should appear beside chat with tabs for schematic, PCB, Gerbers,
   3D, STEP, BOM, libraries, analysis, panelization, and checks.
3. Run `agent-portal plugin doctor backplane` or the plugin `doctor` command
   before promising native KiCad checks.
4. Use script commands as the automation boundary. Prefer:
   - `bin/backplane doctor --json --cwd <repo>`;
   - `bin/backplane drc --json --cwd <repo>`;
   - `bin/backplane erc --json --cwd <repo>`;
   - `bin/backplane export jlcpcb --cwd <repo> --out <artifact-dir>`.
5. Use the Backplane surface for visual claims. Prefer pointing at board,
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
and generated 3D board previews require `kicad-cli`. On Linux x86_64, run
`bin/backplane setup --install-kicad` to install the managed current-stable
KiCad runtime into the plugin's `.runtime/` directory. If usable KiCad tooling
is still unavailable afterward, report that as the blocker and do not claim
fabrication outputs have been validated.
