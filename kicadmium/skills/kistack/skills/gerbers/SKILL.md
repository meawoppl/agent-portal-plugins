---
name: kicad-gerbers
description: Generate, render, and visually review KiCad Gerber layers as a fabrication-output inspection step.
---

# This skill allows an LLM to review generated GERBER files for production of a PCB.

If no generated Gerbers exist, generate review-only artifacts in a separate
directory with `kicadmium export gerbers --cwd <repo> --out <dir>`. Use the
Kicadmium Gerbers tab, whose viewer is integrated from Rust, to inspect every
layer. Do not install a separate renderer or create an auxiliary script.

Ensure that their are no unexpected shorts, that vias do not intersect, and that overall the board appears to be fabricatable. 
