test_explains_snapshot_file_is_missing() {
	given_snapshots_file_is_missing

	assert_matches "data/preprod/snapshots.json.*NotFound" "$(bootstrap_amaru)"
}

setup() {
	export NETWORK=preprod
	export CONFIG_FOLDER=data
	export LEDGER_DIR=./ledger.${NETWORK}.db
	export CHAIN_DIR=./chain.${NETWORK}.db
	export BUILD_PROFILE=dev
	export AMARU_PEER_ADDRESS=w127.0.0.1:3001
}

given_snapshots_file_is_missing() {
	cd fixtures/missing_snapshots_file_setup
}

bootstrap_amaru() {
	cargo run --profile ${BUILD_PROFILE} -- bootstrap \
		--peer-address ${AMARU_PEER_ADDRESS} \
		--config-dir ${CONFIG_FOLDER} \
		--ledger-dir ${LEDGER_DIR} \
		--chain-dir ${CHAIN_DIR} \
		--network ${NETWORK} \
		2>&1
}