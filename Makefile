AMARU_NETWORK ?= preprod
AMARU_CONFIG_DIR ?= data
DATA_FOLDER := $(AMARU_CONFIG_DIR)/$(AMARU_NETWORK)
SNAPSHOTS_FILE := $(DATA_FOLDER)/snapshots.json
NONCES_FILE := $(DATA_FOLDER)/nonces.json
HEADERS_FILE := $(DATA_FOLDER)/headers.json
AMARU_PEER_ADDRESS ?= 127.0.0.1:3001
HASKELL_NODE_CONFIG_DIR ?= cardano-node-config
DEMO_TARGET_EPOCH ?= 182
HASKELL_NODE_CONFIG_SOURCE := https://book.world.dev.cardano.org/environments
COVERAGE_DIR ?= coverage
COVERAGE_CRATES ?=
AMARU_LISTEN_ADDRESS ?= 0.0.0.0:0
AMARU_LEDGER_DIR ?= ./ledger.$(AMARU_NETWORK).db
AMARU_CHAIN_DIR ?= ./chain.$(AMARU_NETWORK).db
BUILD_PROFILE ?= release

export AMARU_NETWORK AMARU_CONFIG_DIR AMARU_PEER_ADDRESS AMARU_LISTEN_ADDRESS AMARU_LEDGER_DIR AMARU_CHAIN_DIR

.PHONY: help bootstrap start import-ledger-state import-headers import-nonces download-haskell-config coverage-html coverage-lconv check-llvm-cov fetch-chain-headers dev

help:
	@echo "\033[1;4mGetting Started:\033[00m"
	@grep -E '^[a-z]+[^:]+:.*## &start '  Makefile | while read -r l; do printf "  \033[1;32m$$(echo $$l | cut -f 1 -d':')\033[00m:$$(echo $$l | cut -f 3- -d'#' | sed 's/^ \&start//')\n"; done
	@echo ""
	@echo "\033[1;4mBuilding & Running:\033[00m"
	@grep -E '^[a-z]+[^:]+:.*## &build '  Makefile | while read -r l; do printf "  \033[1;32m$$(echo $$l | cut -f 1 -d':')\033[00m:$$(echo $$l | cut -f 3- -d'#' | sed 's/^ \&build//')\n"; done
	@echo ""
	@echo "\033[1;4mDev & Testing:\033[00m"
	@grep -E '^[a-z]+[^:]+:.*## &test '  Makefile | while read -r l; do printf "  \033[1;32m$$(echo $$l | cut -f 1 -d':')\033[00m:$$(echo $$l | cut -f 3- -d'#' | sed 's/^ \&test//')\n"; done
	@echo ""
	@echo "\033[1;4mConfiguration:\033[00m"
	@grep -E '^[a-zA-Z0-9_]+ \?= '  Makefile | sort | while read -r l; do printf "  \033[36m$$(echo $$l | cut -f 1 -d'=')\033[00m=$$(echo $$l | cut -f 2- -d'=')\n"; done

bootstrap: clear-dbs ## &start Bootstrap Amaru from scratch (snapshots + headers + ledger-state + nonces)
	cargo run --profile $(BUILD_PROFILE) -- bootstrap \
		--config-dir $(AMARU_CONFIG_DIR) \
		--ledger-dir $(AMARU_LEDGER_DIR) \
		--chain-dir $(AMARU_CHAIN_DIR)

snapshots/$(AMARU_NETWORK): ## &start Download initial snapshots
	@if [ ! -f "${SNAPSHOTS_FILE}" ]; then echo "SNAPSHOTS_FILE not found: ${SNAPSHOTS_FILE}"; exit 1; fi;
	mkdir -p "$@"
	cat $(SNAPSHOTS_FILE) \
		| jq -r '.[] | "\(.point) \(.url)"' \
		| while read -r p u; do \
			echo "Fetching $$p.cbor"; \
			curl --progress-bar -o - "$$u" | gunzip > "$@/$$p.cbor"; \
		done

import-headers: ## &start Import initial headers from $AMARU_PEER_ADDRESS
	cargo run --profile $(BUILD_PROFILE) -- import-headers --config-dir "$(AMARU_CONFIG_DIR)"

import-snapshots: import-ledger-state # 'backward-compatibility'; might remove after a while.
import-ledger-state: snapshots/$(AMARU_NETWORK) ## &start Import initial ledger-state snapshots
	@SNAPSHOT_ARGS=""; \
	CBOR_FILES=$$(find "$^" -maxdepth 1 -name '*.cbor'); \
	if [ -z "$$CBOR_FILES" ]; then echo "No .cbor files found in $^"; exit 1; fi; \
	for SNAPSHOT in $(wildcard $^/*.cbor); do \
		SNAPSHOT_ARGS="$$SNAPSHOT_ARGS --snapshot $$SNAPSHOT"; \
	done; \
	cargo run --profile $(BUILD_PROFILE) -- import-ledger-state \
		--ledger-dir "$(AMARU_LEDGER_DIR)" \
		$$SNAPSHOT_ARGS

import-nonces: ## &start Import initial nonces
	@if [ ! -f "$(NONCES_FILE)" ]; then echo "NONCES_FILE not found: $(NONCES_FILE)"; exit 1; fi; \
	cargo run --profile $(BUILD_PROFILE) -- import-nonces \
		--chain-dir $(AMARU_CHAIN_DIR) \
		--at $$(jq -r .at $(NONCES_FILE)) \
		--active $$(jq -r .active $(NONCES_FILE)) \
		--candidate $$(jq -r .candidate $(NONCES_FILE)) \
		--evolving $$(jq -r .evolving $(NONCES_FILE)) \
		--tail $$(jq -r .tail $(NONCES_FILE))

download-haskell-config: ## &start Download Haskell node configuration files for $AMARU_NETWORK
	mkdir -p $(HASKELL_NODE_CONFIG_DIR)
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/config.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/topology.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/byron-genesis.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/shelley-genesis.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/alonzo-genesis.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/conway-genesis.json"

build: ## &build Compile for $BUILD_PROFILE
	cargo build --profile $(BUILD_PROFILE)

build-examples: ## &build Build all examples
	@for dir in $(wildcard examples/*/.); do \
		if [ -f $$dir/Makefile ]; then \
			echo "Building $$dir"; \
			$(MAKE) -C $$dir || exit; \
		fi; \
	done

dev: start # 'backward-compatibility'; might remove after a while.
start: ## &build Compile and run for $BUILD_PROFILE with default options
	cargo run --profile $(BUILD_PROFILE) -- run

demo: ## &build Synchronize Amaru until a target epoch $DEMO_TARGET_EPOCH on $AMARU_NETWORK
		./scripts/demo $(BUILD_PROFILE) $(AMARU_PEER_ADDRESS) $(AMARU_LISTEN_ADDRESS) $(DEMO_TARGET_EPOCH) $(AMARU_NETWORK)

clear-dbs: ## &build Clear the databases (!!)
	@test -n "$(AMARU_LEDGER_DIR)" -a -n "$(AMARU_CHAIN_DIR)"
	@rm -rf -- "$(AMARU_LEDGER_DIR)" "$(AMARU_CHAIN_DIR)"

all-ci-checks: ## &test Run all CI checks
	@cargo fmt-amaru
	@cargo clippy-amaru
	@cargo test-amaru
	@$(MAKE) build-examples
	@$(MAKE) coverage-lconv

fetch-chain-headers: $(AMARU_CONFIG_DIR)/$(AMARU_NETWORK)/ ## &test Fetch chain headers from the network
	cargo run --profile $(BUILD_PROFILE) -- fetch-chain-headers \
		--peer-address $(AMARU_PEER_ADDRESS) \
		--config-dir $(AMARU_CONFIG_DIR)

fetch-data: ## &test Fetch epoch data (dreps, pools, accounts, ...) from a Haskell node
	@npm --prefix data run fetch -- "$(AMARU_NETWORK)"

generate-test-snapshots: ## &test Generate test snapshots for test-e2e
	@npm --prefix conformance-tests run generate-all -- "$(AMARU_NETWORK)"
	@./scripts/generate-snapshot-test-cases

test-e2e: ## &test Run snapshot tests, assuming snapshots are available
	AMARU_NETWORK=$(AMARU_NETWORK) cargo test --profile $(BUILD_PROFILE) -p amaru -- --ignored

check-llvm-cov: ## &test Check if cargo-llvm-cov is installed, install if not
	@if ! cargo llvm-cov --version >/dev/null 2>&1; then \
		echo "cargo-llvm-cov not found. Installing..."; \
		cargo install cargo-llvm-cov; \
	else \
		echo "cargo-llvm-cov is already installed"; \
	fi

coverage-html: check-llvm-cov ## &test Run test coverage for Amaru
	cargo llvm-cov \
		--no-cfg-coverage \
		--html \
		--output-dir $(COVERAGE_DIR) $(foreach package,$(COVERAGE_CRATES), --package $(package))

coverage-lconv: ## &test Run test coverage for CI to upload to Codecov
	cargo llvm-cov \
		--all-features \
		--workspace \
		--lcov \
		--output-path lcov.info
