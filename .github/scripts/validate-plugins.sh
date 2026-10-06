#!/usr/bin/env bash
# Validate and preview every plugin manifest in this repo with
# `agent-portal plugin validate` / `plugin preview`. Fails only on validation
# errors; warnings become annotations. Usage: validate-plugins.sh [agent-portal]
set -euo pipefail

bin="${1:-agent-portal}"
summary="${GITHUB_STEP_SUMMARY:-/dev/null}"
in_ci="${GITHUB_ACTIONS:-}"
failed=0

for manifest in */agent-portal-plugin.toml; do
  dir="$(dirname "$manifest")"
  report="$("$bin" plugin validate "$dir" --json || true)"
  if [ -z "$report" ]; then
    echo "::error file=$manifest::agent-portal plugin validate produced no report"
    failed=1
    continue
  fi
  errors="$(jq '.errors | length' <<<"$report")"
  warnings="$(jq '.warnings | length' <<<"$report")"
  echo "$dir: $errors error(s), $warnings warning(s)"
  if [ -n "$in_ci" ]; then
    jq -r --arg f "$manifest" \
      '(.errors[] | "::error file=\($f),title=\(.field)::\(.message)"),
       (.warnings[] | "::warning file=\($f),title=\(.field)::\(.message)")' <<<"$report"
  else
    jq -r '(.errors[] | "  error: \(.field): \(.message)"),
           (.warnings[] | "  warning: \(.field): \(.message)")' <<<"$report"
  fi
  [ "$errors" -eq 0 ] || failed=1

  {
    echo "### \`$dir\` — $errors error(s), $warnings warning(s)"
    echo
    echo '```'
    "$bin" plugin preview "$dir" || true
    echo '```'
    echo
  } >>"$summary"
done

exit "$failed"
