# Third-party code in kicadmium

This file records the viewer-related code and data in kicadmium that is not
original to it. Other vendored skills and ports retain their notices beside the
corresponding source.

## Three.js

The only JavaScript implementation retained by the workbench is upstream
Three.js under its MIT licence:

- `frontend/static/kicad-viewer/three/three.module.js`;
- `three/addons/loaders/GLTFLoader.js`;
- `three/addons/controls/OrbitControls.js`;
- `three/addons/utils/BufferGeometryUtils.js`;
- `three/LICENSE`.

Rust/WASM imports these modules directly. No first-party JavaScript bridge,
remote decoder, browser KiCad parser or browser STEP parser remains.

## American Embedded dark palette

`frontend/static/kicad-viewer/american-embedded-dark.json` is a data-only layer
palette. Its attribution and CC BY 4.0 notice are in the adjacent README.

## pastebom `vector-view`

- Source: <https://github.com/meawoppl/pastebom.com>, crate `vector-view`.
- The exact revision is pinned in the workspace `Cargo.toml`/`Cargo.lock`.
- Author/rightsholder: meawoppl, the same author as kicadmium, who explicitly
  authorized this use and the earlier extraction work under kicadmium's MIT
  licence.

`vector-view` is the renderer-independent Rust scene model plus native/Canvas2D
view, hit-test and input engine. It was extracted from pastebom's viewer and
generalized from its front/back iBOM model to layer-aware PCB, schematic,
symbol, footprint and Gerber geometry. kicadmium's backend produces
`vector_view::Scene` for schematic and library views.

## pastebom-derived PCB Canvas renderer

The current PCB component predates `vector-view` and remains an authorized,
layer-aware adaptation of pastebom's `crates/viewer` at revision
`91ee2d7c9bad66bf85bec006f63d95d0f26c11c2`. The same author/rightsholder
explicitly authorized these derived portions under kicadmium's MIT licence.
Provenance headers remain in `frontend/src/pcb_view/`.

The derived algorithms cover pad/drill outline construction, transforms and
distance tests; prioritized hit testing; fit/pan/zoom math; Canvas2D path
batching and highlighted-net rendering; pointer/pinch handling; and persisted
layer/view settings. They were adapted from pastebom's merged front/back iBOM
model to kicadmium's all-layer `shared::pcb` contract. Backend conversion,
KiCad layer ordering/theme, text handling, properties UI and the layer-aware
extensions are original kicadmium work.

## NewStroke stroke font (`kicad-strokes`)

- **What:** `kicad-strokes/src/glyphs.rs`, the NewStroke glyph table
  (11,232 entries, U+0020–U+2BFF) used to turn schematic and library-preview
  text into backend-reified stroke polylines.
- **Attribution:** NewStroke by Vladimir Uryvaev (vovanium), CC0 1.0, release
  2015-11-19, <https://vovanium.ru/sledy/newstroke/en>.
- **Source:** the author's release tarball
  <https://vovanium.ru/_media/sledy/newstroke/newstroke-font.tgz>, SHA-256
  `dba1c334834e21bdbd15395dc610ecc2bf2eb6675ba0c4585597b375b7bd099e`.
  Only glyph strings from that release are imported; no KiCad/KiCanvas table or
  header text is copied.
- **Licence:** CC0 1.0. Attribution is retained even though the dedication does
  not require it.
- **Generator:** `kicad-strokes/examples/gen_glyphs.rs` verifies the archive
  checksum and glyph count and writes the Rust table. Tests pin its checksum.
- **Coverage:** CJK (U+3000+) is excluded. U+007E uses the 2015 original;
  U+2126 aliases the CC0 U+03A9 Greek capital omega rather than importing
  KiCad's later redraw.
- **Layout:** `kicad-strokes/src/layout.rs` is original kicadmium code. Its
  metrics were measured from `kicad-cli 10.0.6` PCB and schematic SVG plots.
  Fixture stroke points agree within 0.0002 mm, with a 0.001 mm test threshold.

See [Newstroke font provenance](newstroke-provenance.md) for the primary-source
record and the distinction between the CC0 font and downstream KiCad copies.
