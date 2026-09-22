# Backplane PCB Workflow

Use this skill when a repo contains KiCad, Gerber, BOM, schematic, footprint, or
other electronics design artifacts and the Backplane Portal plugin is available.

## Workflow

1. Inspect `.portal/plugins.toml` first. Treat an explicit Backplane suggestion
   as project intent.
2. Run `agent-portal plugin open backplane` when visual PCB context would help.
   The surface should appear beside chat.
3. Run `agent-portal plugin doctor backplane` or the plugin `doctor` command
   before promising native KiCad checks.
4. Use the Backplane surface for visual claims. Prefer pointing at board,
   schematic, layer, net, BOM, or manufacturing artifacts over relying only on
   prose.
5. Before saying PCB work is complete, run the available check path:
   - DRC for board changes;
   - ERC for schematic changes when supported;
   - BOM/fabrication export checks for release/manufacturing changes.
6. Put generated manufacturing artifacts in the session artifact directory or
   the repo's configured fabrication output directory.
7. Ask before publishing designs or manufacturing outputs to an external
   service.

## Completion Standard

A good PCB turn ends with:

- changed files summarized by board/schematic/manufacturing category;
- checks run and their result;
- artifacts generated or a clear reason they were not;
- a short visual summary if the surface revealed layout-relevant state.

## Current Test Plugin Limits

This reference plugin contains a smoke-test wrapper. It validates the Portal
plugin lifecycle and surface contract, but full KiCad-native rendering and
automation should be provided by the upstream Backplane runtime.
