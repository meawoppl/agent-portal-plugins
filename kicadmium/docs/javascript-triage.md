# JavaScript triage

Status: **complete**. kicadmium's first-party browser code is Rust/WASM. The
old KiCanvas, OCCT, iframe runtime, browser KiCad parser, Newstroke JavaScript
table, and adapter modules have been removed.

## Current budget

`frontend/static/kicad-viewer/` is approximately 1.6 MiB and contains only:

| Asset | Purpose |
|---|---|
| `three/three.module.js` | Upstream Three.js renderer |
| `three/addons/loaders/GLTFLoader.js` | Load backend-produced board GLB |
| `three/addons/controls/OrbitControls.js` | Orbit, pan and zoom |
| `three/addons/utils/BufferGeometryUtils.js` | Upstream geometry utility imported by the retained loader path |
| `three/LICENSE` | Three.js MIT licence |
| `american-embedded-dark.json` | Data-only layer palette; CC BY attribution is in the directory README |
| `README.md` | Asset boundary and attribution |

There are no `.wasm` files, remote decoder fetches, first-party JavaScript
bridges, browser-side STEP parsers, or browser-side KiCad parsers in that tree.

## Completed replacement

KiCad source is parsed once on the Rust backend. Schematic, symbol and footprint
data are reified into the generic `vector_view::Scene` contract: layers, stable
item/group/net ids, properties, bounds and drawing primitives. PCB uses that
same contract, including backend-reified pad shapes, layer paint order and
Newstroke text geometry. Yew/WASM owns Canvas2D drawing, hit testing, layer
visibility, selection/highlighting and pointer/touch navigation through one
shared scene host.

Board 3D uses the revision-keyed `/api/kicad/model.glb` artifact. The Yew
`ModelView` binds directly to the retained Three.js ES modules through
`wasm-bindgen`; Rust owns lifecycle and UI state. STEP remains an export or
download artifact and is not parsed in the browser.

Text geometry comes from the `kicad-strokes` crate. Its glyph table is generated
from the pinned 2015 CC0 Newstroke release, never from KiCad or KiCanvas. Its
layout was measured against `kicad-cli 10.0.6` PCB and schematic SVG plots;
fixture points agree within 0.0002 mm (the regression threshold is 0.001 mm).
See [Newstroke provenance](newstroke-provenance.md).

## Removed inventory

The completed migration removed:

- KiCanvas `ecad-viewer.js`, `parser.worker.js` and `glyph-full.js`;
- `runtime.html`, `runtime.js`, the iframe/postMessage protocol and
  `RuntimeFrame`;
- all native-viewer, selection, layer-cache, source-index, gesture, sizing,
  properties and presentation adapters;
- the old `3d-viewer.js`, board-model/model-* and cel-renderer bridge;
- `step-*`, OCCT JavaScript/WASM and all decoder/transcoder bundles.

The historical deletion contract and parity gates are retained in
[Vendored viewer runtime retirement](runtime-retirement.md).

## Boundary going forward

- First-party UI and geometry code remains Rust.
- KiCad and STEP source formats are never parsed in the browser.
- Reified geometry contracts stay renderer-independent; backend subject ids may
  support review or explicit edit requests, but rendering remains read-only.
- New browser JavaScript/WASM assets require an explicit inventory, licence and
  reason. Reimplementing Three.js, GLTF parsing or WebGL is a non-goal.
