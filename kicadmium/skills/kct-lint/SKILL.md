# kct lint: find, fix, waive, prune

Use this skill when you change board geometry (routing, placement, silkscreen,
zones), review layout quality, or handle a failing `kct lint` CI job. The full
reference is `docs/lint.md` in the kicadmium repo. `kicadmium kct -- lint
--help` and each subcommand's `--help` are authoritative.

## What CI does

Board repositories run `.github/workflows/kct-lint.yml`, from kicadmium's
`docs/ci/kct-lint.yml`. On every push and PR it runs
`kicadmium kct -- lint ci <every .kicad_pcb> --out lint-results --fail-on error`
and publishes:

- a job summary per board: open findings by severity, exceptions by status,
  stale exceptions;
- the `kct-lint-results` artifact with `findings.html` and `exceptions.html`
  contact sheets for each board;
- SARIF annotations on the PR when code scanning is enabled.

The exceptions sheet shows every waiver you record, your reason, and whether it
still applies. Reviewers read it, so write reasons for them. Check for the
workflow file before claiming CI coverage. If it is missing, offer to add it.

## Loop

1. **Find.** `kicadmium kct -- lint run <board> --format json`. Each finding
   has a `key`, `rule`, `severity`, `confidence`, `message`, `subjects`, `nets`
   and location. Look at it in the workbench PCB tab or with `--contact-sheet
   /tmp/f.html` before deciding. Heuristics can be wrong.
2. **Fix** the real issues with the narrowest native command (see
   `kicad-tools-automation`). When routing, pass `--lint-gate error` (or
   `warning`) to `kct route` / `route-auto`, so a route that adds findings is
   rolled back. After hand edits, compare revisions with
   `kicadmium kct -- lint diff before.kicad_pcb after.kicad_pcb`.
3. **Record non-issues.** For a finding that is intended or harmless:
   `kicadmium kct -- lint waive <board> <key> --reason "<why, specific>" --reviewer <you>`.
   Use `flag` instead for real issues deliberately deferred. They stay open
   with your note. Add `--expires-at <unix>` when the exception is temporary.
   Never waive just to turn CI green, and never waive an `error` without saying
   why it is safe. Board-wide intent such as net classes or deliberate stubs
   belongs in the `config` of `<board>.lint.json` (create it with
   `lint init <board>`), not in dozens of waivers.
4. **Keep exceptions honest** after every geometry change:
   - `kicadmium kct -- lint stale <board>` lists `orphaned` (geometry deleted),
     `resolved` (finding gone) and `expired` exceptions.
   - `kicadmium kct -- lint prune <board>` shows what would be removed. Add
     `--apply` to remove it.
   - `changed` means nearby geometry or config moved. Re-inspect, then
     `waive` again or fix. Prune those only with `--changed`.
   - `unverified` means the rule did not run (disabled, needs input, out of
     budget). It is never pruned. Do not treat it as fixed.
5. **Commit** the board and `<board>.lint.json` together, on a clean branch.
   The lint file is a design artifact and changes are reviewed like code.

## Report

In the handoff, give open findings by severity before and after, each new
waiver with its reason, what you pruned, and anything still `changed` or
`unverified`. Link the contact sheets when you generated them.

Lint is advisory. KiCad ERC/DRC/parity and the `quality` stage still decide
done-ness (see `pcb-workflow`).
