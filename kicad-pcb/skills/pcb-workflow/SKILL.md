# KiCad PCB Workflow

Use this skill when a repo contains KiCad, Gerber, BOM, schematic, footprint, or
other electronics design artifacts and the KiCad PCB Portal plugin is available.

## Workflow

1. Inspect `.portal/plugins.toml` and `.kicad-pcb.json` first. The manifest
   names the boards, the active `.kicad_pcb`, artifact dirs, and the
   `qualityProfile`. Read that profile (and any review checklist next to it)
   before you place or route anything. It records the user's layout
   preferences: via rules, octilinear routing, silkscreen policy, and
   decoupling distance.
2. Run `agent-portal plugin open kicad-pcb` when visual context would help.
   The surface has tabs for schematic, PCB, Gerbers, 3D, STEP, BOM, libraries,
   analysis, panelization, and checks, plus a build strip.
3. Run the plugin `doctor` command before promising native KiCad or `kct`
   checks.
4. Use the plugin commands as the automation boundary:
   - `bin/kicad-pcb-rs doctor --json --cwd <repo>`
   - `bin/kicad-pcb-rs drc|erc --json --cwd <repo> --project <id>`
   - `bin/kicad-pcb-rs quality --json --cwd <repo> --project <id>`
   - `bin/kicad-pcb-rs kct --json --cwd <repo> -- <kct args...>`
5. **Try `kct` before writing ad-hoc pcbnew/s-expr scripts.** Check
   `kct --help` and `kct <cmd> --help` for an existing query, audit, or fix
   (`check`, `detect-mistakes`, `optimize-traces --dry-run`, `net-status`,
   `validate --sync`, `fix-vias`, `place-silk-refs`, and others). Write a
   custom script only when kct cannot do the job, and say why.
6. Use the surface for visual claims. Point at the board, schematic, layer,
   net, or artifact instead of relying only on prose.

## Done Means More Than DRC

Never report "DRC clean" as done or fab-ready. DRC checks the fab's rules, not
layout quality. Before you call layout work complete:

1. ERC and DRC (with schematic parity) pass.
2. The **quality** stage is green, or each remaining finding is explained.
   Read it from the Checks tab **Layout quality** card, from
   `/api/kicad/quality`, or from the `quality` command. Severities come from
   the workspace `qualityProfile`. Fix errors. Justify any warning you keep.
3. Walk the repo's review checklist (e.g. `docs/pcb-playbook/review-checklist.md`)
   for items no tool checks: stubs, kinks, pad exits, pin-1 marks, and 3D
   models. Say which items you checked by eye.

## Fabrication Outputs

- Use **Publish** (the build-strip button, or `POST /api/build/publish?project=<id>`)
  to put Gerbers, BOM, CPL, and checks into the repo. Do not run ad-hoc
  `kicad-cli` exports into fab dirs. Publish records hashes in
  `<fab>/.kicad-pcb-build.json`.
- Check the build strip's stale badge before you hand off outputs. "fab outputs
  stale" means the sources changed since the last publish. Wait for the rebuild
  and publish again.
- After publishing, post the Gerber ZIP, BOM, and CPL as **separate**
  `portal://file/<path>` links, one per file.
- Ask before uploading designs or outputs to an external service.
- When the JLCPCB preview shows a part rotated or shifted against its pads,
  record the fix on the part: hidden `JLCPCB Rotation Offset` (degrees, CCW
  positive) and/or `JLCPCB Position Offset` (`x,y` mm in the footprint-local
  frame, +Y down) fields on the symbol and footprint. Do not edit the CPL by
  hand or add entries to the deprecated `docs/jlcpcb-placement-offsets.json`.
  Check the build log's correction list before publishing.

## Completion Standard

A good PCB turn ends with:

- changed files by category (schematic, PCB, libraries, fab, docs);
- ERC, DRC, and quality results, with counts and any accepted findings;
- checklist items reviewed by eye;
- published artifacts as links, or a clear reason there are none.

## Tooling Limits

Real ERC/DRC, exports, 3D previews, the build pipeline, and `kct` need local
tooling. On Linux x86_64, run
`bin/kicad-pcb-rs setup --install-kicad --install-kicad-tools` to install
managed tooling into the plugin's `.runtime/`. If tooling is still missing,
report that as the blocker and do not claim outputs are validated.

Load `kicad-tools-automation` before running `kct` mutation commands (route,
placement, sync-netlist, fix-*, zones, stitch). They are design edits.
