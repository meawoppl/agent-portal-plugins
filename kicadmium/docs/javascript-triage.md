# JavaScript triage

kicadmium's first-party code is Rust. The browser still runs JavaScript in
`frontend/static/kicad-viewer/`, the viewer runtime inherited from the kicad-pcb
plugin. This file tracks what remains, why, and the plan to shrink it.

Budget at the time of writing (tracked `.js` under `frontend/static/kicad-viewer/`):

| Group | Lines | Status |
|---|---|---|
| three.js core + used addons | 53,834 | **Keep** (vendored upstream; not reimplementing three.js) |
| KiCanvas bundle: `ecad-viewer.js`, `parser.worker.js`, `glyph-full.js` | 124,676 | **Replace** (T2, T3) |
| `3d-viewer.js` (vendored ecad-viewer 3D component) | 1,175 | Keep for now (thin wrapper over three) |
| First-party adapters | 4,901 | **Port to Rust/Yew** except a thin three bridge (T1) |
| `occt/occt-import-js.wasm` | (wasm) | **Keep** (C++ STEP import; no mature Rust STEP reader) |

## T0: dead three.js decoders (done)

KiCad's GLB export has no `extensionsUsed` and no images (checked on bp-test
boards), so the following were removed (~120k lines):

- `three/addons/libs/draco/` (two Emscripten Draco decoder builds)
- `three/addons/libs/basis/` (Basis transcoder)
- `three/addons/libs/ktx-parse.module.js`, `zstddec.module.js`, `fflate.module.js`

`3d-viewer.js` imports `DRACOLoader`, `KTX2Loader`, `MeshoptDecoder` and
`EXRLoader` statically. These are now small local stubs that fail with a clear
error if a model ever needs them. This also removes the bundle's remote fetches
of decoders from unpkg and EXR environment maps from storage.googleapis.com.
The viewer keeps the built-in neutral `RoomEnvironment`. Verified by browser
smoke: PCB, schematic and 3D (direction-led-tester and esp32-fpga-module) render
with no page errors.

## T1: first-party adapters (owner: kc-codex)

Import graph and classification. "three refs" counts direct uses of three.js.

| File | Lines | three refs | Imports | Plan |
|---|---|---|---|---|
| `runtime.js` + `runtime.html` | 230 + 295 | 0 | board-model, canvas-presentation, ecad-viewer, native-touch, properties-mobile, retained-native-viewer, schematic-sizing | Rust: postMessage protocol, state and mode switching |
| `retained-native-viewer.js` | 1,158 | 0 | board-net-selection, native-layer-cache, native-source-index, schematic-compatibility, schematic-net | Rust (drives KiCanvas until T2/T3 retire it) |
| `properties-mobile.js` | 569 | 0 | none | Rust/Yew properties panel |
| `board-net-selection.js` | 254 | 0 | none | Rust (net graph from `kct::schema::pcb`) |
| `schematic-net.js` | 253 | 0 | none | Rust (connectivity from `kct::schema`) |
| `native-layer-cache.js` | 179 | 0 | none | Rust |
| `canvas-presentation.js` | 147 | 0 | none | Rust |
| `schematic-compatibility.js` | 114 | 0 | none | Move into backend source normalization |
| `gesture-surface.js`, `native-touch.js` | 110 + 78 | 0 | gesture-surface | Rust (web-sys pointer events) |
| `native-source-index.js` | 71 | 0 | none | Rust |
| `schematic-sizing.js` | 65 | 0 | none | Rust |
| `orthographic-camera.js` | 27 | 0 | none | Fold into the three bridge |
| `board-model.js` | 508 | 22 | three, GLTFLoader, TrackballControls, cel-renderer, gesture-surface, model-* | Thin three bridge (typed command/event API) |
| `model-selection.js` | 485 | 3 | three | Split: picking stays in the bridge, selection state goes to Rust |
| `model-update.js` | 260 | 0 | three (module) | Split as above |
| `model-appearance.js` | 146 | 2 | three, BufferGeometryUtils | Thin three bridge |
| `cel-renderer.js` | 122 | 1 | three | Thin three bridge |
| `step-viewer.js`, `step-scene.js`, `step-orientation.js`, `step-worker.js` | 125 | 9 | three, occt | Thin three/OCCT bridge |

Target: only the three/OCCT bridge remains in JS, behind a small typed
command/event interface (load model, set camera, highlight, pick → event).

## T2: PCB view on Rust (owner: kc-claude)

Replace KiCanvas for the PCB view with pastebom's Rust/WASM Canvas2D viewer
(`crates/viewer`, `pcb-extract`), fed from `kct::schema::pcb`. Don't delete the
KiCanvas PCB paths until it matches on all of: pads, tracks, zones, text,
drills, pours/layer visibility, selection, net highlight, source identity
(revision), and touch.

Status: implemented behind the PCB tab's "Rust renderer" switch
(`localStorage["kicadmium:pcb-renderer"] = "rust"`, default KiCanvas).

- Data: `/api/kicad/pcbview` (`backend/src/pcb_view.rs`) builds
  `shared::pcb::PcbBoard` from `kct::schema::pcb` and the same `kct::sexp`
  tree, at the project's current source revision. Text is sent as the raw
  KiCad text plus position, rotation (after keep-upright), size, thickness,
  justification, mirror, italic/bold, line spacing, layer and visibility.
- View: `frontend/src/pcb_view/` (Canvas2D, Yew). Pad shapes, hit testing,
  pointer handling and settings persistence are derived from pastebom's
  viewer, which has no license; see `docs/third-party.md` before merging.
  They were copied in with attribution rather than taken as a crate
  dependency: pastebom's viewer is a binary built around the iBOM `PcbData`
  model, which merges KiCad layers into front/back silkscreen/fab and
  cannot express per-layer visibility. Layer order, palette and blending
  follow the KiCanvas view (bundled theme JSON).
- **Visual-metric divergence (text):** no KiCad font data is shipped (KiCad's
  newstroke font is GPL-2.0-or-later). The view draws text with Canvas2D
  `fillText` in the system monospace font, scaled so the cap height equals
  the KiCad text height and stretched to the KiCad width/height ratio, with
  stems thickened to the KiCad stroke width. It honours anchor, rotation,
  mirror, horizontal/vertical justification (KiCad's line-position metrics),
  multi-line spacing, italic and overbar/sub/superscript markup. Glyph
  shapes and advance widths differ from KiCad's stroke font, so text runs
  can be slightly longer or shorter than in KiCad or KiCanvas, and text
  bounding boxes are not KiCad-exact.
- Other deliberate differences: Beziers and arc segments in polygons are
  drawn (KiCanvas skips or flattens them). Footprint values and fab/drawing
  user text are toggles, off by default because KiCanvas never shows them.

## T3: schematic view on Rust (later)

A Rust schematic renderer over `kct::schema::{schematic,symbol,library,hierarchy}`,
with KiCad's newstroke font as Rust static data instead of `glyph-full.js`.
Required parity: hierarchy navigation, symbol graphics, text and fields, and
selection. Then `ecad-viewer.js`, `parser.worker.js` and `glyph-full.js` can
all be deleted.
