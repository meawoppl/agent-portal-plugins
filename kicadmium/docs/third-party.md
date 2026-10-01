# Third-party code in kicadmium

This file records code in kicadmium that is not original to it. Vendored
JavaScript/WASM runtimes under `frontend/static/kicad-viewer/` are covered by
their own upstream notices (`frontend/static/kicad-viewer/UPSTREAM-LICENSE.txt`
and the files themselves).

## pastebom viewer (Rust PCB view)

- Source: <https://github.com/meawoppl/pastebom.com>, `crates/viewer/src/`
  (`render.rs`, `pcbdata.rs`, `state.rs`, `main.rs`). Reference revision:
  `91ee2d7c9bad66bf85bec006f63d95d0f26c11c2` (branch
  `meawoppl/gerber-view-embeddable`).
- Author: meawoppl, the same author as kicadmium.
- The pastebom repository has no LICENSE file and its `Cargo.toml` declares
  no license. On 2026-10-01, its author and rightsholder explicitly authorized
  incorporating the derived items listed below into kicadmium under
  kicadmium's MIT license. This authorization applies to these derived items;
  it does not assert a license for the pastebom repository as a whole.

Nothing was copied verbatim. The items below were written while working from
the pastebom code: same algorithms, data layout and conventions, adapted to
kicadmium's layer-aware board model. They are marked in each file's header.
They have been left as written and not reworded to hide where they came from.

| kicadmium file / item | Derived from (pastebom `crates/viewer/src/`) | What changed |
|---|---|---|
| `frontend/src/pcb_view/geom.rs` `rect_outline` | `render.rs` `get_chamfered_rect_path` | Builds a point list in the pad frame (arcs sampled) instead of a `Path2d` with `arcTo`, so pads can be batched and hit-tested. Same chamfer bit layout (1 TL, 2 TR, 4 BL, 8 BR). |
| `geom.rs` `stadium` | `render.rs` `get_oblong_path` | Point list instead of `Path2d`. |
| `geom.rs` `shape_pieces`, `pad_pieces`, `drill_piece` | `render.rs` `get_pad_path`, `draw_pad`, `draw_pad_hole` | Pad/drill outlines are returned in board coordinates (KiCad angle and drill offset applied in Rust) rather than drawn under canvas transforms. Adds `trapezoid` (`rect_delta`), the custom-pad anchor plus primitives, and oval/rect slot drills from KiCad data. |
| `geom.rs` `rotate` | `render.rs` `rotate_vector` | KiCad's counter-clockwise (y-down) sign convention. |
| `geom.rs` `dist_to_segment` | `render.rs` `point_within_distance_to_segment` | Returns the distance instead of a bool. |
| `geom.rs` `dist_to_shape` (arc case) | `render.rs` `track_hit_scan` (arc sweep test) | Generalised to all shapes; arc end caps measured to the endpoints. |
| `geom.rs` `hit_test`, `hit_net` | `render.rs` `bbox_hit_scan`, `net_hit_scan` | One prioritised pick (pads > vias > tracks > footprints > zones, front side first) over KiCad layers with visibility filtering, instead of separate footprint and net scans over iBOM F/B data. |
| `geom.rs` `View` (`fit`, `zoom_at`, `to_board`, `to_screen`) | `render.rs` `Transform`, `recalc_layer_scale`, `screen_to_board`; `main.rs` wheel zoom | Single `scale`/`tx`/`ty` transform plus a back-view mirror flag, instead of `s`/`x`/`y`/`panx`/`pany`/`zoom` and per-side canvases. Fit margin matched to KiCanvas. |
| `frontend/src/pcb_view/render.rs` `GroupBuilder`, `paint_group` | `render.rs` `get_polygons_path`, `draw_edge`, `draw_polygon_shape`, `draw_tracks`, `draw_zones` | Same canvas conventions (round caps/joins, minimum one-pixel line width, filled vs stroked drawings, via ring then hole). Geometry is batched into cached `Path2d`s per paint pass, with strokes grouped by width, instead of drawn item by item each frame. Polygon winding is normalised for nonzero fill. |
| `render.rs` `Scene` net/normal groups, `draw` | `render.rs` `draw_background`, `draw_highlights_on_layer`, `redraw_canvas` | Highlighted net drawn over a dimmed board, as in pastebom's highlight layer. Layer order and palette come from KiCanvas (`theme.rs`, original), not pastebom's CSS colours. |
| `frontend/src/pcb_view/mod.rs` `install_input` (pointer capture, pointer map, two-pointer pinch with centroid pan, wheel zoom) | `main.rs` `on_canvas_pointerdown`/`pointermove`/`pointerup`, `on_canvas_wheel` | Uses `gloo_events` listeners on a shared engine. Adds a click-vs-drag threshold and Escape to clear. |
| `mod.rs` `Settings`, `load_settings`, `Engine::save` | `state.rs` `Settings`, `read_storage`, `write_storage`, `init_settings` (`hidden_layers`) | Fewer fields (hidden/shown layers, flip, panel, text classes), stored as one JSON value; per-project camera added. |
| `shared/src/pcb.rs` `PcbShape`, `PcbPad`, `PcbTrack`, `PcbZone` | `pcbdata.rs` `Drawing`, `Pad`, `Track`, `Zone` | Same kind of render-ready board model (segment/arc/circle/polygon/curve shapes; pad shape, size, angle, offset, chamfer, drill). Layer-indexed (all KiCad layers) instead of iBOM front/back silkscreen/fab. Arcs are centre/radius/radians. Text is KiCad text plus effects (see below). |

Original to kicadmium (not derived from pastebom): `backend/src/pcb_view.rs`
(KiCad to `PcbBoard` conversion over `kct`), `frontend/src/pcb_view/theme.rs`
(KiCanvas layer stack and theme), `frontend/src/pcb_view/text.rs` (system-font
text layout), the properties/layers panels in `mod.rs`, and the toggle in
`frontend/src/viewer.rs`.

## Fonts

No KiCad font data ships with the Rust PCB view. Newstroke is not used. Text
is drawn with the browser's system monospace font (see
`docs/javascript-triage.md`, T2). Before any future Rust port of Newstroke,
the original upstream source, license, and required notice must be recorded;
KiCad's downstream file labels alone are not the provenance record.
