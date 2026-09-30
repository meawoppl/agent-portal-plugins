---
name: reproducible-figures
description: Generate and adjust engineering diagrams, plots, and CAD illustrations from versioned scripts and inputs, with reproducible rendering and visual checks in the destination presentation.
---

# Reproducible Engineering Figures

## Choose editable sources

Find the existing figure and its generator before changing it. Prefer the
repository's rendering tools. Use plotting libraries for quantitative charts,
SVG or a diagram language for schematic figures, and CAD/Three.js/Blender for
geometry-based renders. Photographs and generated raster illustrations are
appropriate when they convey the subject; do not substitute invented imagery
for measured data or mechanically meaningful geometry.

Keep the complete generator in `figures/src/` or the existing equivalent.
Version its input data, geometry sources or reproducible acquisition recipe,
configuration, and dependency locks. Never leave the only working generator in
a terminal command, notebook output, temporary directory, or chat transcript.

## Make adjustment straightforward

- Expose expected changes as named parameters: dimensions and units, labels,
  colors, line widths, font sizes, view direction, camera target, projection,
  lighting, resolution, and crop/padding as applicable.
- Separate input data and physical geometry from presentation styling. Show
  physical scale honestly; label conceptual or exaggerated geometry explicitly.
- Share theme tokens across figures. Use an opaque background; for dark Portal
  figures use `#1a1b26`. Avoid label placement that only works at one resolution.
- Use deterministic seeds when randomness is involved. Record tool versions
  and resolve paths relative to the project, not a developer's home directory.
- Prefer SVG for diagrams and plots and PNG for raster output; provide the
  format the slide renderer actually supports. Preserve editable source even
  when the delivered asset is a raster image.

## Rebuild contract

Provide one documented command that regenerates the figure and integrate it
into the presentation build when appropriate. Track every required helper script.
Pin dependencies and document system requirements such as the browser, fonts,
or Blender version. Do not claim bit-for-bit reproducibility across untested
platforms; distinguish deterministic inputs from pixel-identical output.

If caching expensive renders, fingerprint all relevant scripts, inputs,
configuration, and tool versions. Missing outputs invalidate the cache. Provide
a force-rebuild path. Publish completed outputs atomically so previews do not
consume partially written files.

Record provenance: source/license of external assets, input datasets, generator
path, parameter file, and reproduction command. For remote or proprietary inputs
that cannot be checked in, document acquisition and fail clearly when absent.
For AI-generated raster assets, retain the asset and prompt/reference provenance;
do not promise the model will regenerate identical pixels.

## Verify the delivered figure

Run the generator, inspect the output, then inspect it inside the rendered
slide/page at its actual size. Check framing, labels, units, color contrast,
clipping, and whether important geometry is visible. For numerical charts,
compare plotted values with their source data. For CAD views, check the camera
and occlusion rather than treating nonblank pixels as sufficient validation.

Include the generator, inputs, parameters, and build instructions in the change
set, not only the resulting image. Follow the user's commit/push instructions;
do not imply files are committed when they are merely present in the worktree.
