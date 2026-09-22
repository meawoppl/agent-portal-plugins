# Backplane Portal Plugin

This plugin brings Backplane-shaped PCB workflows into Agent Portal as a
Portal-hosted session surface plus agent skills.

The plugin provides:

- `agent-portal-plugin.toml` declares install, surface, commands, detection,
  skills, and prompts.
- `bin/backplane` is a portable Python runtime with a tabbed PCB workbench.
- `static/kicad-viewer/` carries Backplane's bundled 3D/STEP viewer runtime
  assets.
- `skills/pcb-workflow/SKILL.md` teaches agents how to use the workbench.
- `prompts/session.md` is the session reminder text Portal can inject.

The workbench mirrors Backplane's KiCad panel shape: schematic, PCB, Gerbers,
3D, STEP, BOM, footprints/symbols, analysis, panelization, and checks. Native
DRC/ERC/export/3D generation use `kicad-cli` when available. Without KiCad, the
surface still detects project files and presents the workflow, but check/export
commands fail loudly instead of producing placeholder manufacturing output.

The plugin is intentionally script-first. Agents should use the documented CLI
commands and bundled skill instructions, with no additional integration setup.

## Try Locally

```console
backplane/bin/backplane doctor --json --cwd /path/to/hardware/repo
backplane/bin/backplane serve --port 48888 --cwd /path/to/hardware/repo
```

Then open:

```text
http://127.0.0.1:48888/
```

For production fabrication outputs, install KiCad or set `KICAD_CLI` /
`BACKPLANE_KICAD_CLI` to the desired executable:

```console
backplane/bin/backplane drc --json --cwd /path/to/hardware/repo
backplane/bin/backplane erc --json --cwd /path/to/hardware/repo
backplane/bin/backplane export jlcpcb --cwd /path/to/hardware/repo --out build/jlcpcb
```

## Example Repo Config

A PCB repository can suggest this plugin with:

```toml
# .portal/plugins.toml
schema_version = 1

[[suggested_plugins]]
name = "backplane"
source = "github:meawoppl/agent-portal-plugins//backplane"
reason = "This repo contains PCB design files and uses Backplane for review."
required = false

[suggested_plugins.config]
default_view = "board"
fabrication_output = "build/fab"
```
