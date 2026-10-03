# Design principles

Kicadmium is the heavy metal your PCBs were missing: blunt, fast,
opinionated, and willing to point at the ugly part of a board. It is an
independent review and automation workbench. It does not ship KiCad, replace
KiCad's native ERC/DRC, or turn a heuristic suggestion into electrical proof.

## Truthful viewers (from i2cjak/Backplane)

- The configured project files are canonical. Never present a half-written
  save or stale async result as the current board.
- Read a coherent snapshot, attach its revision to every view, and discard a
  result if the user has changed project or revision before it arrives.
- On refresh, retain scene identity and highlight only genuine changes.
- Selection is semantic: pads, tracks, vias, symbols, and pins expose stable
  net/reference/value/footprint context that can be carried into a request.
- Missing external tools remain explicit. A viewer may degrade gracefully;
  validation may not fabricate success.
- Long work is a job with progress and cancellation. A completed job is
  immutable and its result is delivered once.
- File watching is debounced and revisioned. UI state is derived from an
  event log/snapshot rather than timing accidents.

These are engineering conventions adopted from Backplane's documented
`scene_merge_self`, `stale_board_ignored`, replay, and task-delivery laws. They
are implemented as Rust invariants and tests where practical, not copied as a
Bend runtime.

## Agent-oriented tools (from rjwalters/kicad-tools)

- Prefer structured JSON and stable identifiers over terminal prose.
- Separate read-only inspection from mutation. Mutations require an explicit
  target and are followed by native ERC/DRC, sync, quality, and visual review.
- Preserve useful upstream command shapes where practical while implementing
  them as typed, tested Rust APIs.
- Make manufacturer rules, source revisions, assumptions, and unresolved
  evidence part of the result.
- Keep part lookup tiered and optional. Credentials are user-supplied and are
  never required for offline review.
- Treat routing, placement, repair, and optimization output as proposals until
  validated against canonical files and native tools.

`kct` is a native workspace crate dispatched inside the Kicadmium binary.
Kicadmium's server, UI, parsers, review engine, and `kct lint` are Rust; only
KiCad's own `kicad-cli` remains an external runtime dependency.

## Heuristic lint contract

The integrated linter is read-only. Its 101-rule registry includes explicit
coverage states such as `evaluated`, `needs_input`, `needs_contract`,
`unsupported_geometry`, and `budget_exhausted`. Unknown is not pass. Saved
zone fills are inspected, not recomputed. Suggestions for shortcuts, via
relocation, and passive alignment are advisory and are not proof of routing,
clearance, locking, courtyard, or electrical safety.

Review decisions are keyed to stable subjects and evidence. Geometry or
configuration changes invalidate stale approvals. Native KiCad remains the
authority for ERC/DRC and fabrication output.

### Findings in the existing work queue

The contact sheet is a visual review surface, not just a report: each card
combines a board crop, highlighted subjects, explanation, evidence, and a
stable review key. An explicit **enqueue** action may translate a reviewed
card into the existing interaction queue with:

- board identity and source revision;
- rule ID, subject UUIDs, location/crop bounds, and suggested action;
- finding key for deduplication and evidence hash for stale-review detection;
- an authorization state that distinguishes triage from an approved repair.

Queue processing rechecks the revision and finding, performs at most the
authorized focused change, and reruns the relevant lints and native checks.
An absent finding is not “fixed” when its rule became skipped,
`unsupported_geometry`, or `budget_exhausted`. Outcomes are resolved,
intentionally ignored, or needs review; this does not create a second task
system or automatically turn lint findings into board edits.
