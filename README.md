# Agent Portal Plugins

This repository is a collection of test and reference plugins for Agent Portal.
Each plugin lives in a subdirectory and contains its own
`agent-portal-plugin.toml` manifest.

Install any plugin from its subdirectory:

```console
agent-portal plugin install github:meawoppl/agent-portal-plugins//kicadmium
agent-portal plugin install github:meawoppl/agent-portal-plugins//verilog
agent-portal plugin install github:meawoppl/agent-portal-plugins//visilog
```

## Plugins

- [`verilog`](verilog/) - Icarus testbench runs, GTKWave-inspired waveform
  inspection, retained artifacts, and queued annotation feedback.
- [`visilog`](visilog/) - interactive design exploration with the native
  Visilog viewer: module hierarchy diagrams, live values, source inspection,
  stepping, breakpoints and JSON design graphs.
- [`kicadmium`](kicadmium/) - all-Rust KiCad PCB/electronics workbench: one
  binary with a browser workbench, `kct` (a native port of kicad-tools) and
  `kct lint`. It replaces the former `kicad-pcb` plugin and lived in
  `meawoppl/kicadmium` (now archived) before moving here with its full history.
