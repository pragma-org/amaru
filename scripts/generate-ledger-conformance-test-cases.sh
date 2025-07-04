#!/bin/bash

# Must match what is included in
# `crates/amaru/tests/evaluate_ledger_conformance_tests.rs`
DEST_PATH="crates/amaru/tests/generated_ledger_conformance_test_cases.incl"
if [[ ! -f "$DEST_PATH" ]]; then
    echo "Error: File $DEST_PATH does not exist"
    exit 1
fi

SNAPSHOTS_ROOT="cardano-blueprint/src/ledger/conformance-test-vectors/eras/conway/impl/dump"

# Check if snapshots directory exists
if [[ ! -d "$SNAPSHOTS_ROOT" ]]; then
    echo "Error: Directory $SNAPSHOTS_ROOT does not exist"
    exit 1
fi

# Iterate through network directories
for TEST_GROUP in "$SNAPSHOTS_ROOT"/*; do
    if [[ -d "$TEST_GROUP" ]]; then
        TEST_GROUP_DIR_NAME=$(basename "$TEST_GROUP")
        if [[ $TEST_GROUP_DIR_NAME == "pparams-by-hash" ]]; then
            continue
        fi
        
        # Iterate through files in network directory
        for FILE_PATH in "$TEST_GROUP"/*; do
            if [[ -f "$FILE_PATH" ]]; then
                FILE_NAME=$(basename "$FILE_PATH")
                TEST_CASES+=("$FILE_PATH")
            fi
        done
    fi
done

# Sort test cases
IFS=$'\n' TEST_CASES=($(sort <<<"${TEST_CASES[*]}"))
unset IFS

# Create/truncate the output file
> "$DEST_PATH"

# Write test cases to file
for TEST_CASE in "${TEST_CASES[@]}"; do
    echo "#[test_case(\"../../$TEST_CASE\")]" >> "$DEST_PATH"
done

# Append the test function content
cat >> "$DEST_PATH" << 'EOF'
#[allow(clippy::unwrap_used)]
pub fn compare_snapshot_test_case(snapshot: &str) {
    evaluate_vector(snapshot).unwrap()
}
EOF

echo "Generated test cases written to: $DEST_PATH"
