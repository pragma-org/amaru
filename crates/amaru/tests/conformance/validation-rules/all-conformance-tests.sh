#!/usr/bin/env bash

set -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LEDGER_TEST_DATA_PATH="${LEDGER_TEST_DATA_PATH:-$(cd "$SCRIPT_DIR/../../../../amaru-ledger/tests/data" && pwd)}"
PHASE_ONE_ROOT="$LEDGER_TEST_DATA_PATH/phase-one"

banner() {
  printf '\n\033[1;36m'
  printf '╔════════════════════════════════════════════════════════════════╗\n'
  printf '║  %-62s║\n' "$1"
  printf '╚════════════════════════════════════════════════════════════════╝'
  printf '\033[0m\n\n'
}

run_suite() {
  local dir="$1" title="$2" summary_file rc
  banner "$title"

  summary_file="$(mktemp)"

  cabal run -v0 exe:conformance -- validate-phase-one --test-directory "$PHASE_ONE_ROOT/$dir" 2>"$summary_file" |
    jq -r --arg root "$PHASE_ONE_ROOT" '
        def pad($n): . + (" " * ($n - length));
        if has("error") then
          (.path | ltrimstr($root + "/")) as $path
          | (.error.label // null) as $label
          | (.error | if has("label") then .error else . end) as $e
          | "\n\u001b[31m✗ \($path)\u001b[0m"
            + " \u001b[33m[\($e.type)]\u001b[0m"
            + (if $label then " \u001b[2m(\($label))\u001b[0m" else "" end)
            + "\n"
            + ( [ $e | del(.type) | to_entries[] | "    \(.key | pad(9)): \(.value)" ] | join("\n") )
            + "\n"
        else
          "\u001b[32m✓ \(.label)\u001b[0m" + (if .result == "PASS" then "" else " \(.result)" end)
        end
      '
  rc=$?

  cat "$summary_file" >&2
  rm -f "$summary_file"

  return $rc
}

echo "Building Test Runner..."
build_log="$(mktemp)"
if ! cabal build exe:conformance >"$build_log" 2>&1; then
  cat "$build_log" >&2
  rm -f "$build_log"
  exit 1
fi
rm -f "$build_log"

overall=0
run_suite pass "Tests that should PASS (valid transactions)" || overall=1
run_suite fail "Tests that should FAIL (invalid transactions)" || overall=1
exit "$overall"
