#!/usr/bin/env bash

PHASE_ONE_ROOT="../../../amaru-ledger/tests/data/phase-one"

run_suite() {
  kind=$1
  while IFS= read -r test_case; do
    cabal run -v0 exe:conformance -- validate-phase-one --test-case "$test_case"
  done < <(find "$PHASE_ONE_ROOT/$kind" -type f -name '*.json' | sort)
}

echo "Building Test Runner..."
cabal build exe:conformance

echo "Running Tests..."
run_suite pass
run_suite fail
