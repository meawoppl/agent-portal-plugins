pcb-lint
========

Read-only Rust KiCad PCB lint, with 99 implemented checks and persistent,
evidence-bound review decisions. Uses petgraph for copper connectivity and
bounded route search, geo for geometry, and explicit design contracts for facts
that cannot be determined from copper. No board or schematic edits are made.

Build and run (from the repository root)
---------------------------------------
cargo build --release --manifest-path tools/pcb-lint/Cargo.toml
tools/pcb-lint/target/release/pcb-lint rules
tools/pcb-lint/target/release/pcb-lint init-config -o lint-config.json
tools/pcb-lint/target/release/pcb-lint inspect board.kicad_pcb -o objects.json
tools/pcb-lint/target/release/pcb-lint lint board.kicad_pcb \
  --board-id my-project/mainboard --config lint-config.json -o findings.json
tools/pcb-lint/target/release/pcb-lint corpus \
  tools/pcb-lint/examples/corpus.json -o evaluation.json

Lint defaults to reporting without a failing exit code. Use --fail-on warning
or --fail-on error in CI. Exit codes: 0 success, 1 invalid input/runtime failure,
2 threshold failure or corpus mismatch. A zero exit code is NOT engineering
sign-off. Read coverage: evaluated, disabled, needs_input, unsupported_geometry,
and search-budget statuses explain what was actually checked.

Rule catalog
------------
catalog.json contains the full 99-rule catalog: stable ID, family, stage,
severity, confidence, detector, suggested action and caveat. The original 29
cover elementary traces, vias, path shape and basic contracts. The next 70
add copper topology; route alternatives and tuning; placement and repeated
channels; decoupling, feedback and power routing; schematic presentation and
netlist comparison; pin roles and swap constraints; power operating modes;
capacitor/package requirements; probing, signal return and launches; mechanical,
assembly and release checks; annotation/review integrity and labeled noise rates.

An implemented rule can require additional input. It is not treated as a pass
when its inputs are absent. These are heuristics and contract checks, not a
replacement for native KiCad ERC/DRC, a field solver, or manufacturing review.

Design intent inputs
--------------------
Config.intent is strongly typed (src/intent.rs). The committed
tests/fixtures/advanced-bad.json is a complete executable example of all
contract categories, intentionally paired with a faulty board. Do not copy its
electrical values into a real design.

Use inspect to obtain pad, trace and via UUIDs and exact net/reference names.
Config supports enabled_only and disabled rule-ID lists, thresholds, and explicit
pin/net/placement contracts. Intent includes:
  - route bounds, keepouts, roles, tuned nets and search limits;
  - placement partners, channel mappings, repeated spacing and signal flow;
  - power corridors, sensitive regions and ordered feedback/decoupling loops;
  - pin-role, strap, interface, mating and swap requirements with provenance;
  - supply sources, directed conducting links, isolation and operating modes;
  - capacitor effective capacitance, ratings and voltage derating;
  - package, probe, pair, reference-plane, RF-launch and mechanical requirements;
  - normalized schematic observations and expected netlist;
  - BOM/CPL fields, labels, native observations and release artifact manifests;
  - baselines, annotation provenance and labeled evaluation observations.

Schematic observations are supplied as normalized geometry, not parsed directly
from .kicad_sch. Datasheet and component-body facts must be supplied by an adapter
or reviewer. Artifact actual hashes and check-run outcomes are supplied
observations, not independent file hashing or invocation of external tools.
Native snapshots bind to the board source hash. Release checks validate that
reported artifacts/checks belong to that source. Do not assert unverified facts
merely to turn needs_input into evaluated.

Power modes model ideal DC sources with explicit polarity and isolation groups.
Two isolated 12 V sources in series can satisfy 24 V; parallel 12 V sources
cannot. A separate mode represents a single 24 V supply. A multi-source mode
without isolation facts remains needs_input. This model does not simulate
transients, diode drops, current limits, startup or every switch state; describe
each allowed operating state and conductive path explicitly.

Review keying and invalidation
-----------------------------
tools/pcb-lint/target/release/pcb-lint review \
  --report findings.json --ledger reviews.json --key FULL_FINDING_KEY ignore \
  --reason "Reviewed intentional tuning segment; see issue 123" --reviewer agent-name

Use flag instead of ignore to retain an explicit reviewed concern, or clear
to remove that decision. --expires-at accepts Unix seconds. Re-run lint with
--reviews reviews.json.

Identity: SHA-256 of caller-chosen persistent board identity, rule ID, sorted
object UUIDs and a discriminator. Choose a board ID once per independent board;
do not use a revision hash as its identity. Reordering segments does not change
the finding key. Geometry-only fallback IDs and synthetic contract subjects
are not eligible for persistent ignore.

Evidence is separate: configuration, relevant geometry/context and rule-engine
version are hashed. Advanced checks conservatively include the normalized whole
board and parsed extra geometry. Consequently unrelated board edits can require
advanced reviews to be renewed. This favors reopening a review over silently
hiding changed electrical context. Exact key plus exact evidence plus unexpired
review is required for suppression. Changed, expired and unobserved entries
are reported in review audit. Unobserved does not mean resolved.

Recreated UUIDs receive new keys. review.reattach_candidate suggests nearby
same-kind/net candidates from a supplied baseline; it never transfers approval.
An agent must examine the candidate and record a new decision. Reasons and
reviewer identities are mandatory for decisions. Ledger writes are locked and
atomic. Never concurrently edit a ledger manually.

Geometry and search limits
--------------------------
The core endpoint rules are fast screens that do not account for all zone/arc
connections. Advanced topology models filled polygons, holes, pads, vias and
tessellated arcs on copper layers. Unfilled zones, custom pads and unsupported
geometry withhold topology/shortcut conclusions. Invalid polygons are reported
as incomplete geometry. Curves and circles are approximated; exact boundary
cases need native verification. Polygon touching, pad offsets, solder behavior
and text bounding boxes do not constitute fabrication proof.

Route alternatives are bounded searches of 45-degree candidates. Failure to
find a path does not prove optimality or impossibility. Current capacity is a
legacy empirical screening estimate, not IPC-2152 thermal certification.
Pair metrics use total routed copper and straight-segment separation; they do
not establish controlled impedance or propagation delay. Reference planes and
thermal spokes are geometric screens. Placement cost and power-corridor
centroids suggest review, not proven routing improvements. Declared intentional
roles and reviewed suppressions allow legitimate exceptions.

Regression and corpus evaluation
--------------------------------
cargo test --manifest-path tools/pcb-lint/Cargo.toml
cargo clippy --manifest-path tools/pcb-lint/Cargo.toml --all-targets -- -D warnings

examples/corpus.json runs the intentionally faulty fixture against every one
of the 70 added checks. Integration tests also cover clean contracts, valid
12+12 V / 24 V modes, review invalidation, graph cutouts, stale native data,
unsupported geometry, selection independence and bounded search.

A corpus manifest uses schema 1 and cases with name, board, board_id, optional
config, and expected [{rule,min,max,subjects:[]}]. Paths resolve relative to
the manifest. Use tight counts and subject UUIDs for real labeled cases.
Unevaluated expected checks fail corpus evaluation, even if min is zero.
The broad upper bounds in the smoke fixture only assert detection.

Noise observations record rule, key, actual, predicted, max_false_positive_rate
and max_false_negative_rate. Rates use FP/(FP+TN) and FN/(FN+TP); label negatives
as well as positives. Synthetic detections establish executable coverage, not
precision on production boards. Preserve revision-specific board files and
annotations when collecting examples; a screenshot alone cannot reproduce
connectivity or clearance. The historical Tesla revisions are not present in
this worktree.

Visual contact sheets
---------------------
Add --contact-sheet PATH.html to lint:
  tools/pcb-lint/target/release/pcb-lint lint board.kicad_pcb \
    --board-id my-project/mainboard --contact-sheet issues.html -o findings.json

This writes one self-contained offline HTML file with embedded SVG geometry.
No browser or KiCad installation is required to generate it. Open the file in a
browser: filter by rule/severity or search notes, nets, references and UUIDs.
All findings are included; ignored entries are initially hidden. Layer selection
changes the imagery, not the finding count. Filled zones are off by default for
legibility and can be enabled. Subjects are yellow, with red front copper, blue
back copper, purple inner layers, and teal multilayer pads/vias.

Click any crop to enlarge, zoom and pan. Download crop SVG creates a standalone
image with geometry and an opaque background. Print current page prints only
the displayed page of findings; pagination prevents thousands of crops loading
at once. Expand a card's details for the full review key, evidence, object IDs,
location, measurements and any existing decision. The sheet does not edit
review ledgers; use the review CLI after inspection.

Images use the linter's geometry approximation, not native KiCad plots.
Component labels are synthetic. Unsupported shapes and unmodeled graphics
may be missing; text findings show their measured bounding boxes. Nonspatial
findings show an explicitly labeled board overview. The image source hash must
match the report. HTML generation preserves --fail-on behavior and still writes
the sheet when findings cause exit code 2. Output paths cannot alias board,
configuration, review ledger or report inputs.

The repository run is in reports/pcb-lint/contact-sheets/index.html, with
individual offline sheets and JSON reports for all five design boards.
These runs use default thresholds, not calibrated project-specific profiles.

via.low_attachment uses the copper graph: same-net tracks, tessellated arcs,
pads, and saved filled polygons must intersect the via's annulus on at least
two distinct layers within its span. Zone outlines alone do not count. Missing
or invalid fills withhold conclusions for affected nets/layers; unsupported
unlocated copper also withholds incomplete cases (coverage:
partial_unsupported_geometry). Saved fills must be refreshed in KiCad after
layout edits; the lint tool does not refill zones. Fill geometry participates
in review evidence, so changed pours invalidate previous decisions.

route.layer_excursion now screens the actual path moved to a common departure/
return layer, using every segment's width plus configured route clearance.
Foreign-net tracks, arcs, pads, vias, holes and filled pours block a finding;
same-net copper is allowed. Native and configured keepouts and available board outlines
are checked too. Unmodeled copper/keepouts withhold uncertain suggestions.
This is not a search for an alternate route or a substitute for native DRC.

via.overshoot proposes relocating a via joining exactly two straight trace
branches on different layers. It searches their octilinear intersections,
keeps the far endpoints fixed, adds no bends, and requires savings greater
than intent.route.minimum_saving_mm. Findings include candidate coordinates
and saved length, and use the usual review/ignore keys and contact sheets.
The replacement preserves existing branch contacts and screens trace widths,
via diameter on every spanned layer, copper, board edges and track/via keepouts.
It requires complete geometry and an outline; plane/pad/arc-attached vias,
extra branches, tuned nets and declared differential pairs are skipped.
This is a local candidate, not a globally shortest route or automatic edit.
Native DRC and electrical review remain necessary before accepting the move.

route.legal_shortcut also checks consecutive 2–8 segment subchains, then whole
chains. Fixed-endpoint straight/45-degree elbow candidates are screened before
using the bounded visibility router. All external copper contacts of replaced
segments must survive. Tuned nets and declared differential pairs are skipped.
Only the affected segments are subjects, with replacement coordinates and saved
length in the finding. Overlapping findings are suppressed. The default saving
threshold remains intent.route.minimum_saving_mm (1 mm); lower it for smaller
doglegs. Limits are 4096 windows and 32 graph searches per board; exhausted
coverage is explicit. Windows over eight segments are not exhaustively searched.

placement.passive_alignment infers nearby rows/columns of matching two-pad SMD
R/C footprints on the same board side and with the same cardinal pad axis
(180-degree reversals allowed). It uses pad centers, not footprint origins.
Defaults under intent.passive_alignment: min_group=3, max_gap_mm=5,
max_offset_mm=0.5, tolerance_mm=0.1, angle_tolerance_deg=2.
Set min_group=2 to include pairs; exclude_references lists deliberate exceptions.
Findings name the group and signed movements to its median alignment coordinate.
Declared sensitive regions, loop components, tuned nets and differential pairs
are excluded. Advisory only: no courtyard, locked-placement or routing feasibility
claim, and no automatic movement. Review or ignore with the normal ledger.
