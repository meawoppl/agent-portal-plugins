# Board lint with `kct lint`

`kct lint` is kicadmium's layout-review toolchain. It runs the pcb-lint
heuristic engine, records reviewed non-issues next to each board, flags those
records once they go stale, and writes viewable CI results. It is advisory:
KiCad ERC/DRC/parity stay the ground truth, and a lint finding is a candidate
to inspect, not an authorised repair.

Agents, humans and CI all run the same commands:

```console
kicadmium kct -- lint <command> ...
```

## Commands

| Command | Effect |
|---|---|
| `lint run BOARD [--format text\|json] [--fail-on never\|warning\|error] [--contact-sheet F] [--exceptions-sheet F]` | Lint one board with its lint file. Prints findings plus an exception audit. Warns on stderr about stale exceptions. |
| `lint init BOARD [--board-id ID] [--force]` | Create `<board>.lint.json` with the board id and default config. `lint init --print-default-config` prints the full default config (every threshold and intent field) instead. |
| `lint waive BOARD KEY --reason R --reviewer W [--expires-at UNIX\|YYYY-MM-DD]` | Record a reviewed non-issue. The finding stops counting as open. |
| `lint flag BOARD KEY --reason R --reviewer W [--expires-at UNIX\|YYYY-MM-DD]` | Record a reviewed finding that still needs work. It stays open, with the note attached. |
| `lint clear BOARD KEY` | Remove one exception. |
| `lint stale BOARD` | List orphaned, resolved and expired exceptions. Exits 1 if there are any. |
| `lint prune BOARD [--apply] [--changed] [--keep-orphaned] [--keep-resolved] [--keep-expired]` | Remove stale exceptions. It is a dry run unless you pass `--apply`. |
| `lint diff BEFORE.kicad_pcb AFTER.kicad_pcb [--format ...]` | Show findings that are new or fixed between two board revisions. |
| `lint rules [--format ...]` | The rule catalogue: ids, severity, confidence, suggested action. |
| `lint ci BOARD... [--out lint-results] [--fail-on error]` | Write the CI bundle for every board and exit non-zero on failure. |
| `lint inspect BOARD [--format text\|json] [-o FILE]` | Parse the board read-only. Text gives object counts; json gives the full normalized model with UUIDs and exact net and reference names. |
| `lint corpus MANIFEST [-o FILE]` | Check a labelled corpus manifest against expected finding counts. Exits 2 if any case fails. |

Every command that reads a lint file (all but `init`, `inspect`, `diff`,
`rules`, `ci` and `corpus`) also accepts `--file PATH` to override the lint
file next to the board. A `KEY` can be the full finding key or a unique prefix
of at least 8 characters.

Routing uses the same engine: `kct route ... --lint-gate warning|error` (also
`route-auto`) lints before and after. If the route introduces new findings at
or above the gate severity, it rolls back, the same way DRC regressions are
rolled back.

## The board lint file

Each board has one file next to it: `foo.kicad_pcb` → `foo.lint.json`. Commit
it with the board. It is the only place lint policy and review decisions live
for that board.

```json
{
  "schema": 2,
  "board_id": "esp32-fpga-module",
  "config": { "...": "only fields that differ from pcb-lint defaults" },
  "exceptions": [
    {
      "key": "889f69e50c2c…",
      "rule": "bom.completeness",
      "subjects": ["<footprint uuid>"],
      "evidence": "<hash of the finding's local context>",
      "action": "ignore",
      "reason": "MPN supplied by assembler",
      "reviewer": "routing-agent",
      "reviewed_at": 1790000000,
      "expires_at": null,
      "severity": "info",
      "message": "Assembly BOM missing MPN",
      "at": { "x": 20.0, "y": 30.0 },
      "nets": []
    }
  ]
}
```

- **key** identifies the finding: board id, rule, subject ids and slot. It
  survives unrelated edits.
- **evidence** hashes the geometry and config the verdict depends on. If it
  changes, the decision has to be reviewed again.
- **ignore** needs a finding with stable identity (UUID-backed subjects).
  Express intent for anonymous objects in `config` instead.
- The rule, subjects, message, severity, location and nets are snapshotted, so
  a card can still be drawn after the geometry is gone.
- Writes take `<file>.lock` and are atomic. Unknown fields are rejected.
- The project-aware workbench reads legacy `.kicad-pcb.json` `lint.config` /
  `lint.reviews` settings when no adjacent board lint file exists. Migrated
  entries stay `unverified` until a run that raises their finding fills in the
  missing metadata; save them into the adjacent file before retiring the legacy
  paths.

## Exception statuses

Every run audits every exception:

| Status | Meaning | Stale? |
|---|---|---|
| `applied` | Finding raised, evidence matches. The decision holds. | no |
| `changed` | Finding raised, but nearby geometry or relevant config changed. Reopened for review. | no; pruned only with `--changed` |
| `resolved` | The rule ran, the finding is gone, and the subjects are still on the board. | yes |
| `orphaned` | The rule ran and the subject geometry no longer exists. | yes |
| `expired` | Past `expires_at`. | yes |
| `unverified` | The finding is absent, but its rule did not fully run (disabled, needs input or a contract, budget exhausted, unsupported geometry). | never pruned |

`resolved` and `orphaned` only apply when coverage says the rule was
`evaluated`, so a skipped rule never looks like a fix. `lint run` warns about
stale exceptions, `lint stale` lists them, and `lint prune --apply` removes
them.

## CI results

`kct lint ci boards/**/*.kicad_pcb --out lint-results` writes:

```text
lint-results/
  summary.md            index of all boards with open/stale counts and links
  findings.sarif        all boards, for GitHub code scanning
  <board-path-slug>__<path-hash>/
    report.json         full pcb-lint report, including review-audit statuses
    findings.html       contact sheet of findings: crops, highlights, filters
    exceptions.html     contact sheet of every exception: crop, reason,
                        reviewer, dates, status badge
    summary.md          per-board tables: open by severity, exceptions by status, stale list
    findings.sarif
```

The HTML pages are self-contained and work offline. The exceptions sheet shows
what the agent (or a human) decided not to fix and why. Orphaned cards list the
stored subjects and say to run `kct lint prune`.

The template workflow is [`docs/ci/kct-lint.yml`](ci/kct-lint.yml).

## Agent loop

1. `lint run BOARD --format json` to find candidates. Inspect each one in the
   workbench or the contact sheet before acting.
2. Fix real issues. Route with `--lint-gate error` (or `warning`), and use
   `lint diff` on hand edits.
3. For true non-issues, `lint waive` with a specific reason. Use `flag` for
   known issues deferred on purpose. Do not waive to make CI pass.
4. After edits, `lint stale`, then `lint prune` (dry run) followed by
   `lint prune --apply`. Re-review `changed` items.
5. Commit the board and its `.lint.json` together. CI reruns everything and
   publishes both contact sheets.

## Engine reference

The engine is the `kct::lint` module (`kct/src/lint/`), formerly the standalone
`pcb-lint` crate. It is read-only: it never edits a board or schematic. It has
101 registered checks and uses petgraph for copper connectivity and bounded
route search, geo for geometry, and explicit design contracts for facts that
copper cannot show.

A zero exit code is not engineering sign-off. Read the report's coverage: the
`evaluated`, `disabled`, `needs_input`, `unsupported_geometry` and
search-budget statuses say what was actually checked.

### Rule catalogue

`kct/src/lint/catalog.json` holds all 101 rules (`kct lint rules`): stable id,
family, stage, severity, confidence, detector, suggested action and caveat. The
first 29 cover basic traces, vias, path shape and contracts. The next 70 add:

- copper topology, route alternatives and tuning;
- placement and repeated channels;
- decoupling, feedback and power routing;
- schematic presentation and netlist comparison;
- pin roles and swap constraints;
- power operating modes;
- capacitor and package requirements;
- probing, signal return and launches;
- mechanical, assembly and release checks;
- annotation and review integrity, and labelled noise rates.

An implemented rule can need extra input. If that input is missing, the rule
does not count as a pass. These are heuristics and contract checks, not a
replacement for native KiCad ERC/DRC, a field solver, or manufacturing review.

### Design intent inputs

`Config.intent` is strongly typed (`kct/src/lint/intent.rs`). The fixture
`kct/tests/fixtures/lint/advanced-bad.json` is a complete, runnable example of
every contract category. It is deliberately paired with a faulty board, so do
not copy its electrical values into a real design.

Run `kct lint inspect BOARD --format json` to get pad, trace and via UUIDs and
exact net and reference names. Config supports `enabled_only` and `disabled`
rule-id lists, thresholds, and explicit pin, net and placement contracts.
Intent includes:

- route bounds, keepouts, roles, tuned nets and search limits;
- placement partners, channel mappings, repeated spacing and signal flow;
- power corridors, sensitive regions and ordered feedback/decoupling loops;
- pin-role, strap, interface, mating and swap requirements, with provenance;
- supply sources, directed conducting links, isolation and operating modes;
- effective capacitance, ratings and voltage derating for capacitors;
- package, probe, pair, reference-plane, RF-launch and mechanical requirements;
- normalised schematic observations and an expected netlist;
- BOM/CPL fields, labels, native observations and release artefact manifests;
- baselines, annotation provenance and labelled evaluation observations.

Schematic observations are supplied as normalised geometry. The engine does
not parse `.kicad_sch` itself. Datasheet and component-body facts must come
from an adapter or a reviewer. Artefact hashes and check-run outcomes are
supplied observations: the engine does not hash files or run external tools.
Native snapshots are tied to the board source hash, and release checks confirm
that reported artefacts and checks belong to that source. Do not assert facts
you have not verified just to turn `needs_input` into `evaluated`.

Power modes model ideal DC sources with explicit polarity and isolation groups.
Two isolated 12 V sources in series can satisfy 24 V, but two 12 V sources in
parallel cannot. A single 24 V supply is described as a separate mode. A
multi-source mode without isolation facts stays `needs_input`. The model does
not simulate transients, diode drops, current limits, startup or every switch
state, so describe each allowed operating state and conducting path explicitly.

### Finding identity and evidence

A finding key is the SHA-256 of the persistent board id, rule id, sorted object
UUIDs and a discriminator. Choose a board id once per independent board, and
do not use a revision hash as the id. Reordering segments does not change the
key. Geometry-only fallback ids and synthetic contract subjects cannot be
persistently ignored.

Evidence is hashed separately per rule (engine-3, `kct/src/lint/evidence.rs`).
It covers:

- the subjects, with footprint fields, pad thermals and footprint copper;
- every object within 5 mm of the finding or its subject geometry, on any net;
- zones that are nearby or share a net with the finding;
- only the config fields the rule reads.

Whole-net rules (connectivity, islands, stubs, length/skew, decoupling loops,
net contracts; see `WHOLE_NET`) also hash all copper on the finding's nets.
Metadata rules (BOM, CPL, schematic, release; see `METADATA`) hash only their
subjects. As a result, distant unrelated edits keep reviews applied, while
nearby edits or changes to the rule's own thresholds reopen them. Upgrading
from engine-2 changes every evidence hash once. Existing reviews then read as
`changed` and need one re-review (re-waive or re-flag them).

A finding is suppressed only by an exact key, exact evidence and an unexpired
review. Recreated UUIDs get new keys. `review.reattach_candidate` suggests
nearby candidates of the same kind and net from a supplied baseline, but it
never transfers approval: an agent must examine the candidate and record a new
decision.

### Geometry and search limits

The core endpoint rules are fast screens and do not account for every zone or
arc connection. Advanced topology models filled polygons, holes, pads, vias and
tessellated arcs on copper layers. Unfilled zones, custom pads and unsupported
geometry withhold topology and shortcut conclusions. Invalid polygons are
reported as incomplete geometry. Curves and circles are approximated, so exact
boundary cases need native verification. Polygon touching, pad offsets, solder
behaviour and text bounding boxes are not proof of fabricability.

Route alternatives are bounded searches over 45-degree candidates. Not finding
a path proves neither optimality nor impossibility. Current capacity is a
legacy empirical screening estimate, not IPC-2152 thermal certification. Pair
metrics use total routed copper and straight-segment separation, so they do not
establish controlled impedance or propagation delay. Reference-plane and
thermal-spoke checks are geometric screens. Placement cost and power-corridor
centroids suggest review; they are not proven routing improvements. Declared
intentional roles and reviewed suppressions cover legitimate exceptions.

- **`via.low_attachment`** uses the copper graph. Same-net tracks, tessellated
  arcs, pads and saved filled polygons must intersect the via's annulus on at
  least two distinct layers within its span. Zone outlines alone do not count.
  Missing or invalid fills withhold conclusions for the affected nets and
  layers. Unsupported unlocated copper also withholds incomplete cases
  (coverage `partial_unsupported_geometry`). Refresh saved fills in KiCad after
  layout edits, because the engine does not refill zones. Fill geometry is part
  of the evidence, so changed pours reopen earlier decisions.
- **`route.layer_excursion`** screens the actual path, moved to a common
  departure/return layer, using every segment's width plus the configured route
  clearance. Foreign-net tracks, arcs, pads, vias, holes and filled pours block
  a finding; same-net copper is allowed. Native and configured keepouts and any
  available board outline are also checked. Unmodelled copper or keepouts
  withhold uncertain suggestions. This is not a search for an alternate route
  and does not replace native DRC.
- **`via.overshoot`** proposes moving a via that joins exactly two straight
  trace branches on different layers. It searches their octilinear
  intersections, keeps the far endpoints fixed, adds no bends, and requires a
  saving greater than `intent.route.minimum_saving_mm`. Findings include the
  candidate coordinates and saved length. The move preserves existing branch
  contacts and screens trace widths, via diameter on every spanned layer,
  copper, board edges and track/via keepouts. It needs complete geometry and an
  outline. Plane-, pad- or arc-attached vias, extra branches, tuned nets and
  declared differential pairs are skipped. This is a local candidate, not a
  globally shortest route or an automatic edit, so native DRC and electrical
  review are still needed before accepting it.
- **`route.legal_shortcut`** checks runs of 2 to 8 consecutive segments, then
  whole chains. Fixed-endpoint straight or 45-degree elbow candidates are
  screened before the bounded visibility router runs. Every external copper
  contact of the replaced segments must survive. Tuned nets and declared
  differential pairs are skipped. Only the affected segments are subjects, and
  the finding includes the replacement coordinates and saved length.
  Overlapping findings are suppressed. The default saving threshold is
  `intent.route.minimum_saving_mm` (1 mm); lower it to catch smaller doglegs.
  The limits are 4096 windows and 32 graph searches per board, and exhausted
  coverage is reported. Windows longer than eight segments are not searched
  exhaustively.
- **`placement.passive_alignment`** finds nearby rows or columns of matching
  two-pad SMD R/C footprints on the same side with the same cardinal pad axis
  (180-degree reversals allowed). It uses pad centres, not footprint origins.
  Defaults under `intent.passive_alignment` are `min_group=3`, `max_gap_mm=5`,
  `max_offset_mm=0.5`, `tolerance_mm=0.1` and `angle_tolerance_deg=2`. Set
  `min_group=2` to include pairs, and list deliberate exceptions in
  `exclude_references`. Findings name the group and the signed move to its
  median alignment coordinate. Declared sensitive regions, loop components,
  tuned nets and differential pairs are excluded. The check is advisory only:
  it makes no claim about courtyards, locked placement or routing feasibility,
  and moves nothing.

### Contact sheets

`--contact-sheet PATH.html` writes one self-contained offline HTML file with
embedded SVG geometry. Generating it needs no browser or KiCad install. In a
browser you can filter by rule or severity, and search notes, nets, references
and UUIDs. All findings are included, with ignored ones hidden at first.
Choosing layers changes the images, not the finding count. Filled zones are off
by default for legibility and can be turned on. Subjects are yellow; front
copper is red, back copper blue, inner layers purple, and multilayer pads and
vias teal.

Click a crop to enlarge, zoom and pan it. "Download crop SVG" saves a
standalone image with geometry and an opaque background. "Print current page"
prints only the page of findings on screen; pagination keeps thousands of crops
from loading at once. Open a card's details to see the full review key,
evidence, object ids, location, measurements and any existing decision. The
sheet never edits exceptions; use `kct lint waive`/`flag` after inspecting.

The images come from the engine's geometry approximation, not native KiCad
plots. Component labels are synthetic. Unsupported shapes and unmodelled
graphics may be missing, and text findings show their measured bounding boxes.
Findings with no location show a clearly labelled board overview. The image
source hash must match the report. The sheet is still written when findings
fail the `--fail-on` threshold.

### Regression and corpus evaluation

`cargo test -p kct --test 'lint_*'` runs the engine suite. The smoke corpus,
`kct lint corpus kct/tests/fixtures/lint/corpus.json`, runs the deliberately
faulty board against every one of the 70 added checks. The integration tests
also cover clean contracts, valid 12+12 V and 24 V modes, review invalidation,
graph cutouts, stale native data, unsupported geometry, independence from
selection, and bounded search.

A corpus manifest uses schema 1. Each case has `name`, `board`, `board_id`, an
optional `config`, and `expected` entries of `{rule, min, max, subjects: []}`.
Paths are relative to the manifest. Use tight counts and subject UUIDs for real
labelled cases. An expected check that was not evaluated fails the evaluation,
even when `min` is zero. The broad upper bounds in the smoke fixture only
assert that each check fires.

Noise observations record `rule`, `key`, `actual`, `predicted`,
`max_false_positive_rate` and `max_false_negative_rate`. Rates are FP/(FP+TN)
and FN/(FN+TP), so label negatives as well as positives. Synthetic detections
show that a check runs, not how precise it is on production boards. Keep
revision-specific board files and annotations when collecting examples, because
a screenshot alone cannot reproduce connectivity or clearance.
