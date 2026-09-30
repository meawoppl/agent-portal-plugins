---
name: kicad-footprint
description: Generate and validate compliant KiCad footprints with native Kicadmium tooling
---

# This skill allows an LLM to generate KiCad footprints with Kicadmium's native
# Rust automation and validate them in the workbench.

## Setup
Inspect `kicadmium kct --help` for the current footprint commands. Do not clone
or execute a generator framework. Use existing KiCad library footprints as the
style reference and keep generated assets inside the project library.

## Usage

- First, identify if the footprint the user is asking for already exists in their KiCad library! If it does, there's no need to recreate it unless they are asking for modifications like solder mask expansion for BGAs or smaller courtyard areas, or silkscreen modification.
- Next, identify the footprint category (for example QFN or LGA). Ask when the
  package identity or land-pattern variant is ambiguous.
- Compare against existing project and official KiCad footprints in the same
  category so naming, courtyard, fabrication, silkscreen, and pad conventions
  remain consistent.
- If the user supplied a land-pattern drawing, transcribe every dimension into
  an explicit geometry plan before invoking the native generator.
- Ask the user for the 3D dimensions if necessary to render the 3D model (unless the user specifies not to render a 3D model of course).
- Verify the footprint by investigating the files and rendering them. Also do the same with the 3D model
- When complete, ask the user where the footprint should live. If this info is already in context, please use it. (AGENTS.md or CLAUDE.md might contain that so if so please use that.)

Items to watch for that may require human intervention:
- When pads are non-standard in shape
- When an existing generator is not available, please ask before implementing a new generator

Keep generator inputs and provenance beside the project library. Do not modify
unrelated footprints.
