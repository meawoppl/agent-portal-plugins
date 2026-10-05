# Browser viewer assets

The first-party viewer is Rust/Yew. KiCad PCB, schematic, symbol, and footprint
sources are converted by the backend into `vector-view::Scene` geometry and
drawn by the shared Rust Canvas2D renderer. Text arrives as Newstroke
polylines; the browser does not parse KiCad files or ship a font engine.

The only vendored JavaScript runtime is Three.js for interactive GLB display:

- `three/three.module.js`
- `three/addons/loaders/GLTFLoader.js`
- `three/addons/controls/OrbitControls.js`
- `three/addons/utils/BufferGeometryUtils.js` (required by `GLTFLoader`)

These upstream files are read-only and retain the Three.js MIT license in
`three/LICENSE`. Kicadmium binds to them directly from Rust/WASM; there is no
first-party JavaScript bridge and no remote module fetch.

STEP remains an export/download artifact. Browser-side OpenCascade, KiCanvas,
the old iframe/postMessage runtime, parser worker, glyph bundle, and adapter
scripts were removed after the Rust viewers became their only consumers.

## Bundled color theme

`american-embedded-dark.json` is the American Embedded Dark KiCad color theme,
copied from the [American Embedded KiCad Repository](https://github.com/American-Embedded/American_Embedded_KiCad_Repository/tree/main/packages/themes/american-embedded-dark)
at version 1.0.0. It is included under CC BY 4.0; American Embedded is the
author. See the [Creative Commons Attribution 4.0 license](https://creativecommons.org/licenses/by/4.0/).
