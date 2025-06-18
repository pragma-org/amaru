NETWORK ?= preprod
CONFIG_FOLDER ?= data
DATA_FOLDER := $(CONFIG_FOLDER)/$(NETWORK)
SNAPSHOTS_FILE := $(DATA_FOLDER)/snapshots.json
NONCES_FILE := $(DATA_FOLDER)/nonces.json
HEADERS_FILE := $(DATA_FOLDER)/headers.json
AMARU_PEER_ADDRESS ?= 127.0.0.1:3001
HASKELL_NODE_CONFIG_DIR ?= cardano-node-config
DEMO_TARGET_EPOCH ?= 182
HASKELL_NODE_CONFIG_SOURCE := https://book.world.dev.cardano.org/environments
COVERAGE_DIR ?= coverage
COVERAGE_CRATES ?=
LISTEN_ADDRESS ?= 0.0.0.0:0
LEDGER_DIR ?= ./ledger.$(NETWORK).db
CHAIN_DIR ?= ./chain.$(NETWORK).db
BUILD_PROFILE ?= release

.PHONY: help bootstrap run import-snapshots import-headers import-nonces download-haskell-config coverage-html coverage-lconv check-llvm-cov

help:
	@echo "\033[1;4mTargets:\033[00m"
	@grep -E '^[a-zA-Z0-9 -]+:.*##'  Makefile | sort | while read -r l; do printf "  \033[1;32m$$(echo $$l | cut -f 1 -d':')\033[00m:$$(echo $$l | cut -f 3- -d'#')\n"; done
	@echo ""
	@echo "\033[1;4mConfiguration:\033[00m"
	@grep -E '^[a-zA-Z0-9_]+ \?= '  Makefile | sort | while read -r l; do printf "  \033[36m$$(echo $$l | cut -f 1 -d'=')\033[00m=$$(echo $$l | cut -f 2- -d'=')\n"; done

snapshots/$(NETWORK): ## Download snapshots
	@if [ ! -f "${SNAPSHOTS_FILE}" ]; then echo "SNAPSHOTS_FILE not found: ${SNAPSHOTS_FILE}"; exit 1; fi;
	mkdir -p $@
	cat $(SNAPSHOTS_FILE) \
		| jq -r '.[] | "\(.point) \(.url)"' \
		| while read p u; do \
			echo "Fetching $$p.cbor"; \
			curl --progress-bar -o - $$u | gunzip > $@/$$p.cbor; \
		done

import-snapshots: snapshots/$(NETWORK) ## Import snapshots for demo
	@SNAPSHOT_ARGS=""; \
	CBOR_FILES=$$(find "$^" -maxdepth 1 -name '*.cbor'); \
	if [ -z "$$CBOR_FILES" ]; then echo "No .cbor files found in $^"; exit 1; fi; \
	for SNAPSHOT in $(wildcard $^/*.cbor); do \
		SNAPSHOT_ARGS="$$SNAPSHOT_ARGS --snapshot $$SNAPSHOT"; \
	done; \
	cargo run --profile $(BUILD_PROFILE) -- import-ledger-state \
		--network $(NETWORK) \
		--ledger-dir "$(LEDGER_DIR)" \
		$$SNAPSHOT_ARGS

import-headers: ## Import headers from $AMARU_PEER_ADDRESS for demo
	@if [ ! -f "$(HEADERS_FILE)" ]; then echo "HEADERS_FILE not found: $(HEADERS_FILE)"; exit 1; fi; \
	HEADERS=$$(jq -r '.[]' $(HEADERS_FILE)); \
	for HEADER in $$HEADERS; do \
		cargo run --profile $(BUILD_PROFILE) -- import-headers \
			--network $(NETWORK) \
			--chain-dir $(CHAIN_DIR) \
			--peer-address $(AMARU_PEER_ADDRESS) \
			--starting-point $$HEADER \
			--count 2; \
	done

import-nonces: ## Import nonces for demo
	@if [ ! -f "$(NONCES_FILE)" ]; then echo "NONCES_FILE not found: $(NONCES_FILE)"; exit 1; fi; \
	cargo run --profile $(BUILD_PROFILE) -- import-nonces \
		--network $(NETWORK) \
		--chain-dir $(CHAIN_DIR) \
		--at $$(jq -r .at $(NONCES_FILE)) \
		--active $$(jq -r .active $(NONCES_FILE)) \
		--candidate $$(jq -r .candidate $(NONCES_FILE)) \
		--evolving $$(jq -r .evolving $(NONCES_FILE)) \
		--tail $$(jq -r .tail $(NONCES_FILE))

download-haskell-config: ## Download Cardano Haskell configuration for $NETWORK
	mkdir -p $(HASKELL_NODE_CONFIG_DIR)
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/config.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/topology.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/byron-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/shelley-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/alonzo-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/conway-genesis.json

clear-dbs: ## Clear the databases
	@rm -rf $(LEDGER_DIR) $(CHAIN_DIR)

bootstrap: clear-dbs ## Bootstrap the node from scratch
	cargo run --profile $(BUILD_PROFILE) -- bootstrap \
		--peer-address $(AMARU_PEER_ADDRESS) \
		--config-dir $(CONFIG_FOLDER) \
		--ledger-dir $(LEDGER_DIR) \
		--chain-dir $(CHAIN_DIR) \
		--network $(NETWORK)

dev: ## Compile and run for development with default options
	cargo run --profile $(BUILD_PROFILE) -- daemon \
		--ledger-dir $(LEDGER_DIR) \
		--chain-dir $(CHAIN_DIR) \
		--peer-address $(AMARU_PEER_ADDRESS) \
		--network=$(NETWORK) \
		--listen-address $(LISTEN_ADDRESS)

test-e2e: ## Run snapshot tests, assuming snapshots are available.
	cargo test --profile $(BUILD_PROFILE) -p amaru -- --ignored

test-e2e-from-scratch: bootstrap demo test-e2e ## Run end-to-end tests from scratch

check-llvm-cov: ## Check if cargo-llvm-cov is installed, install if not
	@if ! cargo llvm-cov --version >/dev/null 2>&1; then \
		echo "cargo-llvm-cov not found. Installing..."; \
		cargo install cargo-llvm-cov; \
	else \
		echo "cargo-llvm-cov is already installed"; \
	fi

coverage-html: check-llvm-cov ## Run test coverage for Amaru
	cargo llvm-cov \
		--no-cfg-coverage \
		--html \
		--output-dir $(COVERAGE_DIR) $(foreach package,$(COVERAGE_CRATES), --package $(package))

coverage-lconv: ## Run test coverage for CI to upload to Codecov
	cargo llvm-cov \
		--all-features \
		--workspace \
		--lcov \
		--output-path lcov.info

demo: ## Synchronize Amaru until a target epoch $DEMO_TARGET_EPOCH
	LEDGER_DIR=$(LEDGER_DIR) CHAIN_DIR=$(CHAIN_DIR) \
		./scripts/demo $(BUILD_PROFILE) $(AMARU_PEER_ADDRESS) $(LISTEN_ADDRESS) $(DEMO_TARGET_EPOCH) $(NETWORK)

build-examples: ## Build all examples
	@for dir in $(wildcard examples/*/.); do \
		if [ -f $$dir/Makefile ]; then \
			echo "Building $$dir"; \
			$(MAKE) -C $$dir || exit; \
		fi; \
	done

all-ci-checks: ## Run all CI checks
	@cargo fmt-amaru
	@cargo clippy-amaru
	@cargo test-amaru
	@$(MAKE) build-examples
	@$(MAKE) coverage-lconv
