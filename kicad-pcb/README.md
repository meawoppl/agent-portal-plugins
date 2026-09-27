# KiCad PCB Portal Plugin

This plugin brings KiCad PCB workflows into Agent Portal as a
Portal-hosted session surface plus agent skills.

The plugin provides:

- `agent-portal-plugin.toml` declares install, surface, commands, detection,
  skills, and prompts.
- `bin/kicad-pcb-rs` launches (and builds when needed) the Rust runtime in
  `crates/`, which serves the tabbed PCB workbench. `bin/kicad-pcb` is the
  older Python runtime, now used only for the KiCad AppImage install and as a
  fallback (see [Runtime](#runtime)).
- `static/kicad-viewer/` carries bundled 3D/STEP viewer runtime
  assets.
- `skills/pcb-workflow/SKILL.md` teaches agents how to use the workbench, and
  `skills/kistack/` vendors the KiStack electronics skill bundle.
- `prompts/session.md` is the session reminder text Portal can inject.

The workbench mirrors the KiCad panel shape: schematic, PCB, Gerbers,
3D, STEP, BOM, footprints/symbols, analysis, panelization, and checks. Native
DRC/ERC/export/3D generation use `kicad-cli`. On Linux x86_64, setup installs
the current stable KiCad AppImage runtime under the plugin's `.runtime/`
directory and prefers that managed `kicad-cli` over the host distro copy.
Without KiCad, the surface still detects project files and presents the
workflow, but check/export commands fail loudly instead of producing placeholder
manufacturing output.

The plugin is intentionally script-first. Agents should use the documented CLI
commands and bundled skill instructions, with no additional integration setup.

## kicad-tools Automation

The plugin also integrates `rjwalters/kicad-tools` as the heavy automation
layer. Setup installs it into the plugin-owned virtualenv at
`.runtime/kicad-tools`, so the user's global Python environment is not changed.
Set `KICAD_PCB_KCT` to override the managed `kct` executable.
See `docs/kicad-tools-integration.md` for the full command, UI, and skill
mapping.

```console
kicad-pcb/bin/kicad-pcb-rs setup --install-kicad-tools
kicad-pcb/bin/kicad-pcb-rs doctor --json --cwd /path/to/hardware/repo
kicad-pcb/bin/kicad-pcb-rs kct --cwd /path/to/hardware/repo -- symbols board.kicad_sch --format json
kicad-pcb/bin/kicad-pcb-rs kct --cwd /path/to/hardware/repo -- nets board.kicad_sch --net VCC
kicad-pcb/bin/kicad-pcb-rs kct --cwd /path/to/hardware/repo -- readiness . --format json
kicad-pcb/bin/kicad-pcb-rs tool --cwd /path/to/hardware/repo -- kicad-cli version
```

The wrapper forwards arguments verbatim after `--`. Keep plugin commands thin:
prefer `kct -- <current upstream args>` or
`tool -- <kct|kicad-cli|kikit> <current upstream args>` over adding
plugin-specific aliases for upstream subcommands.

Use `kct` for workflows that KiCad's native CLI does not cover well:

- schematic/PCB drift and sync analysis;
- manufacturer-specific rule floors and `.kicad_dru` generation;
- order-readiness reports that bind checks, artifacts, BOM/CPL, and human
  review evidence;
- LCSC/JLCPCB part lookup and BOM enrichment;
- 3D model substitution and transform provenance;
- routing, placement, zone, stitch, and repair experiments.

Mutation commands such as `route`, `route-auto`, `pcb sync-netlist`,
`fix-footprints`, `fix-drc`, `fix-erc`, `repair-clearance`, `zones`, `stitch`,
and placement optimization must be treated as design edits. Run them only on a
clean working tree or an explicit branch/snapshot, then verify with the
workbench, native ERC/DRC, schematic parity, Gerbers, BOM/CPL, and 3D view
before calling the design complete.

By default, setup installs the plugin's `agent` kicad-tools bundle:
`kicad-tools[placement,parts,datasheet,report,native]`. This covers routing,
placement, readiness/reporting, parts/BOM enrichment, datasheet workflows, and
native acceleration without dragging in every optional GPU/research dependency.
Use `--kicad-tools-extra all` only when a task explicitly needs the entire
upstream extra set.

## Sources Of Inspiration

This plugin intentionally learns from adjacent KiCad automation projects. Treat
these repositories as watchlist inputs for periodic scrape/review jobs that
look for workflows, checks, viewer improvements, skills, and manufacturing
ideas worth bringing into the Portal plugin:

- `https://github.com/i2cjak/Backplane` - original Backplane workflow and
  browser PCB/schematic viewer lineage.
- `https://github.com/rjwalters/kicad-tools` - agent-oriented KiCad CLI
  automation, routing, manufacturer rules, readiness reports, parts lookup,
  repair flows, and structured JSON operations.
- `https://github.com/American-Embedded/kistack` - reusable KiCad/electronics
  skills bundled under `skills/kistack/`.
- `https://github.com/meawoppl/pastebom.com` - Gerber/BOM viewing and
  manufacturing artifact review patterns.

## Bundled KiStack Skills

This plugin includes the American Embedded KiStack electronics workflow skills and vendors the bundle from
`https://github.com/American-Embedded/kistack` at revision
`73ece96e45a3a202f2ca05af32c66dbcefcf9851` under `skills/kistack/` and exposes
each skill in `agent-portal-plugin.toml`:

- `kicad-bom`
- `kicad-export`
- `kicad-footprint`
- `kicad-gerbers`
- `kicad-layout`
- `kicad-panelize`
- `pcb-product-render`
- `kicad-pcb`
- `kicad-schematic`
- `kicad-symbol`

Keep this vendored tree immutable except when intentionally refreshing to a new
KiStack revision; record the source revision in `skills/kistack/kistack.bundle.json`.

## Try Locally

```console
kicad-pcb/bin/kicad-pcb-rs doctor --json --cwd /path/to/hardware/repo
kicad-pcb/bin/kicad-pcb-rs serve --port 48888 --cwd /path/to/hardware/repo
```

Then open:

```text
http://127.0.0.1:48888/
```

For production fabrication outputs, run setup once or set `KICAD_CLI` /
`KICAD_PCB_KICAD_CLI` to a desired executable:

```console
kicad-pcb/bin/kicad-pcb-rs setup --install-kicad --install-kicad-tools
kicad-pcb/bin/kicad-pcb-rs drc --json --cwd /path/to/hardware/repo --project board-a
kicad-pcb/bin/kicad-pcb-rs erc --json --cwd /path/to/hardware/repo --project board-a
kicad-pcb/bin/kicad-pcb-rs export jlcpcb --cwd /path/to/hardware/repo --project board-a --out build/jlcpcb
```

## Library View

The workbench **Libraries** tab is a card grid with one card per unique
(symbol, footprint, 3D model set) in the design, plus unused items from
project-local libraries (`sym-lib-table`/`fp-lib-table` entries inside the repo
and `.kicad-pcb.json` `libraries`). Each card shows a symbol | footprint | 3D
triptych rendered with `kicad-cli`, the references using it, values, LCSC/MPN
fields, and badges for missing 3D models, missing model files, and board or
schematic copies that drifted from the library. Clicking a render opens the
interactive KiCanvas/Three.js viewers.

Renders are content-addressed (item s-expression, referenced model bytes,
kicad-cli version, render parameters, renderer version) under
`.portal/cache/<repo-hash>/library/`, rendered lazily by a bounded worker pool
that prioritizes visible cards. `KICAD_PCB_LIBRARY_WORKERS` (default 2) and
`KICAD_PCB_LIBRARY_CACHE_MB` (default 256, LRU GC) tune the pool and cache.
APIs: `/api/kicad/library`, `/api/kicad/library/status`,
`/api/kicad/library/thumb/<key>.{svg,png}`; the legacy
`/api/kicad/libraries` link table remains available.

## Runtime

The manifest launches the Rust server through `bin/kicad-pcb-rs`. The wrapper
execs `.runtime/kicad-pcb-rs`, rebuilding it with
`cargo build --release -p kicad-pcb-server` first when the binary is missing or
any file under `crates/`, `static/`, `Cargo.toml`, or `Cargo.lock` is newer
than it. Build logs go to stderr. Set `KICAD_PCB_RS_BIN` to use a specific
binary, or `KICAD_PCB_SKIP_BUILD=1` to never build.

```console
cd kicad-pcb
bin/kicad-pcb-rs doctor --json --cwd /path/to/hardware/repo
bin/kicad-pcb-rs serve --port 48888 --cwd /path/to/hardware/repo
bin/kicad-pcb-rs quality --cwd /path/to/hardware/repo --project board-a
```

The Python runtime (`bin/kicad-pcb`) is still used only where Rust lacks
parity:

- `setup --install-kicad`: downloading and unpacking the KiCad AppImage into
  `.runtime/kicad/`. The wrapper runs it, then continues with Rust `setup`.
  The Rust runtime finds the managed `kicad-cli` there.
- Fallback when no Rust binary exists and cargo is unavailable. `serve`,
  `doctor`, `drc`, `erc`, `export`, `kct`, and `tool` then run in Python.
  The Python runtime has no build pipeline, publish, or `quality`.

## Example Repo Config

A PCB repository can suggest this plugin with:

```toml
# .portal/plugins.toml
schema_version = 1

[[suggested_plugins]]
name = "kicad-pcb"
source = "github:meawoppl/agent-portal-plugins//kicad-pcb"
reason = "This repo contains PCB design files and uses KiCad PCB review."
required = false

[suggested_plugins.config]
default_view = "board"
fabrication_output = "build/fab"
```

## Hardware Workspace Manifest

`.kicad-pcb.json` is an Agent Portal plugin manifest, not a KiCad file. Use it
at the repository root when a repo contains multiple boards or shared hardware
libraries. Paths in top-level sections are relative to the repo root. Paths
inside a project are relative to that project's `root`.

```json
{
  "version": 1,
  "defaultProject": "direction-led-tester",
  "libraries": {
    "symbols": ["libraries/symbols/company.kicad_sym"],
    "footprints": ["libraries/footprints/company.pretty"],
    "models": ["libraries/3dmodels"],
    "datasheets": ["libraries/datasheets"]
  },
  "projects": [
    {
      "id": "direction-led-tester",
      "name": "Direction LED Tester",
      "root": "boards/direction-led-tester",
      "kicad": {
        "project": "direction-led-tester.kicad_pro",
        "schematic": "direction-led-tester.kicad_sch",
        "pcb": "direction-led-tester.kicad_pcb"
      },
      "libraries": {
        "symbols": ["direction-led-tester.kicad_sym"],
        "footprints": ["pretty"],
        "models": ["3dmodels"]
      },
      "artifacts": {
        "fab": "fab",
        "checks": "fab/checks",
        "gerbers": "fab/gerbers",
        "jlcpcb": "fab/jlcpcb",
        "docs": "docs"
      },
      "manufacturer": {
        "default": "jlcpcb",
        "bom": "fab/jlcpcb/BOM_direction-led-tester.csv",
        "cpl": "fab/jlcpcb/CPL_direction-led-tester.csv",
        "package": "fab/jlcpcb/direction-led-tester-jlcpcb.zip"
      }
    }
  ]
}
```

Recommended layout:

```text
repo/
  .portal/plugins.toml
  .kicad-pcb.json
  libraries/
    symbols/
    footprints/
    3dmodels/
    datasheets/
  boards/
    board-a/
      board-a.kicad_pro
      board-a.kicad_sch
      board-a.kicad_pcb
      fp-lib-table
      sym-lib-table
      fab/checks/
      fab/gerbers/
      fab/jlcpcb/
      docs/
```

The workbench opens `defaultProject` first and shows a board selector when more
than one project is listed. If the manifest is absent, the plugin keeps the
single-board convention and treats the repository root as the active board
folder.

`doctor --json` validates the manifest before reporting tool state. Hard errors
include unsupported manifest versions, duplicate or empty project ids, missing
project roots, invalid `defaultProject`, and configured `kicad.*` files that are
missing or have the wrong extension. Missing shared or project library paths are
reported as warnings so in-progress repos still open in the workbench.

The Rust workbench and CLI currently honor:

- `defaultProject`
- `projects[].id`
- `projects[].name`
- `projects[].root`
- `projects[].kicad.project`
- `projects[].kicad.schematic`
- `projects[].kicad.pcb`
- `projects[].artifacts.fab`, `.checks`, `.gerbers`, `.jlcpcb` (publish targets;
  must be relative paths inside the project root)
- `projects[].artifacts.autoPublish`
- `projects[].manufacturer.placementOffsets` (deprecated)
- top-level `build`
- top-level `qualityProfile` and `quality`, and `projects[].qualityProfile`
  (see [Layout Quality Checks](#layout-quality-checks))
- top-level and project `libraries` for doctor warnings and source hashing

The `manufacturer` object is surfaced through `doctor --json` for agents. The
only key that alters generated files is the deprecated `placementOffsets` (see
JLCPCB placement corrections below).

## Automatic Build Pipeline

The Rust server builds fabrication outputs in the background for every board
opened in the workbench, caches them by content, and only copies them into the
repository when you publish.

**Revisions are content hashes.** A board's revision is a sha256 over the
relative path and contents of its sources, never mtimes, so touching or
re-saving an unchanged file does not rebuild anything. Sources split into two
sets so stages depend only on what they read:

- schematic set: `.kicad_sch`, `.kicad_sym`, `sym-lib-table`, `.kicad_pro`,
  `.kicad_wks`
- PCB set: `.kicad_pcb`, `.kicad_mod`, `fp-lib-table`, `.kicad_dru`,
  `.kicad_pro`, `.kicad_wks`, and project-local 3D models referenced by the
  board (`${KIPRJMOD}/...`)

Library directories that lib tables or `.kicad-pcb.json` `libraries` point to
outside the board root, but inside the repo, are included. KiCad's own
libraries (`${KICAD*_DIR}`) are covered by the KiCad version, which is part of
every stage key.

**Watch scoping.** Each changed file is mapped to the board(s) whose sources
it affects. A project whose root contains another project's root (for example
a root `"."` board plus `boards/*`) excludes the nested boards, so editing
one board never refreshes or rebuilds another. `/ws/events?project=<id>` and
`/api/build/events?project=<id>` (SSE) deliver only that board's events;
`Revision` events carry a `project` field.

**Stages.** Each stage's cache key is sha256(stage, KiCad version, project
config, and only the hashes it depends on):

| Stage id | Depends on | Output |
| --- | --- | --- |
| `erc` | schematic | `erc.json` |
| `drc` | schematic + PCB | `drc.json` (`--schematic-parity`) |
| `bom` | schematic | `<sch>-bom.csv` |
| `schematic-pdf` | schematic | `<sch>-schematic.pdf` |
| `gerbers` | PCB | Gerbers, drill, `<board>-gerbers.zip` |
| `jlcpcb` | schematic + PCB | Gerbers, drill, `BOM_<board>.csv`, `CPL_<board>.csv`, zip |
| `glb` | PCB | `<board>.glb` (served to the 3D tab) |
| `step` | PCB | `<board>.step` (lowest priority) |
| `quality` | schematic + PCB + quality profile + kct version | `quality.json` (layout-quality report) |

Outputs live under `<plugin>/.portal/cache/<root-hash>/builds/<stage>-<key>/`
and are reused whenever the key matches. Jobs run through a bounded queue
(default 2 concurrent `kicad-cli` processes) with per-command timeouts. A
newer revision drops queued jobs and kills running `kicad-cli` processes whose
outputs are no longer wanted. Outputs are discarded if the sources changed
while the stage ran. `GET /api/kicad/drc` and `/api/kicad/erc` serve the cached
result for the current revision, building it first if needed.

**Status.** The workbench shows a build strip under the header with every
stage's state (`queued`, `running`, `ok`, `failed`, `stale`, `cancelled`), its
timing, and downloads for its outputs. The same data is available from
`GET /api/build/status?project=<id>`, from `Build` events on `/ws/events`,
and from `/api/build/events`. `POST /api/build/run?project=<id>[&force=1]`
triggers a build (`force` discards cached results for that revision).

**Publish.** Builds never write into the repository by themselves.
`POST /api/build/publish?project=<id>` (the **Publish** button) copies the
current, finished build into the project's artifact dirs:

| Stage | Destination (defaults) |
| --- | --- |
| `erc`, `drc`, `quality` | `artifacts.checks` (`<fab>/checks`) |
| `bom` | `<fab>/bom` |
| `gerbers` | `artifacts.gerbers` (`<fab>/gerbers`) |
| `jlcpcb` | `artifacts.jlcpcb` (`<fab>/jlcpcb`) |
| `schematic-pdf`, `glb`, `step` | `<fab>` |

`<fab>` is `artifacts.fab`, or `fab` by default. Publish writes
`<fab>/.kicad-pcb-build.json`, which records the source hashes, per-file
source sha256s, the KiCad version, every stage's input key, and each published
file's sha256. It deletes files that an earlier publish wrote if the new build
no longer produces them. It never deletes files it did not write. Set
`artifacts.autoPublish: true` to publish automatically after each complete
build (default `false`).

**Staleness.** The strip shows a "fab outputs stale" badge when the published
manifest's stage keys differ from the current ones, when sources changed since
publish, or when published files were edited or removed. It lists the stages
that differ. Boards without a plugin manifest fall back to a
`<checks>/revision-sha256.json` (path to sha256 map) if one exists. The same
status appears in `/api/build/status` (`publish`) and in `doctor --json`
(`build.projects[].publish`).

**Gerber tab.** The tab renders the current cached build when its `gerbers`
(or `jlcpcb`) stage is ready. Otherwise it falls back to published files and
labels which one it is showing.

**Cache GC.** After each build the server keeps the last `keepRevisions`
revisions per board, plus every stage the published manifest references, and
deletes the rest. It also removes pre-pipeline `<revision>.glb` cache files.

**JLCPCB placement corrections.** When JLCPCB's part model does not match
the KiCad footprint (pin 1 a quarter turn off, origin shifted), record the
correction on the part itself with two optional fields. Put them on the symbol
(KiCad copies symbol fields to the footprint on *Update PCB from Schematic*)
or directly on the footprint; keep them hidden on a Fab layer so silkscreen is
unchanged.

| Field | Value | Meaning |
|-------|-------|---------|
| `JLCPCB Rotation Offset` | degrees, e.g. `-90` | added to KiCad's rotation (counter-clockwise positive, KiCad convention); `-90` = a quarter turn clockwise |
| `JLCPCB Position Offset` | `x,y` in mm, e.g. `0,-2.75` | shift in the **footprint-local** frame (KiCad +Y down, before rotation/flip) |

Both are relative to the footprint, so they stay right when the part is moved,
rotated, or flipped: the exporter maps the local offset to the board exactly
like a pad offset (bottom-side parts mirror local Y, then the footprint
rotation applies; verified against pcbnew at 0/90/180/270° on both sides),
then to CPL coordinates (+Y up). On the bottom side the rotation correction is
mirrored (subtracted) and flagged for preview review. Formats are tolerant
(`1.2, -0.5`, `1.2mm,-0.5mm`, `(1.2; -0.5)`, `-90°`); empty means none; an
unparseable value fails the export naming the reference. If the footprint and
schematic disagree the footprint wins and the build log warns.

To find a local offset from a shift observed in the JLCPCB preview at native
rotation θ: convert the CPL shift (+Y up) to KiCad board axes (negate Y), then
undo the footprint rotation θ. For example a part at 180° that must move 1.425 mm down on the
board has local offset `0,-1.425`; one at 0° that must move 2.75 mm up also
has a negative local Y: `0,-2.75`.

Each JLCPCB export logs every corrected reference with the applied rotation,
local offset, CPL shift, and source, and writes the same data to
`<board>-placement-corrections.json` beside the CPL (not included in the
upload ZIP). The Libraries tab shows a `JLC corr.` badge on cards whose parts
carry corrections, with the values (details on hover). This applies to both
the `jlcpcb` stage and `export jlcpcb`.

*Deprecated table.* The earlier per-board table (`manufacturer.placementOffsets`
or `docs/jlcpcb-placement-offsets.json`, keyed by LCSC number, offsets in CPL
+Y-up coordinates, translations pinned by `verified_native_rotation_degrees`)
is still read as a fallback for parts without the fields, with a deprecation
warning in the build output. Part fields win over table entries. To migrate,
for each table entry add `JLCPCB Rotation Offset` = `rotation_offset_degrees`
to every part with that LCSC number, and convert `cpl_offset_x_mm/y_mm` at the
verified rotation θ to a local offset (`x_local = dx·cosθ + dy·sinθ`,
`y_local = dx·sinθ − dy·cosθ`), then delete the table.

`build` configuration (all keys optional):

```json
{
  "build": {
    "auto": true,
    "concurrency": 2,
    "debounceMs": 1500,
    "keepRevisions": 5,
    "timeoutSeconds": 300,
    "stageTimeouts": { "step": 900 },
    "stages": ["erc", "drc", "bom", "schematic-pdf", "gerbers", "jlcpcb", "glb", "step", "quality"]
  }
}
```

- `auto`: when `false`, changed boards show `stale` until you trigger a build
  through `POST /api/build/run` or open a check.
- `concurrency`: 1 to 16.
- `debounceMs`: 100 to 60000. This is the quiet period after the last source
  change before a build starts. Viewer refreshes keep their 100 ms debounce.
- `keepRevisions`: at least 1.
- `timeoutSeconds`: applies to every stage except `step`, which defaults to
  900 s.
- `stageTimeouts`: per-stage overrides, keyed by stage id.
- `stages`: the enabled subset. Unknown keys and stage ids fail validation.

The Rust workbench's **BOM / Assembly CSV** tab previews BOM and placement CSVs
for the selected board, with artifact downloads and LCSC part links. Name files
with `bom`, `cpl`, `pos`, `position`, or `placement` as a filename token (for
example `fab/bom/module-bom.csv` or `fab/jlcpcb/CPL_module.csv`). Unrelated CSVs
such as carrier pinouts are not treated as BOMs. Quoted fields and multiline
notes are supported. Previews are limited to 500 rows and 2 MB; downloads
preserve the complete original file. The tab reloads on activation and every
10 seconds while visible, independently of schematic/PCB revision changes.

## Layout Quality Checks

A clean DRC is not a finished layout. The `quality` build stage checks the
board against a user preference profile (see `examples/pcb-profile.yaml`) and
shows the results in the **Layout quality** card on the Checks tab. The same
results are available from `GET /api/kicad/quality?project=<id>` and
`bin/kicad-pcb-rs quality --cwd <repo> [--project <id>] [--json]`. The stage
fails when any finding has `error` severity.

```json
{
  "qualityProfile": "docs/pcb-playbook/pcb-profile.yaml",
  "quality": {
    "kct": true,
    "pluginAudits": true,
    "disabledAudits": [],
    "includeDrcRules": false,
    "detectMistakes": true,
    "optimizeTraces": true,
    "severity": { "mistake.bypass_capacitor.bypass_capacitor_too_far_from_power_pin": "off" }
  },
  "projects": [{ "id": "board-a", "root": "boards/a", "qualityProfile": "profile.yaml" }]
}
```

- `qualityProfile` is relative to the repo root. `projects[].qualityProfile`
  is relative to the project root, may point anywhere inside the repo, and
  overrides the workspace profile. Editing the profile re-runs only the
  `quality` stage.
- **kct rules.** The stage runs `kct check --drc-only --format json`,
  `kct detect-mistakes --format json`, and `kct optimize-traces --dry-run` (on
  a scratch copy of the board). Every reported rule passes through. The one
  exception is kct rule families that duplicate native KiCad DRC (clearance,
  dimension, hole, edge, and similar), which are hidden unless
  `includeDrcRules` is set. New upstream rules appear after a kct upgrade
  without plugin changes. The kct version is part of the stage key.
- **Severity.** Precedence is `quality.severity[rule id or profile item]`,
  then the profile value for the rule's profile item, then the tool's
  default. Profile values map as follows: `error`, `forbid`, and `required`
  become error; `warn` becomes warning; `false` and `allow` turn the rule off.
  Rule ids map to profile items by substring. For example, `via_in_pad` maps
  to `vias.in_pad`, `silk_over*` to `silkscreen.over_vias_or_pads`, and
  `pin1*` to `silkscreen.pin1_dots`.
- **In-plugin audits** cover profile items kct lacks: `via_under_package`
  (vias inside the Fab body of QFN/DFN/BGA parts, per
  `routing.no_front_routing_under`), `off_angle_track` (non-0/45/90 segments),
  `orphan_via` (same-net copper on fewer than 2 layers; zones count by
  outline), `silk_reference_prefix`, `silk_text_size` (below
  `text_height_mm`/`text_thickness_mm`, or non-uniform), `silk_explanatory_text`,
  and `decoupling_distance` (IC pins on nets bridged to ground by a cap of at
  least 10 nF, measured to the nearest same-side cap pad against
  `decoupling.max_pin_distance_mm`). Each audit is retired automatically when
  kct reports an equivalent rule id (listed in `crates/server/src/audits.rs`).
  You can also disable audits individually (`disabledAudits`) or all at once
  (`pluginAudits: false`).
