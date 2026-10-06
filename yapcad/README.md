# yapCAD Workbench

Agent Portal plugin for [yapCAD](https://github.com/rdevaul/yapCAD), the
procedural CAD system with a typed parametric DSL. Agents and users build DSL
commands into retained runs, review them in a live 3D/2D pane, pin points on
the model, and queue those annotations back to the agent.

## Install and run

```sh
agent-portal plugin install github:meawoppl/agent-portal-plugins//yapcad
agent-portal plugin setup yapcad
agent-portal plugin doctor yapcad
agent-portal plugin open yapcad
```

Requires Python 3.11+, uv and Node/npm. `setup` installs the pane's three.js
and icon assets; uv syncs the locked Python environment on first use. yapCAD
is pinned to upstream revision `5225474` (PyPI's 1.1.0 predates `yapcad.sdf`),
with manifold3d booleans and trimesh/pymeshfix/scipy diagnostics.

### BREP (optional)

pythonocc-core is only on conda-forge, so the default environment is yapCAD's
pure-Python tier: mesh and SDF solids, booleans, gears, fasteners, the DSL,
packages and assemblies. Fillets/chamfers of mesh solids, analytic STEP and
STEP import need BREP. Either:

- `bin/yapcad setup-brep` creates `.tools/brep` with micromamba/mamba/conda
  and installs the same yapCAD revision there; it is used automatically, or
- set `YAPCAD_PYTHON` to any Python with pythonocc-core and yapCAD installed.

`doctor` reports which interpreter and capabilities are active
(`doctor --require-brep` fails without BREP).

## Agent commands

All commands are `bin/yapcad ACTION --cwd PROJECT ...` and print JSON.

| Command | Purpose |
| --- | --- |
| `doctor` | interpreter, yapCAD/BREP/manifold/trimesh capabilities, discovered files |
| `list [FILE.dsl...]` | commands with typed params, defaults, `@ui` hints, `@meta` |
| `check [FILE.dsl...]` | parse and type-check; diagnostics with line, column, hints |
| `build --file F --command C [-p k=v]...` | retained run with stats and preview |
| `package --file F --command C [--name N]` | validated `.ycpkg` with provenance |
| `import --file PATH` | STL, STEP, DXF or `.ycpkg` as a reviewable run |
| `render [--run ID] [--view V]... [--size WxH] [--out P]` | PNG of a run |
| `runs [--limit N]` | history, newest first, with artifact paths |
| `validate [PKG...] [--strict]` | `.ycpkg` validation |
| `api [NAME]` | DSL built-ins, types, methods, patterns (yapCAD introspection) |
| `tool -- ARGS` | Python in the yapCAD environment (`-m yapcad.dsl ...`, scripts) |

Build options: `--params JSON`, `--representation mesh|sdf`, `--sdf-cell MM`,
`--no-simplify`, `--export stl,step,dxf,svg`, `--step-format analytic`,
`--strict-step`, `--strict-stl`, `--timeout S` (default 600), `--out DIR`
(copy artifacts). `-p` values are JSON when they parse (numbers, booleans,
lists), otherwise strings; ints are coerced to declared `float` parameters and
unknown parameter names are rejected before building.

Render views: `sheet` (default 2x2 iso/front/top/right), `iso`, `front`,
`back`, `top`, `bottom`, `left`, `right`, or a direction `x,y,z`. Previews use
a depth-sorted rasterizer with depth-tested feature edges on the Portal's dark
palette; show them with `agent-portal show`.

## Runs

Every build, package and import is a directory in `.yapcad-runs/` (which
ignores itself in git):

- `run.json` - spec, status (`ok`, `failed`, `timeout`, `cancelled`,
  `error`), message, params after type coercion, stats, exports and notes;
- `source.dsl` - snapshot of the source that was built; `revision` hashes
  the spec and source;
- `run.log` - worker output including DSL `print()` and tracebacks;
- `view.json` - per-part triangle soups, feature edges, colors, datums, or 2D
  polylines; `preview.png`;
- exports (`model.stl`, `model.step`, `model.dxf`, `model.svg`),
  `package.ycpkg/`, `assembly.json` for `emit_assembly()` commands.

Stats: bounding box, volume, area, volume centroid, triangle count,
watertightness with open and non-manifold edge counts, body count, Euler number
and per-part breakdowns. A failing `require` fails the run and the message
names its source line (yapCAD itself only records "Constraint violated").

Builds run in a separate worker process (the yapCAD interpreter) with a hard
timeout and cancellation; the pane runs one build at a time and a newer live
build replaces an in-flight one. CLI builds are independent processes and
appear in an open pane immediately.

## Pane

- File and command pickers; a parameter form generated from command
  signatures and `@ui(label, min, max, step, group, widget="slider")`, with
  changed values highlighted and remembered per command.
- Live mode rebuilds when the `.dsl` changes on disk or a parameter changes.
  Runs started by the agent are followed and their parameters adopted.
- Build options: representation, SDF cell size, exports, analytic STEP and
  Package.
- 3D viewport (Z up): orbit, fit, view presets, perspective/ortho, edges,
  wireframe, grid, X/Y/Z section plane with flip, per-part visibility and
  colors (material colors when present), assembly datum markers, click
  readout of coordinates, two-point measurement, numbered pins.
- 2D results as polylines in a locked top view; scalar results as a value.
- Result panel: status, `require` failures, notes, stats; downloads for
  exports, preview, package zip, assembly JSON, source snapshot and log.
- Library: open `.ycpkg`, STL, STEP and DXF files from the project.
- Source tab with command lines and `check` diagnostics inline; searchable
  DSL reference tab.
- Annotate: the viewport capture with pins and measurements burned in, plus
  run id, path, revision, params, stats, pins (point, part, face normal),
  measurement, section plane, camera and visible parts, queued to the
  Portal's `/__portal/edit-stack` (with optional voice dictation).

## Examples

- `examples/bracket/bracket.dsl` - a plate with `@ui` sliders and groups
  (`PLATE`), a 2D laser profile (`PROFILE`) and a scalar mass (`MASS_G`).
- `examples/linkage/linkage.dsl` - a two-part revolute assembly with datums
  (`POSED` geometry, `GRAPH` semantic JSON).

## Limits

- STL/STEP exports need solid results; DXF/SVG need 2D results. Without BREP,
  STEP is faceted and component STEP export is unavailable; component STL
  falls back to display meshes unless `--strict-stl`.
- Multiple emitted solids are exported as separate STL shells; STEP unions
  them first (slow for large meshes).
- Views are capped at 2,000,000 triangles. SDF builds are slow (tens of
  seconds for simple parts); raise `--sdf-cell` for drafts.
- Python design scripts are not discovered as builds; run them with `tool`
  and `import` the files they write.
- DSL `use` imports resolve however yapCAD resolves them from the source
  file's directory; the run snapshot captures only the built file.

## Testing

```sh
uv run --locked python -m unittest discover -s tests -v
bin/yapcad serve --cwd examples/bracket --port 49131 &
uv run python tests/browser_smoke.py http://localhost:49131
```
