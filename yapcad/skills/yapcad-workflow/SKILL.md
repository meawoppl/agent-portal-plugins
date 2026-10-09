---
name: yapcad-workflow
description: Author, build, inspect and export parametric yapCAD DSL parts and assemblies with the yapCAD Portal plugin - retained runs, previews, stats, STL/STEP/DXF/SVG exports and .ycpkg packages.
---

# yapCAD Workflow

The DSL sources in the repository are authoritative. Every build, package or
import becomes a run in `.yapcad-runs/RUN/` (self-ignored by git) with
`run.json`, `run.log`, `source.dsl` (snapshot), `view.json`, `preview.png` and
any exports. The open pane follows new runs, so CLI builds appear for the user.

Commands below are `bin/yapcad ACTION --cwd PROJECT ...` in the plugin
directory (`agent-portal plugin runtime yapcad` prints it). All print JSON.

## Loop

1. `doctor` - interpreter, `brep` (pythonocc-core), discovered sources.
2. `list [FILE.dsl]` - commands with typed params, defaults, `@ui` hints.
   `api [NAME]` - built-in signatures, types, methods and patterns from
   yapCAD's own introspection. Check a signature here before guessing.
3. Edit the `.dsl`, then `check [FILE.dsl]`; fix every error before building.
4. `build --file F --command C -p width=60 -p holes=[1,2] [--export stl,step]`.
   Values are JSON when they parse, otherwise strings; ints are coerced to
   declared floats. Exit status is non-zero on failure.
5. Read the summary: `status`, `message` (failing `require` with its line),
   `stats` (bbox, volume, area, centroid, watertight, boundary/non-manifold
   edges, bodies, euler number) and `files`. Then look at the preview with
   `agent-portal show FILES.preview`. For other angles:
   `render --view iso --view front --size 800x600` (or `--view 1,-2,1`).
6. Report with evidence: run id, stats that prove the change, the image.

Representation: default `mesh`; `--representation sdf` builds primitives,
fasteners and gears as signed distance fields (fillets/blends without OCC, but
much slower: tens of seconds per part; tune with `--sdf-cell MM`). Long builds
default to a 600 s timeout (`--timeout`).

## Exports and packages

- `--export stl,step,dxf,svg`. STL/STEP need a solid result; DXF/SVG need a 2D
  result (`region2d`, curves). Multiple solids are written as separate STL
  shells; STEP unions them first. STEP is faceted unless BREP is available
  and `--step-format analytic` / `--strict-step` is used. `notes` in the
  summary say exactly which path was taken - do not call a faceted STEP
  "analytic".
- `package --file F --command C --name N --version V` writes a validated
  `package.ycpkg` (provenance, DSL source attachment, assembly product
  definition and BOM for `assembly_compound` results;
  `--component-export stl|step` only for assembly commands).
- `import --file part.stl|part.step|drawing.dxf|thing.ycpkg` makes any
  geometry reviewable as a run; `validate [PKG]` checks packages.
- Offer files to the user as `portal://file/...` links relative to the cwd;
  use `--out DIR` on build/package to copy artifacts somewhere stable.

## DSL essentials

```
module bracket

command PLATE(width: float @ui(widget="slider", min=20.0, max=120.0) = 60.0,
              bore: float = 10.0) -> solid:
    require width > bore + 10.0
    let cut: float = 8.0
    let hole: solid = translate(cylinder(bore / 2.0, cut), 0.0, 0.0, -cut / 2.0)
    emit difference(box(width, 30.0, 6.0), hole)
```

- `UPPERCASE` commands are exported (listed and buildable); `lowercase`
  commands are private helpers. Every path must `emit` exactly once, with
  the declared return type (`solid`, `region2d`, `float`, `string`...).
- Statically typed: write `5.0` for floats; use `0.0 - x` for computed
  negation. No `while`; use bounded `for i in range(n)` with early `emit`.
- Geometry angles (`rotate`, `arc`, `ellipse`) are degrees; `sin`/`cos` take
  radians - wrap with `radians(...)`. Mixing them type-checks but is wrong.
- `box` and `sphere` are centred on the origin; `cylinder` and `cone` sit on
  z = 0 and extend +z. Over-cut subtractions by a millimetre each side so no
  coplanar faces remain.
- `require cond` documents and enforces the valid parameter envelope; a
  failing require fails the build and is reported with its source line.
- `@ui(label=, min=, max=, step=, group=, widget="slider")` drives the pane's
  parameter form; `@meta(...)` adds output metadata. Neither changes geometry.
- Reassigning in a block (`s = union(s, part)` inside `for`/`if`) updates
  the outer variable; a typed `s: solid = ...` in a block declares a new
  local. Booleans keep the first operand's `@meta` (material, tags).
- `extrude` and `helical_extrude` work without pythonocc (manifold3d meshes).
- Mesh-only installs cannot `fillet`/`chamfer` a mesh solid (doctor
  `brep: false`); use `--representation sdf` or BREP via `setup-brep` /
  `YAPCAD_PYTHON` (a conda Python with pythonocc-core and yapCAD).
- Assemblies: `assembly`, `add_part`, `add_named_mate`, `solve_assembly`,
  `set_joint_position` (radians), then `emit assembly_compound(asm)` for
  geometry or `emit_assembly(asm)` (string) for the semantic graph, saved as
  `assembly.json` in the run.
- `print(...)` output lands in `run.log`.

## Quality bar

- A geometry change is not done until it builds, `stats` match intent (size,
  volume direction, bodies = expected part count) and the preview shows it.
- Watertight `false`, non-manifold edges or an unexpected body count are
  defects to investigate (often coplanar or touching booleans), not noise.
- Do not weaken a `require` to make a build pass unless the user asked to
  change the envelope. Report pre-existing failures separately.
- `tool -- -m yapcad.dsl ...` or `tool -- script.py` runs raw yapCAD in the
  same environment for anything the workbench does not wrap; prefer the
  workbench commands so results stay retained and visible.
