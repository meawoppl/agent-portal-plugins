KiCad PCB is available for this session. Use it for PCB and electronics work:

- open the KiCad PCB surface when visual board, schematic, BOM, or Gerber state
  matters;
- run doctor before relying on native KiCad tooling;
- use script commands (`doctor`, `drc`, `erc`, `quality`) as the proof path
  for PCB changes. Read the workspace `qualityProfile` first. "DRC clean" is
  not done until the layout-quality stage and the review checklist pass;
- prefer `kct` commands over ad-hoc pcbnew scripts;
- lint layout changes with `kct lint` (skill `kct-lint`). Board CI runs it and
  publishes findings plus your exceptions as contact sheets, so record
  non-issues in `<board>.lint.json` with specific reasons, and prune stale
  exceptions after geometry edits;
- produce fab outputs with the build strip's Publish, not ad-hoc exports.
  Check the stale badge, then post the Gerber ZIP, BOM, and CPL as separate
  `portal://file/...` links;
- when editing a PCB, identify the canonical configured `.kicad_pcb` path and
  keep saving meaningful milestones to that same active board unless the user
  explicitly asks for an alternate design;
- use the task-specific KiStack skills exposed by this plugin when the work
  matches them: `kicad-schematic`, `kicad-symbol`, `kicad-footprint`,
  `kicad-bom`, `kicad-pcb`, `kicad-layout`, `kicad-gerbers`, `kicad-export`,
  `kicad-panelize`, and `pcb-product-render`;
- ask before publishing or uploading manufacturing artifacts externally.
