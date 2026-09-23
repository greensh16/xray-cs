#!/usr/bin/env bash

set -euo pipefail

FORMAT="$INPUT_FORMAT"
MIN_SEV="$INPUT_MIN_SEVERITY"
FAIL_ON="$INPUT_FAIL_ON"
read -r -a PATH_ARGS <<< "$INPUT_PATHS"
CONFIG_ARGS=()
if [[ -n "$INPUT_CONFIG" ]]; then
  CONFIG_ARGS=(--config "$INPUT_CONFIG")
fi

SARIF_FILE="${GITHUB_WORKSPACE}/xray-results.sarif"

# Count diagnostics from the JSON envelope so `issues-found` is a real total
# rather than a 0/1 flag derived from the exit code.
COUNT_JSON=$(xray --format json --min-severity "${MIN_SEV}" --fail-on never \
  "${CONFIG_ARGS[@]}" -- "${PATH_ARGS[@]}" || true)
ISSUES=$(printf '%s' "$COUNT_JSON" | python3 -c \
  'import json,sys; print(json.load(sys.stdin)["summary"]["total"])' 2>/dev/null || echo 0)
echo "issues-found=${ISSUES}" >> "$GITHUB_OUTPUT"

# xray applies the severity gate itself, so every documented value of fail-on
# (hint | warning | error | never) is honoured.
CMD=(xray --format "${FORMAT}" --min-severity "${MIN_SEV}" --fail-on "${FAIL_ON}")
if [[ -n "$INPUT_CONFIG" ]]; then
  CMD+=(--config "$INPUT_CONFIG")
fi

EXIT_CODE=0
if [[ "$FORMAT" == "sarif" ]]; then
  "${CMD[@]}" -- "${PATH_ARGS[@]}" > "$SARIF_FILE" || EXIT_CODE=$?
  echo "sarif-file=${SARIF_FILE}" >> "$GITHUB_OUTPUT"
else
  "${CMD[@]}" -- "${PATH_ARGS[@]}" || EXIT_CODE=$?
fi

# Exit code 2 is an internal error and always fails the step.
exit $EXIT_CODE
