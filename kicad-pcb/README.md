# KiCad PCB Portal Plugin

This plugin brings KiCad PCB workflows into Agent Portal as a
Portal-hosted session surface plus agent skills.

The plugin provides:

- `agent-portal-plugin.toml` declares install, surface, commands, detection,
  skills, and prompts.
- `bin/kicad-pcb` is a portable Python runtime with a tabbed PCB workbench.
- `static/kicad-viewer/` carries bundled 3D/STEP viewer runtime
  assets.
- `skills/pcb-workflow/SKILL.md` teaches agents how to use the workbench, and
  `skills/kistack/` vendors Backplane's KiStack electronics skill bundle.
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
kicad-pcb/bin/kicad-pcb doctor --json --cwd /path/to/hardware/repo
kicad-pcb/bin/kicad-pcb serve --port 48888 --cwd /path/to/hardware/repo
```

Then open:

```text
http://127.0.0.1:48888/
```

For production fabrication outputs, run setup once or set `KICAD_CLI` /
`KICAD_PCB_KICAD_CLI` or legacy `BACKPLANE_KICAD_CLI` to a desired executable:

```console
kicad-pcb/bin/kicad-pcb setup --install-kicad
kicad-pcb/bin/kicad-pcb drc --json --cwd /path/to/hardware/repo --project board-a
kicad-pcb/bin/kicad-pcb erc --json --cwd /path/to/hardware/repo --project board-a
kicad-pcb/bin/kicad-pcb export jlcpcb --cwd /path/to/hardware/repo --project board-a --out build/jlcpcb
```

## Rust Rewrite

The Rust rewrite lives beside the current Python runtime while it reaches full
parity:

```console
cd kicad-pcb
cargo run -p kicad-pcb-server -- doctor --json --cwd /path/to/hardware/repo
cargo run -p kicad-pcb-server -- serve --port 48888 --cwd /path/to/hardware/repo
```

The Rust server already covers the core read/check/export path and embeds the
viewer assets into the binary. The manifest still points at `bin/kicad-pcb`
until the Rust surface is visually verified against the Python runtime.

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
- `projects[].artifacts.gerbers`
- `projects[].artifacts.jlcpcb`
- top-level and project `libraries` for doctor warnings

The `manufacturer` object is surfaced through `doctor --json` for agents and
future exporter policy, but it does not alter generated files yet.
