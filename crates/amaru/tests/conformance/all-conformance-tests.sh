#!/usr/bin/env bash

set -o pipefail

PHASE_ONE_ROOT="../../../amaru-ledger/tests/data/phase-one"

run_suite() {
  cabal run -v0 exe:conformance -- validate-phase-one --test-directory "$PHASE_ONE_ROOT/$1" \
    | jq -r '
        if has("error") then
          "\u001b[31m✗ \(.error.label // .path)\u001b[0m \(.path)\n  \(.error)"
        else
          "\u001b[32m✓ \(.label)\u001b[0m \(.result)"
        end
      '
}

echo "Building Test Runner..."
cabal build exe:conformance

echo "Running Tests..."
run_suite pass
run_suite fail
