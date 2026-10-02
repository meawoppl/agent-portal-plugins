# Vendored viewer runtime retirement

Status: implementation plan and deletion contract. This document describes the
tree at `d36692e`; it does not authorize deleting a runtime until its parity gate
passes.

## Target boundary

The browser should receive renderer-independent, revision-keyed geometry from
the Rust backend. Yew/WASM owns view state, input, selection, properties,
layer/net visibility, capture and the workbench UI.

The only retained JavaScript implementation is upstream Three.js and the
minimum upstream Three.js modules required to load/render a GLB. A small,
typed, deliberately boring JavaScript bridge may call Three.js because
`wasm-bindgen` cannot conveniently express its class-heavy API; that bridge is
not allowed to parse KiCad, own application state, create application UI, or
implement geometry algorithms. OCCT, KiCanvas, browser-side KiCad parsing and
the present first-party viewer adapters all retire.

```text
 .kicad_sch/.kicad_pcb/.step/.wrl/.glb
                 |
                 v
 +--------------------------------------------+
 | Rust backend                               |
 | kct parsers + hierarchy/net resolution     |
 | PCB/SCH/STEP -> normalized scene geometry  |
 | stable subject ids, nets, layers, bounds   |
 +----------------------+---------------------+
                        | JSON / GLB, revision keyed
                        v
 +--------------------------------------------+
 | Yew/WASM                                   |
 | 2D canvas/SVG view | properties | gestures |
 | selection | crossprobe | layers | capture  |
 +----------------------+---------------------+
                        | typed calls only for 3D
                        v
 +--------------------------------------------+
 | thin JS bridge -> upstream Three.js         |
 | GLTFLoader, camera, controls, ray casting   |
 +--------------------------------------------+
```

This removes a duplicated source of truth: browser code must not reconstruct
nets, transforms, layers, or hierarchy already available to `kct`.

## Current entry points and message contract

There are two Rust callers of `runtime.html`.

### `frontend/src/viewer.rs`

The PCB/schematic/model workbench tabs fetch `/api/kicad/sources`, then post:

```text
kicad-pcb-snapshot, native
  kind="native"
  context="pcb" | "schematic"
  active: bool
  revision: string
  sources: [{ filename, content }]
  polygonPours: bool
  viewState?: opaque object
  uiState?: opaque object

kicad-pcb-snapshot, model
  kind="model"
  context="model"
  active: bool
  url=/api/kicad/model.glb?rev=...&project=...
  viewState?: opaque object
  uiState?: opaque object
```

It consumes `kicad-pcb-runtime-ready` and `kicad-pcb-view-state`. View state is
stored under `localStorage["kicadmium:viewer:<context>"]`. The current component
does **not** consume selection, crossprobe, layer, probe-result or key messages,
although the runtime emits them. PCB can bypass the iframe through the native
Rust `PcbView`; this choice is stored in `kicadmium:pcb-renderer` and is still
opt-in at this revision.

### `frontend/src/runtime_frame.rs` and `frontend/src/library.rs`

`RuntimeFrame` accepts an arbitrary snapshot and forwards all non-ready replies
to an optional callback. The library modal uses it for:

* model: `{type, kind:"model", subject:"part", url, active}`;
* symbol: a synthetic `.kicad_sch` source with `context:"schematic"`;
* footprint: a synthetic `.kicad_pcb` source with `context:"pcb"`.

No library-modal callback currently consumes runtime replies.

### Runtime inputs accepted but not currently sent by these callers

`runtime.js` additionally accepts:

* `kicad-pcb-view-options { polygonPours }`;
* snapshot `layerVisibility: {layer: bool}`;
* snapshot `probe { id, value, ... }`;
* snapshot `netHighlight { id, targetContext?, clear?, ... }`;
* snapshot `kind:"step"`, which selects the STEP/OCCT path.

The runtime can emit:

* `kicad-pcb-selection {selection,userInitiated}`;
* `kicad-pcb-crossprobe {selection,userInitiated}`;
* `kicad-pcb-layers {layers}`;
* `kicad-pcb-probe-result {found,value}`;
* `kicad-pcb-native-key {key}`;
* `kicad-pcb-view-state {context,view,ui}`.

It also exposes the untyped global `window.KicadViewerCapture(): Promise<data
URL|null>`. All of these must become typed Rust component props/callbacks or a
typed capture handle before `runtime.html` is deleted. Do not preserve the
postMessage protocol merely for compatibility inside this repository.

## Exact current import and call graph

```text
viewer.rs --------------------+
library.rs -> RuntimeFrame ---+-> runtime.html -> runtime.js
                                                |
                 +------------------------------+--------------------+
                 |                                                   |
                 | native snapshot                                   | model/step snapshot
                 v                                                   v
            ecad-viewer.js                                  board-model.js
             |      |                                        | | | | | | |
             |      +-> parser.worker.js                     | | | | | | +-> step-viewer.js
             +--------> glyph-full.js                        | | | | | |      |-> step-worker.js
             +--------> 3d-viewer.js (KiCanvas 3D path)      | | | | | |      |   -> occt-import-js
                                                             | | | | | |      |   -> OCCT WASM
 retained-native-viewer.js ---------------------------------+ | | | | |      +-> step-scene.js
   |-> native-source-index.js                                 | | | | | |          -> step-orientation.js
   |-> native-layer-cache.js                                  | | | | | |
   |-> schematic-compatibility.js                             | | | | | +-> model-selection.js
   |-> schematic-net.js                                       | | | | +---> orthographic-camera.js
   +-> board-net-selection.js                                 | | | +-----> model-update.js
                                                             | | +-------> cel-renderer.js
 runtime.js adapters:                                        | +---------> model-appearance.js
   |-> canvas-presentation.js                                +-----------> Three.js
   |-> schematic-sizing.js
   |-> native-touch.js -> gesture-surface.js
   +-> properties-mobile.js

 board-model/model-* -> Three.js core, TrackballControls, GLTFLoader,
                        BufferGeometryUtils
 3d-viewer.js -> Three.js core and several add-ons (legacy KiCanvas path)
```

`runtime.js` serializes all replacements through `chain`, retains one native
viewer and one model viewer per iframe, restores opaque state after replacement,
and schedules view-state reports from pointer/wheel/key/touch events.

## Backend geometry contracts

### Already present: PCB

`GET /api/kicad/pcbview?project=...` is the desired pattern. `backend/src/pcb_view.rs`
parses through `kct::schema::pcb` plus the same S-expression tree and returns
`shared::pcb::PcbViewResponse`. It resolves footprint transforms and emits:

* revision, filename and board bounds;
* ordered layer table and net table;
* footprints, pads, drills and properties;
* tracks, arcs, vias and filled zones;
* board/footprint graphics and text;
* stable layer/net indices and warnings.

The Rust PCB view already covers geometry drawing, layer visibility, pours,
flip, selection/properties, net highlight, mouse/touch input and per-project
camera state. It should become unconditional before native PCB code is removed.

### Required: schematic

Add `GET /api/kicad/schematic-view?project=...` returning a cached,
revision-keyed contract built from `kct::schema::{schematic,hierarchy,...}` and
the native netlist connectivity code. The contract must contain:

* hierarchy pages with stable paths, parent/child sheet links and page bounds;
* fully transformed symbol graphics, fields and pins (unit/convert/mirror/angle
  already applied), pin electrical type/shape/name/number;
* wires, buses, junctions, no-connects, labels and sheet pins;
* resolved net identity on pins/wires/labels and stable subject ids;
* draw order, stroke/fill, visibility, text metrics and paper/title-block data;
* library symbol provenance and warnings for unsupported geometry.

This replaces `parser.worker.js`, schematic hydration, sizing, net union-find and
source indexing in one move. It must work for project hierarchies and synthetic
library-symbol previews; the latter should use a dedicated endpoint accepting a
library-part id, not raw source text in an iframe.

### Required: 3D/STEP

Keep `/api/kicad/model.glb`, because `kicad-cli` already exports board geometry
to a browser-ready, revision-keyed GLB. Normalize all remaining formats on the
backend:

* STEP library preview: tessellate to indexed triangle meshes in Rust/backend
  (or convert to GLB as a build artifact), including assembly tree, stable part
  ids, names, colors, transforms and bounds;
* WRL/other KiCad model inputs: use the existing KiCad GLB export path rather
  than adding browser parsers;
* model selection metadata: emit a small JSON sidecar keyed to mesh/node ids if
  GLB extras are insufficient.

The browser bridge receives only GLB URL/bytes plus typed scene commands. This
deletes OCCT JS/WASM and the STEP worker. Three.js owns scene/camera/material
objects; Rust owns selection state, UI tree, opacity, layer visibility and
capture commands.

### Existing sources that cease to feed viewers

`GET /api/kicad/sources` (`SourcesResponse {revision,sources,warmed_at_ms}`)
remains useful for diagnostics but must no longer send full KiCad source text to
the browser renderer. `/api/kicad/revision` and `/ws/events` continue to drive
refresh. `/api/kicad/model.glb` remains. `/api/kicad/file` must not become a
viewer parser escape hatch.

## File-by-file disposition

### Retain as upstream Three.js assets

| Path | Disposition |
|---|---|
| `three/three.module.js` | Keep, pinned with upstream license. |
| `three/addons/loaders/GLTFLoader.js` | Keep while model GLB is loaded by Three.js. |
| `three/addons/controls/OrbitControls.js` | Keep only if selected for the final camera; otherwise delete. |
| `three/addons/controls/TrackballControls.js` | Keep only if selected for the final camera; do not ship two control systems. |
| `three/addons/utils/BufferGeometryUtils.js` | Keep only if the thin bridge demonstrably needs it. Prefer backend-prepared meshes. |
| `three/LICENSE` | Always keep while any Three.js code remains. |

### Delete immediately after import verification

These are loud-failure compatibility stubs or no longer useful with normalized
GLB and must not define the retained boundary:

| Path | Reason |
|---|---|
| `three/addons/loaders/{DRACOLoader,KTX2Loader,EXRLoader}.js` | Disabled optional formats; KiCad GLB uses none. |
| `three/addons/libs/meshopt_decoder.module.js` | Disabled meshopt path. |
| `three/addons/environments/RoomEnvironment.js` | Application appearance, replace with bridge lighting. |
| `three/addons/utils/WorkerPool.js` | Only supports retired loader paths. |

### Replace with Rust/Yew plus thin Three bridge

| Path | Rust owner / replacement |
|---|---|
| `board-model.js` | Yew `ModelView` state; tiny bridge creates scene, loads GLB, raycasts and renders. |
| `model-selection.js` | Rust scene-tree model, selection/properties UI and opacity state; bridge applies mesh material commands. |
| `model-update.js` | Revision-keyed GLB swap; backend stable ids; bridge atomically replaces/disposes scene. No browser geometry diff. |
| `model-appearance.js` | Backend GLB materials plus Rust theme settings; bridge maps typed appearance commands. |
| `cel-renderer.js` | Optional bridge shader/pass. Delete unless an explicit UI feature and screenshot gate require it. |
| `orthographic-camera.js` | Rust camera math/state; bridge applies projection matrices. |
| `step-scene.js`, `step-orientation.js` | Backend tessellation applies canonical coordinates/transforms. |
| `step-viewer.js`, `step-worker.js` | Backend STEP-to-mesh/GLB endpoint and ordinary `ModelView`. |
| `occt/*` | Delete once STEP parity gate passes; browser no longer parses STEP. |
| `3d-viewer.js` | Delete with the KiCanvas 3D path; do not maintain two Three front ends. |

### Replace with Rust 2D viewers

| Path | Rust owner / replacement |
|---|---|
| `ecad-viewer.js` | Native PCB and schematic components consuming normalized geometry. |
| `parser.worker.js` | Backend `kct` parsers and caches. |
| `glyph-full.js` | CC0-pinned Newstroke Rust data if needed; never import the KiCanvas/KiCad copy. System-font fallback remains valid. |
| `retained-native-viewer.js` | Yew component lifecycle and revision fetch effects. |
| `native-source-index.js` | Stable backend subject ids and typed indexes constructed in Rust. |
| `native-layer-cache.js` | Rust render display lists keyed by revision/layer; profile before adding a cache. |
| `board-net-selection.js` | `pcb-view` Rust geometry/hit testing and net index. |
| `schematic-net.js` | Backend native connectivity, shipped as resolved net ids. |
| `schematic-compatibility.js` | Backend typed symbol/pin expansion; unsupported data becomes warnings. |
| `canvas-presentation.js` | Yew theme/layer controls and Rust canvas renderer. |
| `schematic-sizing.js` | Backend bounds plus Rust responsive layout. |
| `gesture-surface.js`, `native-touch.js` | Shared Rust input state machine used by both 2D viewers. |
| `properties-mobile.js` | Normal Yew properties component and CSS. |
| `american-embedded-dark.json` | Convert to a typed Rust/theme asset or retain as data; it is not runtime code. Preserve CC BY attribution. |

### Delete last: iframe protocol shell

| Path | Replacement |
|---|---|
| `runtime.js`, `runtime.html` | Direct Yew components and typed callbacks. |
| `frontend/src/runtime_frame.rs` | Direct `PcbView`, `SchematicView` or `ModelView`. |
| iframe branch in `frontend/src/viewer.rs` | A typed view enum and ordinary component props. |

`UPSTREAM-LICENSE.txt`, `ecad-viewer.manifest.json` and the current viewer README
can be deleted only when their covered vendored works are gone. Preserve
licenses/attributions for anything retained.

## Implementation and deletion order

Each numbered step is independently shippable. Delete files only in the same
commit that removes their final imports.

1. **Freeze typed view contracts.** Replace JSON `Value` snapshot construction
   with shared Rust structs/enums for context, camera, layer visibility,
   selection, probe, net highlight and capture. Add contract serialization tests.
2. **Make Rust PCB default.** Move reusable PCB geometry/render/input into the
   authorized `pastebom.com/crates/pcb-view` library, consume a pinned revision,
   run parity, then remove the KiCanvas PCB toggle/path. Delete
   `board-net-selection.js` when its last test moves to Rust.
3. **Backend-normalize schematic.** Implement and fixture-test the schematic
   endpoint, including hierarchy and net ids. A server fixture must serialize
   identically twice for the same revision.
4. **Ship native schematic view behind a toggle.** Implement drawing, hit test,
   selection, net highlight, layers/pages, properties, input, camera persistence
   and capture. Exercise project and library-symbol use cases.
5. **Make native schematic default.** Run the parity matrix below, then delete
   `ecad-viewer.js`, `parser.worker.js`, `retained-native-viewer.js`,
   `schematic-*`, `native-*`, `canvas-presentation.js`, `properties-mobile.js`,
   `gesture-surface.js`, `native-touch.js` and (when no consumer remains)
   `glyph-full.js`.
6. **Define the Three bridge.** Its entire public surface should be equivalent
   to `mount`, `load_glb`, `resize`, `set_camera`, `set_visible`, `select`,
   `raycast`, `capture`, `dispose`. Rust owns all ids and state. Add a test page
   that detects leaked canvases/workers/RAF loops over repeated mounts.
7. **Replace board model view.** Use `ModelView` plus the bridge for board GLB;
   move layer/part tree and properties into Yew. Delete `board-model.js` and its
   model/camera/appearance helpers after parity.
8. **Move STEP conversion server-side.** Preserve assembly tree and selection
   ids, then point library STEP preview at `ModelView`. Delete `step-*` and
   `occt/*` after parity and confirm the release artifact no longer contains
   OCCT WASM.
9. **Delete the iframe shell.** Convert library symbol/footprint/model previews
   to direct components, remove `RuntimeFrame`, `runtime.{html,js}` and all
   `kicad-pcb-*` postMessage handling.
10. **Prune Three add-ons.** Keep one controls module and only imports reachable
    from the final bridge. Generate a tracked runtime inventory and fail CI if a
    new non-Three `.js`/`.wasm` asset appears without an allow-list entry.

## Feature parity gates

### Common lifecycle

* Project switch and revision save refresh without a full page reload.
* Inactive tabs stop rendering; remount and repeated revision changes leak no
  iframe, worker, WebGL context, event listener or animation frame.
* Camera/view and UI state persist independently per project and context.
* Resize, device-pixel-ratio changes, narrow mobile layout and dark theme work.
* Loading, empty, unsupported-geometry and error states are visible and honest.
* Capture returns a nonblank image at the displayed resolution.
* Keyboard actions ignore editable controls and are represented in Yew help.

### PCB 2D

* Front/back/inner copper, silk, mask, paste, fab, user and Edge.Cuts layers.
* Pads: circle, rect, oval, roundrect, trapezoid, chamfer, custom; rotation,
  offset, front/back; round/oval drills and NPTH.
* Segments, arcs, vias and layer spans; zone polygons/holes/stroked legacy fills;
  pours toggle; graphics, Béziers, circles, polygons and visible/hidden text.
* Layer visibility and flip; selection precedence; footprint/pad/track/via/zone
  properties; net highlight and clear; pan/zoom/pinch/tap.
* Render/hit-test fixtures include dense, rotated, flipped and multilayer boards.

### Schematic 2D

* Root and hierarchical sheets; symbol unit/convert variants and mirrors.
* Pins and all pin shapes/types; fields, labels (local/global/hierarchical),
  wires, buses, junctions, no-connects, sheet pins and power symbols.
* Native connectivity agrees with `kct netlist` for named and unnamed nets;
  selecting a pin/wire/label highlights the same resolved net across the page.
* Page/title block, bounds/fit, text markup/Newstroke policy, hidden fields,
  properties, crossprobe and library-symbol preview.

### Board GLB and STEP 3D

* GLB board colors/transparency, tracks, pads, silk, mask and component models;
  framing, orbit/pan/zoom, resize, background and capture.
* Repeated revision updates dispose old GPU resources.
* STEP assembly hierarchy, names, transforms, colors, selection, isolate,
  visibility and opacity; canonical orientation and stable fit.
* Raycast selection maps to stable Rust ids and Yew properties.
* Screenshot comparisons cover a board with substituted models and at least one
  nested/multibody STEP assembly.

### Deletion and supply-chain gate

After the last step:

```text
frontend/static/kicad-viewer/**/*.js   only upstream Three.js + approved thin bridge
frontend/static/kicad-viewer/**/*.wasm none
browser KiCad/STEP parsers              none
remote runtime fetches                  none
iframe/postMessage viewer protocol      none
```

CI must run `cargo fmt --check`, workspace Clippy/tests, WASM Clippy/build, the
browser parity smoke, a static import reachability check and a license/asset
inventory. The final bundle-size report is a gate, not an anecdote.

## Non-goals and safeguards

* Do not reimplement Three.js, GLTF parsing or WebGL/WebGPU abstractions.
* Do not move KiCad parsing from one browser language to Rust/WASM; parse once
  on the backend and transmit normalized geometry.
* Do not silently flatten unsupported geometry. Return structured warnings and
  keep coverage counters in endpoint responses.
* Do not auto-convert normalized viewer geometry back into KiCad edits. Viewer
  subject ids may enqueue an explicit edit request, but rendering remains
  read-only.
* Do not couple shared geometry to Canvas, SVG, Three.js or Yew types. It is an
  API model usable by tests, alternative renderers and pastebom.
