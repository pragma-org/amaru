export AMARU_NETWORK ?= preprod
export AMARU_PEER_ADDRESS ?= 127.0.0.1:3001
HASKELL_NODE_CONFIG_DIR ?= cardano-node-config
RUN_UNTIL_TARGET_EPOCH ?= 182
HASKELL_NODE_CONFIG_REPOSITORY := https://raw.githubusercontent.com/input-output-hk/cardano-playground
HASKELL_NODE_CONFIG_DIRECTORY := static/book.play.dev.cardano.org/environments
CARDANO_NODE_CONFIG_COMMIT := 791baff19a998a0cee840d6abbd8fcaa23e8f826
COVERAGE_DIR ?= coverage
COVERAGE_CRATES ?=
BUILD_PROFILE ?= release
TRACE_BASELINE ?= data/$(AMARU_NETWORK)/run-until-trace-baseline.jsonl
TRACE_COMPARE_LOG ?= trace-compare.log

.PHONY: help bootstrap start import-headers import-nonces download-haskell-config coverage-html coverage-lconv check-llvm-cov dev generate-traces-doc run-until compare-trace-contract update-trace-baseline

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
	cargo run --profile $(BUILD_PROFILE) -- $(COMMON_ARGS) bootstrap $(ARGS)

import-headers: ## &start Import initial headers
	cargo run --profile $(BUILD_PROFILE) -- $(COMMON_ARGS) import-headers $(ARGS)

import-nonces: ## &start Import initial nonces
	cargo run --profile $(BUILD_PROFILE) -- $(COMMON_ARGS) import-nonces $(ARGS)

download-haskell-config: ## &start Download Haskell node configuration files for $AMARU_NETWORK
	mkdir -p $(HASKELL_NODE_CONFIG_DIR)

	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_REPOSITORY)/$(CARDANO_NODE_CONFIG_COMMIT)/$(HASKELL_NODE_CONFIG_DIRECTORY)/$(AMARU_NETWORK)/alonzo-genesis.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_REPOSITORY)/$(CARDANO_NODE_CONFIG_COMMIT)/$(HASKELL_NODE_CONFIG_DIRECTORY)/$(AMARU_NETWORK)/byron-genesis.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_REPOSITORY)/$(CARDANO_NODE_CONFIG_COMMIT)/$(HASKELL_NODE_CONFIG_DIRECTORY)/$(AMARU_NETWORK)/config.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_REPOSITORY)/$(CARDANO_NODE_CONFIG_COMMIT)/$(HASKELL_NODE_CONFIG_DIRECTORY)/$(AMARU_NETWORK)/conway-genesis.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_REPOSITORY)/$(CARDANO_NODE_CONFIG_COMMIT)/$(HASKELL_NODE_CONFIG_DIRECTORY)/$(AMARU_NETWORK)/peer-snapshot.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_REPOSITORY)/$(CARDANO_NODE_CONFIG_COMMIT)/$(HASKELL_NODE_CONFIG_DIRECTORY)/$(AMARU_NETWORK)/shelley-genesis.json"
	curl -fsSL -O --output-dir "$(HASKELL_NODE_CONFIG_DIR)" "$(HASKELL_NODE_CONFIG_REPOSITORY)/$(CARDANO_NODE_CONFIG_COMMIT)/$(HASKELL_NODE_CONFIG_DIRECTORY)/$(AMARU_NETWORK)/topology.json"

build: ## &build Compile for $BUILD_PROFILE
	cargo build --profile $(BUILD_PROFILE) $(ARGS)

build-examples: ## &build Build all examples
	@for dir in $(wildcard examples/*/.); do \
		if [ -f $$dir/Makefile ]; then \
			echo "Building $$dir"; \
			$(MAKE) -C $$dir || exit; \
		fi; \
	done

sync-from-mithril: ## &build Fast synchronization from a Mithril snapshot, for $BUILD_PROFILE
	@cargo run --profile $(BUILD_PROFILE) --bin amaru-ledger $(COMMON_ARGS) mithril
	@cargo run --profile $(BUILD_PROFILE) --bin amaru-ledger $(COMMON_ARGS) sync

generate-traces-doc: ## &build Generate documentation for Amaru's tracing spans
	@./scripts/generate-traces-doc

dev: start # 'backward-compatibility'; might remove after a while.
start: ## &build Compile and run for $BUILD_PROFILE with default options
	cargo run --profile $(BUILD_PROFILE) -- $(COMMON_ARGS) run $(ARGS)

run-until: ## &build Synchronize Amaru until a target epoch $RUN_UNTIL_TARGET_EPOCH
		./scripts/run-until $(BUILD_PROFILE) $(RUN_UNTIL_TARGET_EPOCH)

compare-trace-contract: ## &test Compare $(TRACE_COMPARE_LOG) against $(TRACE_BASELINE)
	@if [ ! -f "$(TRACE_BASELINE)" ]; then \
		echo "No trace baseline found for $(AMARU_NETWORK), skipping trace contract check."; \
		exit 0; \
	fi
	@if [ ! -f "$(TRACE_COMPARE_LOG)" ]; then \
		echo "Missing trace log $(TRACE_COMPARE_LOG); run a traced run-until first." >&2; \
		exit 1; \
	fi
	@node scripts/compare-traces "$(TRACE_BASELINE)" "$(TRACE_COMPARE_LOG)"

update-trace-baseline: ## &test Refresh $(TRACE_BASELINE) from a traced run-until run
	@mkdir -p "$(dir $(TRACE_BASELINE))"
	@AMARU_TRACE=amaru=trace $(MAKE) run-until > "$(TRACE_BASELINE)"
	@echo "Updated $(TRACE_BASELINE)"

all-ci-checks: ## &test Run all CI checks
	@cargo fmt-amaru
	@cargo clippy-amaru
	@cargo test --workspace --all-targets
	@cargo test --doc
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
