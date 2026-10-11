# Touch navigation

On coarse-pointer devices (including a touch screen with a mouse attached),
view tabs, PCB tools, layer presets, Gerber controls and build actions have at
least 44 × 44 CSS-pixel targets. Layer rows have a 44px minimum height. PCB
layer aliases are visible below their KiCad names instead of requiring hover.
The enlarged layer panel starts below the toolbar; the narrow-screen bottom
panel remains available. These rules leave the compact desktop layout intact.

PCB and schematic canvases use the shared vector-view navigation engine. The
host remembers whether **any** part of the current pointer sequence involved
multiple pointers: lifting a stationary finger after a pinch cannot select a
part. A fresh one-finger tap still selects. Lost pointer capture cancels that
pointer, just like pointercancel, so later movement cannot continue a stale drag.

The 3D view retains the upstream OrbitControls behavior (one-finger orbit,
two-finger dolly/pan). The pinned Gerber renderer supports one-pointer drag and
wheel navigation, **not native two-finger touch pinch**. This change enlarges
its controls but does not change or claim to fix that upstream limitation.

## Verification

Native regression tests cover stationary-finger release after pinch, a
cancelled second pointer, subsequent selection, and ignored moves after cancel.
Browser checks use an isolated copy of a KiCad 9 LED board plus a text schematic
fixture; no production design or hardware is operated.

- Playwright WebKit, iPad Pro 11 descriptor, 834 × 1194 and 1194 × 834:
  inspect target bounds, PCB pan/pinch via synthetic PointerEvents, lost capture,
  toolbar/layer separation, schematic rendering and no document overflow.
- Chromium with touch emulation and CDP Input.dispatchTouchEvent:
  render a real GLB, orbit with one finger, dolly/pan with two, and inspect the
  before/after frames. Load the generated Gerbers and inspect controls.
- Desktop pointer mode: verify touch sizing/aliases do not activate.

Synthetic PointerEvents exercise application handlers; they do not prove native
pointer capture or hardware palm rejection. Chromium's injected touch events
exercise capture but are not Safari. Physical iPad/Pencil and keyboard testing
remain a separate acceptance check for the annotation workflow.
