NETWORK ?= preprod
AMARU_PEER_ADDRESS ?= 127.0.0.1:3000
HASKELL_NODE_CONFIG_DIR ?= cardano-node-config
DEMO_TARGET_EPOCH ?= 182
HASKELL_NODE_CONFIG_SOURCE := https://book.world.dev.cardano.org/environments
COVERAGE_DIR ?= coverage
COVERAGE_CRATES ?=
LISTEN_ADDRESS ?= 0.0.0.0:0
LEDGER_DIR ?= ./ledger.db
CHAIN_DIR ?= ./chain.db

.PHONY: help bootstrap run import-snapshots import-headers import-nonces download-haskell-config  coverage-html coverage-lconv check-llvm-cov

help:
	@echo "\033[1;4mTargets:\033[00m"
	@grep -E '^[a-zA-Z0-9 -]+:.*##'  Makefile | sort | while read -r l; do printf "  \033[1;32m$$(echo $$l | cut -f 1 -d':')\033[00m:$$(echo $$l | cut -f 3- -d'#')\n"; done
	@echo ""
	@echo "\033[1;4mConfiguration:\033[00m"
	@grep -E '^[a-zA-Z0-9_]+ \?= '  Makefile | sort | while read -r l; do printf "  \033[36m$$(echo $$l | cut -f 1 -d'=')\033[00m=$$(echo $$l | cut -f 2- -d'=')\n"; done

snapshots: ## Download snapshots
	mkdir -p $@/${NETWORK}
	cat data/${NETWORK}/snapshots.json \
		| jq -r '.[] | "\(.point) \(.url)"' \
		| while read p u; do \
			echo "Fetching $$p.cbor"; \
			curl --progress-bar -o - $$u | gunzip > $@/${NETWORK}/$$p.cbor; \
		done

download-haskell-config: ## Download Cardano Haskell configuration for $NETWORK
	mkdir -p $(HASKELL_NODE_CONFIG_DIR)
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/config.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/topology.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/byron-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/shelley-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/alonzo-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/conway-genesis.json

import-snapshots: snapshots ## Import snapshots for demo
	SNAPSHOT_ARGS=""; \
	for SNAPSHOT in $^/${NETWORK}/*.cbor; do \
	  SNAPSHOT_ARGS="$$SNAPSHOT_ARGS --snapshot $$SNAPSHOT"; \
	done; \
	cargo run --release -- import-ledger-state \
	  --ledger-dir "$(LEDGER_DIR)" \
	  $$SNAPSHOT_ARGS

import-headers: ## Import headers from $AMARU_PEER_ADDRESS for demo
	@HEADERS=$$(jq -r '.[]' data/${NETWORK}/headers.json); \
	for HEADER in $$HEADERS; do \
		cargo run --release -- import-headers \
			--chain-dir $(CHAIN_DIR) \
			--peer-address ${AMARU_PEER_ADDRESS} \
			--starting-point $$HEADER \
			--count 2; \
	done

import-nonces: ## Import nonces for demo
	cargo run --release -- import-nonces \
	--chain-dir $(CHAIN_DIR) \
	--at 70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d \
	--active a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b50a060a7a \
	--candidate 74fe03b10c4f52dd41105a16b5f6a11015ec890a001a5253db78a779fe43f6b6 \
	--evolving 24bb737ee28652cd99ca41f1f7be568353b4103d769c6e1ddb531fc874dd6718 \
	--tail 5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7

clear-dbs: ## Clear the databases
	@rm -rf $(LEDGER_DIR) $(CHAIN_DIR)

bootstrap: clear-dbs import-headers import-nonces import-snapshots ## Bootstrap the node from scratch

dev: ## Compile and run for development with default options
	cargo run -- daemon \
	--ledger-dir $(LEDGER_DIR) \
	--chain-dir $(CHAIN_DIR) \
	--peer-address $(AMARU_PEER_ADDRESS) \
	--network=$(NETWORK) \
	--listen-address $(LISTEN_ADDRESS)

test-e2e: ## Run snapshot tests, assuming snapshots are available.
	cargo test --release -p amaru -- --ignored

test-e2-from-scratch: bootstrap demo test-e2e ## Run end-to-end tests from scratch

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
		./scripts/demo.sh $(AMARU_PEER_ADDRESS) $(LISTEN_ADDRESS) $(DEMO_TARGET_EPOCH) $(NETWORK)

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
