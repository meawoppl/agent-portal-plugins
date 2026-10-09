---
name: unlinked-models
description: Review Simulink .slx/.mdl models without MATLAB using the Unlinked Portal plugin: render diagrams and subsystems to SVG, inventory blocks and solver settings, run the supported simulation subset with explicit settings, and transpile MATLAB scripts to Rust.
---

# Simulink models with Unlinked

Unlinked is an independent, partial reimplementation of Simulink import,
rendering and simulation. Rendering a model never implies it can be
simulated, and a successful simulation is not numerical equivalence with
MathWorks tooling. Say which of those you actually checked.

Start with `bin/unlinked doctor --cwd PROJECT`: it reports whether the pinned
CLI is installed and lists every `.slx`/`.mdl` model and `.m` script found.
Install the CLI with `bin/unlinked setup` (Cargo builds it into the plugin's
`.tools/`; nothing is installed system-wide).

The CLI is the authoritative interface; the pane is a viewer over it.
`bin/unlinked tool --cwd PROJECT -- <subcommand>`:

- `info MODEL` prints JSON: block type counts, per-system block/line counts
  with Simulink-style paths, mask and library-link counts, workspace variable
  names and the imported solver configuration. Read this before claiming
  anything about model structure.
- `render MODEL -o out.svg` renders the root; add `--system NAME` once per
  nesting level to descend. Names are exact block names with literal slashes:
  the `info` path `m/Outer//Slash/Inner` becomes `--system 'Outer/Slash'
  --system Inner`.
- `sim MODEL --stop T --step H --solver euler|rk4|rk45 -o trace.json|csv`
  requires explicit settings; they override the imported ones. Supply model
  workspace values with repeatable `--var NAME=EXPR`. Scripts next to the
  model are never executed automatically, so read a parameters `.m` file and
  pass its values explicitly. Root Inports have no source: bind each one with
  `--input-value BLOCK_ID=EXPR` (constants or arrays; find the IDs as
  `data-sid` on `data-type="Inport"` blocks in the rendered SVG). The step
  must divide every ZOH/RandomNumber sample time in the model. Unsupported
  blocks or settings fail with a diagnostic naming the block; report that
  rather than working around it. To find every workspace variable a model
  needs, rerun with placeholder `--var NAME=1` until the error changes, then
  replace the placeholders with real values.
- `transpile SCRIPT.m -o DIR` emits a Cargo project for the supported MATLAB
  subset; `--emit llvm-ir` additionally needs Cargo and rustc.

Trace signals are keyed by stable block SID; the SVG carries `data-sid` and
`data-name` on each block so the pane can label them. Pane runs are written under
`.unlinked-runs/` in the project with `run.json`, `run.log` and `trace.json`;
add that directory to the project's ignore file. Inputs are capped at 16 MiB.
