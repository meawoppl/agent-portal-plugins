---
name: pcb-product-render
description: Export and review current KiCad PCB 3D assets with Kicadmium and kicad-cli.
---

# PCB Product Render

Use this skill when a user wants a current PCB 3D export or a set of review
views. The supported workflow uses Kicadmium's Rust workbench and `kicad-cli`;
do not introduce an auxiliary rendering runtime or helper script.

## Required workflow

1. Resolve the authoritative `.kicad_pcb`. If the user asks for the latest
   board, regenerate outputs rather than trusting filenames or timestamps.
2. Run `kicadmium drc` and record its report and counts. Stop on violations
   unless the user explicitly authorizes proceeding for this task.
3. Use `kicadmium export` or `kicad-cli pcb export step` and `glb` with current
   command help. Include tracks, pads, zones, silkscreen, soldermask, and via
   body cuts where supported.
4. Inspect the GLB in Kicadmium's 3D view. Confirm outline shape, board
   thickness, mounting holes, cutouts, component placement, and model
   transforms. Missing models remain explicit findings; never fabricate a
   replacement silently.
5. Capture and inspect top, bottom, and perspective review views. Do not call
   an export correct solely because its command succeeded.
6. Verify dimensions, source revision, output hashes, and timestamps before
   handoff.

## Visual review

Confirm soldermask color and openings, exposed finish, component bodies,
connector orientation, pin-one cues, board edge geometry, and through-hole
features against the board source and available manufacturer references. Treat
the view as evidence of geometry, not proof of material appearance.

## Handoff

Report the output directory, board revision, DRC status, inspected views, and
every missing or substituted source model. Link generated assets and call out
limitations in the source footprint/model geometry.
