# Provenance

Kicadmium consolidates the standalone portions of the former Agent Portal
`kicad-pcb` plugin. KiCad itself and its installer/runtime are intentionally
excluded.

- Workbench server, quality checks, viewers, and workflow docs:
  `meawoppl/agent-portal-plugins`, `kicad-pcb` plugin.
- Browser viewer lineage and deterministic live-view conventions:
  `i2cjak/Backplane` (MIT).
- Agent-oriented automation conventions and optional `kct` integration:
  `rjwalters/kicad-tools` (MIT).
- Electronics workflow skills: `American-Embedded/kistack`, pinned revision
  recorded in `skills/kistack/kistack.bundle.json`.
- Read-only heuristic linter and independent evidence: `bp-test-reflection`
  handoff, retained in `docs/pcb-lint-independent-review/`. The `pcb-lint`
  crate it produced is now `kct lint` (`kct/src/lint/`).
- Viewer-specific upstream notices remain beside the embedded assets.

The original projects retain their own copyrights. See their license files
and the vendored notices before redistribution.
