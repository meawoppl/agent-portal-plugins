# Agent Portal Plugins

This repository is a collection of test and reference plugins for Agent Portal.
Each plugin lives in a subdirectory and contains its own
`agent-portal-plugin.toml` manifest.

Install syntax is intentionally subdirectory-aware:

```console
agent-portal plugin install github:meawoppl/agent-portal-plugins//backplane
```

The installed checkout still materializes as one plugin directory:

```text
~/agent-portal-plugins/backplane
```

## Plugins

- [`backplane`](backplane/) - PCB/electronics workflow test plugin for the
  Portal plugin architecture.
