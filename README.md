# Agent Portal Plugins

This repository is a collection of test and reference plugins for Agent Portal.
Each plugin lives in a subdirectory and contains its own
`agent-portal-plugin.toml` manifest.

The former `kicad-pcb` plugin has moved to the standalone, all-Rust
[kicadmium](https://github.com/meawoppl/kicadmium) repository. Its workbench,
viewers, skills, exports, quality checks, and automation tools now ship as one
embedded-web-UI binary. Agent Portal retains only its generic surface and
interaction/queue machinery.

Install Kicadmium from its own repository:

```console
agent-portal plugin install github:meawoppl/kicadmium
```

The installed checkout still materializes as one plugin directory:

```text
~/agent-portal-plugins/kicadmium
```

## Plugins

- [`verilog`](verilog/) - Icarus testbench runs, GTKWave-inspired waveform
  inspection, retained artifacts, and queued annotation feedback.
- [Kicadmium](https://github.com/meawoppl/kicadmium) - KiCad PCB/electronics
  workbench, now maintained independently of this reference-plugin collection.
