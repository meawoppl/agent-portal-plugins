# Agent Portal Plugins

This repository is a collection of test and reference plugins for Agent Portal.
Each plugin lives in a subdirectory and contains its own
`agent-portal-plugin.toml` manifest.

Install syntax is intentionally subdirectory-aware:

```console
agent-portal plugin install github:meawoppl/agent-portal-plugins//kicad-pcb
```

The installed checkout still materializes as one plugin directory:

```text
~/agent-portal-plugins/kicad-pcb
```

## Plugins

- [`kicad-pcb`](kicad-pcb/) - KiCad PCB/electronics workflow plugin for the
  Portal plugin architecture.
