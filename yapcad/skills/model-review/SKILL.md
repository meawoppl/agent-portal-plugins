---
name: model-review
description: Act on yapCAD pane annotations - pinned 3D points, measurements, section planes and camera context tied to a retained run - by changing DSL sources and proving the result with a new run.
---

# Model Review

An annotation from the yapCAD pane carries a screenshot with numbered pins and
measurement labels burned in, plus `context`:

- `run`, `runPath`, `file`, `command`, `params`, `representation`,
  `revision` (sha256 of the build spec and source) and `stats` at capture;
- `pins`: `{id, point [x,y,z] mm, part, normal}` in model coordinates;
- `measure`: `{a, b}` points when the user measured a distance;
- `section`: clipping plane `{axis, normal, constant}` if one was active;
- `camera`: projection, position, target and up; `visibleParts`.

Units are millimetres in the model frame (Z up). Pins are surface hits, so
`normal` tells you which face the user meant (e.g. `[0,0,1]` = top face).

## Procedure

1. Read `RUNPATH/run.json` and `RUNPATH/source.dsl`. Diff the snapshot with
   the current source: the user may have annotated an older run, and the
   change must go into the current authoritative `.dsl`, never the snapshot.
2. Map pins to features: compare coordinates with the parameters and the
   `translate`/`box`/`cylinder` expressions in the command (`box`/`sphere`
   are centred on the origin; `cylinder`/`cone` start at z = 0).
   `render --run ID --view top` or another view helps confirm which feature
   a pin sits on.
3. Make the smallest source change that addresses the note. Prefer changing
   or adding a parameter (with `require` bounds and `@ui` hints) over
   hard-coding, when the note implies the value may be tuned again.
4. `check`, then `build` with the annotation's `params` (plus any new ones).
   Verify with stats (bbox, volume, watertight, bodies) and a preview from a
   comparable view - for pins, the same camera direction.
5. Reply with the new run id, what changed, the evidence, and the preview
   (`agent-portal show`). The open pane follows the new run.

Handle one delivered annotation at a time. The Portal work queue dispatches
the next one; do not build a second queue or message yourself. If a pin is
ambiguous (on a shared edge, or the note conflicts with a `require`), say what
is missing instead of guessing. Completion is not user acceptance.
