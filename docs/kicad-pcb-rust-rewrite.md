# KiCad PCB Plugin Rust Rewrite Plan

## Goal

Replace the current `kicad-pcb/bin/kicad-pcb` Python HTTP glue with a Rust
single-binary plugin that follows the shape of
`meawoppl/single-binary-rust-website`: typed shared data, Axum routes, a
Yew/Trunk frontend embedded into the server binary, and a narrow tool-runner
boundary for KiCad/EDA commands.

The rename from `backplane` to `kicad-pcb` is the first migration step. The
viewer still contains Backplane-derived browser assets internally; those names
should be treated as viewer protocol details until the viewer itself is split
or repackaged.

## Target Shape

```text
kicad-pcb/
  agent-portal-plugin.toml
  Cargo.toml
  bin/
    kicad-pcb                # released Rust binary, or dev wrapper while building
    backplane                # temporary compatibility wrapper
  crates/
    shared/                  # serde API types, no native-only deps
    server/                  # Axum app, tool runners, cache, static serving
    frontend/                # Yew app compiled by Trunk
  frontend/
    index.html
    Trunk.toml
    style.css
  static/
    kicad-viewer/            # current vendored viewer assets, later replaceable
  skills/
  prompts/
```

The production artifact should be one native executable plus the plugin
manifest/skills/prompts. Frontend assets are embedded in the executable at
build time with `memory-serve`, matching the skeleton convention.

## Rust Server

Use Axum with one `AppState`:

```rust
struct AppState {
    plugin_dir: PathBuf,
    cwd: PathBuf,
    session_id: Option<String>,
    cache_root: PathBuf,
    toolchain: Toolchain,
    viewer_state: Arc<RwLock<ViewerState>>,
}
```

Routes:

- `GET /healthz`
- `GET /api/project`
- `GET /api/kicad/manifest`
- `GET /api/kicad/revision`
- `GET /api/kicad/sources`
- `GET /api/kicad/model.glb`
- `GET /api/kicad/drc`
- `GET /api/kicad/erc`
- `GET /api/kicad/file?path=...&download=1`
- future: `GET /api/kicad/bom`, `GET /api/kicad/gerber/render`, `GET /api/kicad/step`

All responses get shared serde structs. Avoid ad hoc JSON maps except for raw
KiCad reports that are intentionally passed through.

## Frontend

Port the inline HTML/JS to Yew:

- Tab state as Yew state.
- A typed API client using `gloo_net`.
- Revision polling every 2.5s plus visibility-return refresh.
- Components:
  - `OverviewTab`
  - `NativeViewerTab` for schematic/PCB iframe handoff
  - `Model3dTab`
  - `BomTab`
  - `ChecksTab`
  - `GerberTab`
  - `LibrariesTab`

Keep the existing `/kicad-viewer/runtime.html` iframe protocol initially:
`backplane-snapshot`, `backplane-runtime-ready`, etc. Rename that protocol only
when the vendored viewer is owned as a package and all assets are updated
together.

## Tool Runner Boundary

Make every external call go through a small typed runner:

```rust
struct CommandSpec {
    program: PathBuf,
    args: Vec<OsString>,
    cwd: PathBuf,
    timeout: Duration,
}

struct CommandOutput {
    status: i32,
    stdout: String,
    stderr: String,
    elapsed_ms: u64,
}
```

Tool modules:

- `tools::kicad::doctor`
- `tools::kicad::drc` with `--schematic-parity` when supported
- `tools::kicad::erc`
- `tools::kicad::export_gerbers`
- `tools::kicad::export_jlcpcb`
- `tools::kicad::export_glb`

Exports should continue to stage into a temp directory and zip only generated
whitelisted outputs. Never zip arbitrary pre-existing destination contents.

## Cache And Refresh

Keep plugin-managed state under the plugin install/home root:

```text
.portal/cache/<repo-hash>/
  <source-revision>.glb
  checks/
  gerber-renders/
.runtime/
  kicad/
  python/
  kikit/
```

Revision rules:

- active KiCad sources exclude `tmp/`, `build/`, `dist/`, backups, and backup
  suffixes;
- artifact discovery keeps `fab/` and `build/` outputs visible for downloads;
- source revision includes file paths, mtimes, and content digests where cheap
  enough;
- `/api/kicad/revision` stays cheap;
- `/api/kicad/sources` refreshes warmed state when revision changes.

## Containerized Plugins

Containers make sense as an optional plugin runtime mode for heavy toolchains,
especially KiCad, KiKit, Python, and future renderers. They should not be the
only plugin model.

Recommended model:

- **native plugin**: default for lightweight tools and same-host UX;
- **managed runtime plugin**: downloads tools into plugin `.runtime/`;
- **container plugin**: declares an OCI image and mounts only approved paths.

Manifest sketch:

```toml
[runtime]
mode = "container" # native | managed | container
image = "ghcr.io/meawoppl/kicad-pcb-plugin:10.0.6"
pull_policy = "if-missing"

[[runtime.mounts]]
kind = "project"
target = "/work"
access = "rw"

[[runtime.mounts]]
kind = "plugin-cache"
target = "/plugin/.portal"
access = "rw"
```

Container advantages:

- KiCad/Python/KiKit/tool versions become reproducible.
- No pollution of project repos or host package manager.
- Linux hosts can run full toolchains without AppImage/PPA branches.
- CI and local plugin behavior converge.

Container drawbacks:

- macOS/Windows require Docker/Podman availability.
- GPU/WebGL/browser viewer still runs in the user browser, not the container.
- File ownership and path mapping must be explicit.
- Startup latency and image size can be large.

For `kicad-pcb`, use native/managed Rust first, but design the tool runner so
the backend can switch from `Command::new("kicad-cli")` to
`docker run ... kicad-cli` without changing API handlers.

## Migration PRs

1. **Rename plugin to `kicad-pcb`**
   - Done in `agent-portal-plugins` commit `97b1f4a`.
   - Keep `bin/backplane` compatibility wrapper temporarily.

2. **Introduce Rust workspace skeleton**
   - Add `Cargo.toml`, `crates/shared`, `crates/server`, `frontend`.
   - Add CI for `cargo fmt`, `clippy`, tests, and Trunk build.
   - Keep Python binary as default manifest start command.

3. **Port read-only APIs**
   - `healthz`, manifest, file detection, source revision, sources.
   - Add shared roundtrip tests for response structs.

4. **Embed frontend**
   - Move inline HTML/JS into Yew.
   - Serve with `memory-serve`.
   - Preserve current iframe viewer protocol.

5. **Port KiCad tool commands**
   - DRC/ERC, GLB export, Gerber export, JLCPCB export.
   - Golden tests around bp-test-style fixture outputs.

6. **Flip manifest to Rust binary**
   - `start = "bin/kicad-pcb serve ..."` now points at Rust.
   - Keep Python wrapper as emergency fallback for one release.

7. **Container runtime support**
   - Add manifest schema for container runtime.
   - Implement a container-backed `ToolRunner`.
   - Publish `ghcr.io/.../kicad-pcb-plugin` image.

8. **Retire compatibility names**
   - Remove `bin/backplane` wrapper.
   - Migrate docs/examples from `.backplane.json` fallback to `.kicad-pcb.json`.
   - Keep viewer protocol names until viewer package split.

## Acceptance

- `agent-portal plugin open kicad-pcb` opens the same pane shape as today.
- `doctor`, `drc`, `erc`, `export gerbers`, and `export jlcpcb` match current
  behavior.
- Live refresh still works without reload.
- DRC includes schematic parity.
- No runtime cache/tool installs are written into the design repo.
- Container mode can run checks/exports on a clean Linux host with only
  Docker/Podman plus Agent Portal installed.
