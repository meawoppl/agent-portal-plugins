# kicadmium agent notes

Single-binary Rust workbench, same shape as
https://github.com/meawoppl/single-binary-rust-website:

- `shared/` — serde API/event types used by both sides.
- `backend/` — axum server + CLI, binary `kicadmium`. Embeds `frontend/dist`
  with `memory-serve` (read from disk in debug builds, embedded in release).
- `frontend/` — Yew (trunk) workbench. `frontend/static/kicad-viewer/` holds the
  vendored third-party viewer runtimes (ecad-viewer, three.js, occt, gerber-view)
  and is copied to `dist/kicad-viewer/` by trunk.

Build order matters: `cd frontend && trunk build` before `cargo build -p backend`.

## Rules

- All first-party code is Rust. Vendored JS/WASM viewer runtimes stay vendored;
  do not hand-edit them except to refresh from upstream.
- kicadmium never downloads or bundles KiCad. It discovers `kicad-cli`
  (`KICADMIUM_KICAD_CLI`, `KICAD_CLI`, `PATH`, stock install paths).
- kicad-tools (`kct`, rjwalters/kicad-tools) is installed into a managed venv
  under `KICADMIUM_HOME` by `kicadmium setup --install-kicad-tools`.
- Mutating `kct` commands are design edits: clean tree or explicit branch only.
- Before pushing: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.
