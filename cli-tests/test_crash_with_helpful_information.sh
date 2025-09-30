test_explains_snapshot_file_is_missing() {
	given_snapshots_file_is_missing

	assert_matches "data/preprod/snapshots.json.*NotFound" "$(bootstrap_amaru)"
}

skip_if "! ulimit -n" fd_limit
test_explains_fd_limit_is_too_low() {
	ulimit -n 256
	assert_matches "Increase the limit for open files before starting Amaru" "$(start_amaru)"
}

setup() {
	export AMARU_NETWORK=preprod
	export CONFIG_FOLDER=data
	export LEDGER_DIR=./ledger.${AMARU_NETWORK}.db
	export CHAIN_DIR=./chain.${AMARU_NETWORK}.db
	export BUILD_PROFILE=dev
	export UNREACHABLE_PEER=127.0.0.1:65532
}

given_snapshots_file_is_missing() {
	cd fixtures/missing_snapshots_file_setup
}

bootstrap_amaru() {
	cargo run --profile ${BUILD_PROFILE} -- bootstrap \
		--config-dir ${CONFIG_FOLDER} \
		--ledger-dir ${LEDGER_DIR} \
		--chain-dir ${CHAIN_DIR} \
		2>&1
}

start_amaru() {
	cargo run --profile ${BUILD_PROFILE} -- daemon \
		--peer-address ${UNREACHABLE_PEER} \
		--ledger-dir ${LEDGER_DIR} \
		--chain-dir ${CHAIN_DIR} \
		2>&1
}