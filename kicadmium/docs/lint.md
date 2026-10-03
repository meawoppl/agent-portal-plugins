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
| `lint init BOARD [--board-id ID] [--force]` | Create `<board>.lint.json` with the board id and default config. |
| `lint waive BOARD KEY --reason R --reviewer W [--expires-at UNIX]` | Record a reviewed non-issue. The finding stops counting as open. |
| `lint flag BOARD KEY --reason R --reviewer W [--expires-at UNIX]` | Record a reviewed finding that still needs work. It stays open, with the note attached. |
| `lint clear BOARD KEY` | Remove one exception. |
| `lint stale BOARD` | List orphaned, resolved and expired exceptions. Exits 1 if there are any. |
| `lint prune BOARD [--apply] [--changed] [--keep-orphaned] [--keep-resolved] [--keep-expired]` | Remove stale exceptions. It is a dry run unless you pass `--apply`. |
| `lint diff BEFORE.kicad_pcb AFTER.kicad_pcb [--format ...]` | Show findings that are new or fixed between two board revisions. |
| `lint rules [--format ...]` | The rule catalogue: ids, severity, confidence, suggested action. |
| `lint ci BOARD... [--out lint-results] [--fail-on error]` | Write the CI bundle for every board and exit non-zero on failure. |

Every command that takes `BOARD` also accepts `--file PATH` to override the lint
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
- Schema-1 inputs (a separate `--config` JSON plus `--reviews` ledger, or
  `.kicad-pcb.json` `lint.config` / `lint.reviews`) are migrated. Migrated
  entries stay `unverified` until a run that raises their finding fills in the
  missing metadata.

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
  <board-stem>/
    report.json         full pcb-lint report + exception audit
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
