test_no_color_if_no_terminal() {
	assert_fail "start_misconfigured_amaru_run_cmd | has_escape_sequence" \
		"Found escape characters in output but we're not running in a terminal"
}

test_color_if_in_terminal() {
	assert "emulate_terminal "$start_misconfigured_amaru_run_cmd" | has_escape_sequence" \
		"No escape characters found in output but we're in a terminal and expecting some colors"
}

setup_suite() {
	AMARU_NETWORK=preprod
	LEDGER_DIR=./ledger.${AMARU_NETWORK}.db
	CHAIN_DIR=./chain.${AMARU_NETWORK}.db
	BUILD_PROFILE=dev
	UNREACHABLE_PEER=127.0.0.1:65532
	export start_misconfigured_amaru_run_cmd="cargo run --color never --profile ${BUILD_PROFILE} -- run
		--peer-address ${UNREACHABLE_PEER}
		--ledger-dir ${LEDGER_DIR}
		--chain-dir ${CHAIN_DIR}"
}

emulate_terminal() {
	expect -c "spawn -noecho $@; expect eof { exit 0 }"
}

has_escape_sequence() {
	grep $'\033'
}

start_misconfigured_amaru_run() {
	$start_misconfigured_amaru_run_cmd 2>&1 || true
}