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

## pastebom `eda-view` crate (modules `vector`, `components`, `schematic`, `kicad`, `strokes`, `pcb`, `gerber_view`)

- Source: <https://github.com/meawoppl/pastebom.com>, `crates/eda-view`.
- One crate at one revision, pinned in the workspace `Cargo.toml` and
  `Cargo.lock`, so `eda_view::vector::Scene` is a single type everywhere.
  Each module sits behind a cargo feature; the backend enables `schematic`
  and the frontend `yew` + `gerber-view`.
- Author/rightsholder: meawoppl, the same author as kicadmium, who explicitly
  authorized this use and the earlier extraction work under kicadmium's MIT
  licence. The crate is MIT.

What each module does here:

- `vector` (always on): the renderer-independent scene model plus the native/Canvas2D
  view, hit-test and input engine. It was extracted from pastebom's viewer and
  generalized to layer-aware PCB, schematic, symbol, footprint and Gerber
  geometry. The backend produces `eda_view::vector::Scene` for every 2D view.
- `components` (frontend, feature `yew`): the Yew canvas host (`VectorSceneCanvas`),
  gesture classifier, component/net search and the one-sheet-at-a-time
  `SchematicView`. The canvas, input and search modules were upstreamed from
  kicadmium's own `frontend/src/{vector_scene,view_input,viewer_search}.rs`.
- `schematic` and `kicad` (backend only): filesystem-free
  schematic and library-symbol scene extraction, and the KiCad schematic model
  and s-expression parser it uses. The rendering modules were extracted from
  kicadmium's `backend/src/{schematic_view,schematic_drawing,symbol_graphics}.rs`
  (MIT); the parser models come from kct's schema with its original notice.
  kicadmium keeps the file access, project-root confinement and revision checks.
- `strokes` (backend only): KiCad text to stroke polylines, used for
  schematic, library-preview and PCB text. It was kicadmium's own workspace
  crate and moved upstream unchanged.
- `gerber_view` (frontend, feature `gerber-view`, which brings the `pcb`
  parsers with it): the Gerbers tab viewer.

None of the `schematic`, `kicad` or `strokes` features is enabled for the wasm
frontend; KiCad parsing and text layout stay on the backend.

## NewStroke stroke font (in `eda-view`'s `strokes` module)

- **What:** `crates/eda-view/src/strokes/glyphs.rs` in pastebom, the NewStroke
  glyph table (11,232 entries, U+0020–U+2BFF) used to turn schematic,
  library-preview and PCB text into backend-reified stroke polylines.
- **Attribution:** NewStroke by Vladimir Uryvaev (vovanium), CC0 1.0, release
  2015-11-19, <https://vovanium.ru/sledy/newstroke/en>.
- **Source:** the author's release tarball
  <https://vovanium.ru/_media/sledy/newstroke/newstroke-font.tgz>, SHA-256
  `dba1c334834e21bdbd15395dc610ecc2bf2eb6675ba0c4585597b375b7bd099e`.
  Only glyph strings from that release are imported; no KiCad/KiCanvas table or
  header text is copied.
- **Licence:** CC0 1.0. Attribution is retained even though the dedication does
  not require it.
- **Generator:** `crates/eda-view/examples/strokes_gen_glyphs.rs` verifies the
  archive checksum and glyph count and writes the Rust table. Tests pin its
  checksum. The crate keeps its own copy of this record
  (`crates/eda-view/docs/newstroke-provenance.md`).
- **Coverage:** CJK (U+3000+) is excluded. U+007E uses the 2015 original;
  U+2126 aliases the CC0 U+03A9 Greek capital omega rather than importing
  KiCad's later redraw.
- **Layout:** `crates/eda-view/src/strokes/layout.rs` is original kicadmium code.
  Its metrics were measured from `kicad-cli 10.0.6` PCB and schematic SVG plots.
  Fixture stroke points agree within 0.0002 mm, with a 0.001 mm test threshold.

See [Newstroke font provenance](newstroke-provenance.md) for the primary-source
record and the distinction between the CC0 font and downstream KiCad copies.
