export AMARU_NETWORK ?= preprod
export AMARU_PEER_ADDRESS ?= 127.0.0.1:3001
HASKELL_NODE_CONFIG_DIR ?= cardano-node-config
DEMO_TARGET_EPOCH ?= 182
HASKELL_NODE_CONFIG_SOURCE := https://book.world.dev.cardano.org/environments
COVERAGE_DIR ?= coverage
COVERAGE_CRATES ?=
BUILD_PROFILE ?= release

.PHONY: help bootstrap start import-headers import-nonces download-haskell-config coverage-html coverage-lconv check-llvm-cov dev

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

bootstrap: ## &start Bootstrap Amaru from scratch (snapshots + headers + ledger-state + nonces)
	cargo run --profile $(BUILD_PROFILE) -- bootstrap $(ARGS)

sync-from-mithril:
	@cargo run --profile $(BUILD_PROFILE) --bin amaru-ledger mithril $(ARGS)
	@cargo run --profile $(BUILD_PROFILE) --bin amaru-ledger sync $(ARGS)

import-headers: ## &start Import initial headers
	cargo run --profile $(BUILD_PROFILE) -- import-headers $(ARGS)

import-nonces: ## &start Import initial nonces
	cargo run --profile $(BUILD_PROFILE) -- import-nonces $(ARGS)

download-haskell-config: ## &start Download Haskell node configuration files for $AMARU_NETWORK
	mkdir -p $(HASKELL_NODE_CONFIG_DIR)
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/config.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/topology.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/peer-snapshot.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/byron-genesis.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/shelley-genesis.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/alonzo-genesis.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_SOURCE)/$(AMARU_NETWORK)/conway-genesis.json"

build: ## &build Compile for $BUILD_PROFILE
	cargo build --profile $(BUILD_PROFILE) $(ARGS)

build-examples: ## &build Build all examples
	@for dir in $(wildcard examples/*/.); do \
		if [ -f $$dir/Makefile ]; then \
			echo "Building $$dir"; \
			$(MAKE) -C $$dir || exit; \
		fi; \
	done

dev: start # 'backward-compatibility'; might remove after a while.
start: ## &build Compile and run for $BUILD_PROFILE with default options
	cargo run --profile $(BUILD_PROFILE) -- run $(ARGS)

demo: ## &build Synchronize Amaru until a target epoch $DEMO_TARGET_EPOCH
		./scripts/demo $(BUILD_PROFILE) $(DEMO_TARGET_EPOCH)

all-ci-checks: ## &test Run all CI checks
	@cargo fmt-amaru
	@cargo clippy-amaru
	@cargo test --workspace --all-targets
	@$(MAKE) build-examples
	@$(MAKE) coverage-lconv

fetch-data: ## &test Fetch epoch data (dreps, pools, accounts, ...) from a Haskell node
	@npm --prefix data run fetch -- "$(AMARU_NETWORK)"

update-ledger-conformance-test-vectors: ## &test Update the set of test vectors used for ledger conformance tests
	@./scripts/update-ledger-conformance-test-vectors

update-ledger-conformance-test-snapshot: ## &test Update the snapshot of results from ledger conformance tests
	@./scripts/update-ledger-conformance-test-snapshot

generate-test-snapshots: ## &test Generate test snapshots for test-e2e
	@npm --prefix conformance-tests run generate-all -- "$(AMARU_NETWORK)"
	@./scripts/generate-snapshot-test-cases

test-e2e: ## &test Run snapshot tests, assuming snapshots are available
	cargo test --profile $(BUILD_PROFILE) -p amaru -- --ignored

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
