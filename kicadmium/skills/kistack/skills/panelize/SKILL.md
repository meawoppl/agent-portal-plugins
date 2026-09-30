---
name: kicad-panelize
description: Panelize a PCB with native Kicadmium automation
---

# Skill for PCB panelization 

## Setup
Use the native `kicadmium kct` panelization command and its current `--help`.
Do not invoke a separate panelizer or its historical helper files directly.

## Usage

- Find the source .kicad_pcb requsted by the user
- Inspect its actual board files, Edge.Cuts, copper layers, pads and components that may interesect board edges to identify items like castellations, edge connectors, antennas, plated edges, etc.
- Resolve grid dimensions, spacing, rotation, tabs, separation, frame, and fabricator limits.
- Position tooling holes and fiducials relative to the panel outline
- Write strict json.
- Generate a versioned panel without overwriting the source in a "build" subdirectory
- Check the generated panel before notifying the user, ensuring that it looks proper from the baord file level as well as in render.

## Preferred starting values

- Board routing gap: 2mm
- Tab width: 3mm
- Mouse-bite drill: 0.5mm
- Mouse-bite spacing: 0.8mm
- Mouse-bite offset: 0mm
- Full frame width: 5mm
- Frame-to-array clearance: 2mm
- Frame break cuts: none
- Exterior frame radius: 2mm
- Interior routing radius: 0.25mm
- Tooling: three 1.5mm holes
- Tooling stencil apertures: enabled
- Fiducials: three, 1mm copper with 1mm openings
- Fiducial paste apertures: disabled

## Panel behavior

- source.type: "auto" is appropriate for an ordinary single-board file.
- tabs.type: "spacing" places tabs symmetrically and cannot exclude a particular side.
- With fixed tabs:
  - hcount controls tabs projecting from left/right edges.
  - vcount controls tabs projecting from top/bottom edges.
  - A zero count suppresses that pair of directions.

- Use annotation tabs when only selected segments or one side must be excluded.
- Protected tab directions must be derived from each board’s geometry and rotation. Never hardcode “bottom” or “right” as a universal
rule.

- The global mouse-bite cut method processes every generated cut path, including framing.cuts.
- Therefore, framing.cuts: "both" with mouse bites drills break rows through the rails.
- Use framing.cuts: "none" for a continuous frame.
- Tooling and fiducial offsets are measured inward from the panel’s outer edges.
- To center a feature across a 5 mm frame, use a 2.5 mm perpendicular offset.
- Tooling holes and fiducials with identical counts and offsets overlap. Separate them along the rail.
- paste: true means the feature gets an aperture on the stencil paste layers. It does not control whether a drilled hole is visible.
- framing.fillet rounds exterior panel corners.
- post.millradius rounds concave interior routed corners.
- post.reconstructarcs: true produces real Edge.Cuts arcs instead of dense line-segment approximations.

## Useful commands

```console
kicadmium kct --cwd <repo> -- panel --help
```

Inspect the fully merged panel configuration before accepting output.

Use the command's resolved-configuration output when reviewing merged values.

Render and open it with:

kicad-cli pcb render \
--output panel-top.png \
--width 1600 \
--height 1200 \
--side top \
--background opaque \
--quality high \
panel.kicad_pcb

kicad-cli pcb render \
--output panel-iso.png \
--width 1600 \
--height 1200 \
--perspective \
--rotate 325,0,35 \
panel.kicad_pcb
