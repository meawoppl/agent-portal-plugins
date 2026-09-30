---
name: presentation-authoring
description: Create and revise repository-backed engineering presentations using Marp Markdown, shared data, CSS layouts, and reproducible HTML/PDF builds. Use for technical decks and proposal document sets.
---

# Engineering Presentation Authoring

## Start from the repository

Inspect the document registry, Markdown, theme, build scripts, dependency locks,
and existing output before editing. Follow existing paths and commands. Do not
restructure a working deck just to adopt a template. Determine the intended
audience and decision from the user's request and existing narrative; ask only
when that uncertainty materially affects the content.

For a new deck, use Marp Markdown with YAML frontmatter (`marp: true`, theme,
size, pagination), `---` between slides, and a shared CSS theme. Prefer a named
slide layout over repeated inline styles. Keep content as editable text and
figures as linked assets, not screenshots of whole slides.

Typical source directories are `deck/`, `theme/`, `data/`, `figures/src/`, and
`reference/`; generated outputs belong in `build/`. These are defaults, not a
required migration. A multi-document project should have one registry specifying
document IDs, titles, source paths, and order.

## Content and layout

- Give each slide a clear engineering point with the evidence needed to assess
  it. Separate measured performance, simulations, estimates, and design targets.
- Keep shared numbers, units, assumptions, and repeated claims in structured
  source data. Calculate totals and derived quantities. Reject unresolved
  template variables instead of exporting their literal placeholders.
- Preserve source citations and figure provenance. Do not manufacture evidence
  or change a technical claim merely to make a slide easier to lay out.
- Fit content by improving hierarchy, shortening prose, or splitting slides
  before shrinking everything. Inspect labels at the final presentation size.
- Preserve stable slide identifiers through edits and reordering. Reuse an
  existing ID mechanism; otherwise a source comment such as
  `<!-- slide-id: process-flow -->` provides an authoring anchor. It does not
  automatically create renderer support or a source map.

When creating or changing generated imagery, read
[reproducible-figures](../reproducible-figures/SKILL.md). For review annotations,
read [presentation-feedback](../presentation-feedback/SKILL.md).

## Build and inspect

Use the project's pinned Marp and browser dependencies through its build script.
For a new project, provide a documented build command and dependency lockfile;
do not rely on an unpinned `npx` download. Close stdin on noninteractive rendering
subprocesses and give browser/build subprocesses bounded timeouts.

Keep Markdown, includes, CSS, figures, and data as authoritative inputs. Never
fix the generated HTML/PDF or resolved Markdown in place. Track the complete
build scripts and inputs in the repository, including fonts or documented font
acquisition where licensing permits. A fresh checkout must have a reproducible
way to install dependencies and build the requested artifacts.

Render changed slides and inspect actual images, not just build exit status.
Check clipping, overlap, legibility, aspect ratio, missing assets, and adjacent
slides for unintended theme effects. Validate all documents affected by shared
data or CSS edits. Verify the requested export opens and agrees with the preview;
do not imply that a PowerPoint export preserves editable vector/text objects
unless that has been checked for the chosen export path.

Report the source changes, the artifact/link available for review, and what was
actually verified. Explicitly state when visual inspection was unavailable.
