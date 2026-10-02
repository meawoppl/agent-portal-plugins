# Vendored viewer runtime retirement

Status: **completed**. This document records the boundary reached after the
KiCanvas/OCCT runtime was removed and keeps the important deletion and parity
decisions for future maintainers.

## Current architecture

```text
 .kicad_pcb / .kicad_sch / libraries          KiCad board export
                   |                                  |
                   v                                  v
 +----------------------------------+      revision-keyed model.glb
 | Rust backend                     |                  |
 | kct schema + hierarchy/net data  |                  |
 | SCH/library -> vector Scene      |                  |
 | PCB -> typed render-ready model  |                  |
 | kicad-strokes -> text geometry   |                  |
 +----------------+-----------------+                  |
                  | JSON                               |
                  v                                    v
 +----------------------------------+      +--------------------------+
 | Yew/WASM generic 2D viewport     |      | Yew/WASM ModelView       |
 | vector-view draw/hit/input       |      | wasm-bindgen ES imports  |
 | layers, nets, props, selection   |      | Three + GLTF + Orbit     |
 +----------------------------------+      +--------------------------+
```

The backend is the sole source of KiCad interpretation. Schematic, symbol and
footprint views use the generic, renderer-independent `vector_view::Scene`
contract with layer, item, group and net ids, properties, bounds, and drawing
primitives. PCB uses the same scene contract, with backend-reified pad shapes,
layer ordering, properties and text strokes. The browser does not receive
KiCad sources for reparsing.

`kicad-strokes` converts schematic and library-preview text to `Prim::Strokes`
on the backend. Its
2015 CC0 glyph data is checksum-pinned, excludes CJK, and its original layout
implementation is measured against KiCad SVG plots.

Board 3D is a backend-produced GLB. `ModelView` imports upstream Three.js,
`GLTFLoader` and `OrbitControls` directly through `wasm-bindgen`; there is no
application JavaScript bridge. STEP is retained as a fabrication/download
artifact, not as an interactive browser CAD format.

## Retained browser assets

The complete allow-list is under `frontend/static/kicad-viewer/`:

- `three/three.module.js`;
- `three/addons/loaders/GLTFLoader.js`;
- `three/addons/controls/OrbitControls.js`;
- `three/addons/utils/BufferGeometryUtils.js`;
- `three/LICENSE`;
- the data-only `american-embedded-dark.json` palette and directory README.

The directory is approximately 1.6 MiB. It contains no WASM, first-party JS,
KiCad/STEP parser, worker, iframe shell or network-loaded decoder.

## What retired

The removed system used `runtime.html` and `runtime.js` as a postMessage shell.
It sent full KiCad sources to KiCanvas, restored opaque view/UI state, and
coordinated a large collection of JavaScript adapters. Library previews used
the same iframe. Model/STEP paths used separate JavaScript bridges and an OCCT
WASM worker.

That entire graph is gone:

- `ecad-viewer.js`, `parser.worker.js`, `glyph-full.js`;
- `retained-native-viewer.js`, `native-*`, `schematic-*`,
  `board-net-selection.js`, gesture/touch, properties and presentation code;
- `runtime.{html,js}`, `RuntimeFrame` and the `kicad-pcb-*` postMessage API;
- `3d-viewer.js`, board-model/model-* and cel-renderer;
- `step-*`, OCCT JS/WASM, Draco/Basis/KTX/EXR/zstd/meshopt support and remote
  runtime fetches;

## Gates that authorized deletion

The migration was deleted in dependency order only after the replacement path
covered:

- revision/project refresh, persistent per-project view state, resize and
  mouse/touch navigation;
- PCB layers/flip/pours, pads/tracks/arcs/vias/zones/drills, properties,
  selection and net highlight;
- schematic symbols/pins/fields, wires/buses/junctions/no-connects/labels,
  hierarchy-aware bounds, properties and net identity;
- direct symbol and footprint library previews without synthetic iframe
  documents;
- GLB framing, orbit/pan/zoom, revision replacement and GPU disposal;
- visible loading, empty, warning and error states;
- static import reachability and a licence/asset inventory.

## Supply-chain invariant

The expected final check is:

```text
frontend/static/kicad-viewer/**/*.js   upstream Three.js modules only
frontend/static/kicad-viewer/**/*.wasm none
browser KiCad/STEP parsers             none
remote runtime fetches                 none
iframe/postMessage viewer protocol     none
```

Any expansion requires an explicit design and licence review. In particular,
do not move source parsing into Rust/WASM in the browser, silently flatten
unsupported geometry, or convert viewer geometry back into KiCad edits.
