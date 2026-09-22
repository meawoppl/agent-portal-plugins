# Backplane Portal Plugin

This is the first test plugin for Agent Portal's plugin architecture. It wraps
Backplane-shaped PCB workflows as a Portal-hosted session surface plus agent
skills.

The plugin is deliberately thin:

- `agent-portal-plugin.toml` declares install, surface, commands, detection,
  skills, prompts, and MCP shape.
- `bin/backplane` is a portable smoke-test wrapper using Bash and Python's
  standard library.
- `skills/pcb-workflow/SKILL.md` teaches agents how to use the workbench.
- `prompts/session.md` is the session reminder text Portal can inject.

The wrapper does not vendor or replace the upstream Backplane application. It is
a contract test for the Portal side: install, doctor, open a docked HTTP
surface, detect KiCad/Gerber projects, and route PCB work through a domain skill.

## Try Locally

```console
backplane/bin/backplane doctor --json
backplane/bin/backplane serve --port 48888 --cwd /path/to/hardware/repo
```

Then open:

```text
http://127.0.0.1:48888/
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
