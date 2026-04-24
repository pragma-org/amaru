export AMARU_NETWORK ?= preprod
export AMARU_PEER_ADDRESS ?= 127.0.0.1:3001
HASKELL_NODE_CONFIG_DIR ?= cardano-node-config
RUN_UNTIL_TARGET_EPOCH ?= 182
NODE_SNAPSHOT_DIRS ?=
HASKELL_NODE_CONFIG_REPOSITORY := https://raw.githubusercontent.com/input-output-hk/cardano-playground
HASKELL_NODE_CONFIG_DIRECTORY := static/book.play.dev.cardano.org/environments
CARDANO_NODE_CONFIG_COMMIT := 791baff19a998a0cee840d6abbd8fcaa23e8f826
COVERAGE_DIR ?= coverage
COVERAGE_CRATES ?=
BUILD_PROFILE ?= release
TRACES_PORT ?= 8000
TRACE_CONTRACT ?= data/$(AMARU_NETWORK)/run-until-trace-contract.json
TRACE_COMPARE_LOG ?= trace-compare.log
TRACE_COMPARE_SUMMARY_FILE ?= $${GITHUB_STEP_SUMMARY:-/dev/null}
TRACE_UPDATE_AMARU_TRACE ?= amaru=trace
TRACE_UPDATE_AMARU_TRACE_EMIT_PRIVATE ?= 1

ifeq (,$(findstring n,$(MAKEFLAGS)))
TRACE_SUMMARY_OUTPUT_ENABLED := 1
else
TRACE_SUMMARY_OUTPUT_ENABLED := 0
endif

.PHONY: help bootstrap start import-headers import-nonces download-haskell-config coverage-html coverage-lconv check-llvm-cov check-rust-toolchain-version dev generate-traces-doc run-until compare-trace-contract update-trace-contract generate-traces-doc serve-traces-doc validate-trace-schemas

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

bootstrap-hd: ## &start Bootstrap Amaru from one or more cardano-node UTxOHD snapshot directories (NODE_SNAPSHOT_DIRS=<path>[,<path>...])
	@if [ -z "$(NODE_SNAPSHOT_DIRS)" ]; then \
		echo "NODE_SNAPSHOT_DIRS is required, e.g. NODE_SNAPSHOT_DIRS=data-tmp/db/ledger/119183041 make $@"; \
		exit 1; \
	fi
	cargo run --profile $(BUILD_PROFILE) -- $(COMMON_ARGS) bootstrap-hd --node-snapshot-dirs $(NODE_SNAPSHOT_DIRS) $(ARGS)

import-headers: ## &start Import initial headers
	cargo run --profile $(BUILD_PROFILE) -- $(COMMON_ARGS) import-headers $(ARGS)

import-nonces: ## &start Import initial nonces
	cargo run --profile $(BUILD_PROFILE) -- $(COMMON_ARGS) import-nonces $(ARGS)

download-haskell-config: ## &start Download Haskell node configuration files for $AMARU_NETWORK
	mkdir -p $(HASKELL_NODE_CONFIG_DIR)
	@for file in alonzo-genesis.json byron-genesis.json config.json conway-genesis.json peer-snapshot.json shelley-genesis.json topology.json; do \
		curl -fsSL -o "$(HASKELL_NODE_CONFIG_DIR)/$$file" "$(HASKELL_NODE_CONFIG_REPOSITORY)/$(CARDANO_NODE_CONFIG_COMMIT)/$(HASKELL_NODE_CONFIG_DIRECTORY)/$(AMARU_NETWORK)/$$file"; \
	done

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

serve-traces-doc: generate-traces-doc ## &build Regenerate traces docs and serve docs/traces.html on http://127.0.0.1:$(TRACES_PORT)/traces.html
	@echo "Serving docs/traces.html at http://127.0.0.1:$(TRACES_PORT)/traces.html"
	@python3 -m http.server $(TRACES_PORT) --directory docs

validate-trace-schemas: ## &test Validate generated trace schemas against docs/traces-schema.json
	@cargo build --bin amaru --quiet
	@./target/debug/amaru dump-traces-schema 2> /tmp/schemas-current.json
	@./scripts/unused-schemas
	@set -eu; \
	jq -S 'walk(if type == "object" then del(.private) else . end)' docs/traces-schema.json > /tmp/expected.json; \
	jq -S 'walk(if type == "object" then del(.private) else . end)' /tmp/schemas-current.json > /tmp/current.json; \
	if diff -u /tmp/expected.json /tmp/current.json > /tmp/schemas.diff; then \
		echo "✓ Schemas are up-to-date"; \
	else \
		echo "::group::❌ Schema diff (expected → generated)"; \
		diff --color=always -u /tmp/expected.json /tmp/current.json || true; \
		echo "::endgroup::"; \
		echo "::error title=Schema out of date::Generated schema does not match docs/traces-schema.json"; \
		{ \
			echo "## ❌ Schema mismatch"; \
			echo ""; \
			echo "The generated schema differs from \`docs/traces-schema.json\`."; \
			echo ""; \
			echo "**How to fix:**"; \
			echo '```bash'; \
			echo './scripts/generate-traces-doc'; \
			echo '```'; \
		} >> "$${GITHUB_STEP_SUMMARY:-/dev/null}"; \
		exit 1; \
	fi

dev: start # 'backward-compatibility'; might remove after a while.
start: ## &build Compile and run for $BUILD_PROFILE with default options
	cargo run --profile $(BUILD_PROFILE) -- $(COMMON_ARGS) run $(ARGS)

run-until: ## &build Synchronize Amaru until a target epoch $RUN_UNTIL_TARGET_EPOCH
		./scripts/run-until $(BUILD_PROFILE) $(RUN_UNTIL_TARGET_EPOCH)

compare-trace-contract: ## &test Compare $(TRACE_COMPARE_LOG) against $(TRACE_CONTRACT) including performance thresholds
	@set -e; \
	if [ ! -f "$(TRACE_CONTRACT)" ]; then \
		echo "No trace contract found for $(AMARU_NETWORK), skipping trace contract check."; \
	elif [ ! -f "$(TRACE_COMPARE_LOG)" ]; then \
		echo "Missing trace log $(TRACE_COMPARE_LOG); run a traced run-until first." >&2; \
		exit 1; \
	else \
		if ! node scripts/compare-traces --summary-file "$(TRACE_COMPARE_SUMMARY_FILE)" "$(TRACE_CONTRACT)" "$(TRACE_COMPARE_LOG)"; then \
			echo "Warning: trace contract performance thresholds exceeded; see summary for details"; \
		fi; \
	fi

update-trace-contract: ## &test Refresh $(TRACE_CONTRACT) from a traced run-until run
	@mkdir -p "$(dir $(TRACE_CONTRACT))"
	@tmp_log="$$(mktemp)"; \
	AMARU_TRACE="$(TRACE_UPDATE_AMARU_TRACE)" AMARU_TRACE_EMIT_PRIVATE="$(TRACE_UPDATE_AMARU_TRACE_EMIT_PRIVATE)" $(MAKE) run-until > "$$tmp_log"; \
	node scripts/compare-traces --export-contract "$(TRACE_CONTRACT)" "$$tmp_log"; \
	if [ "$(TRACE_SUMMARY_OUTPUT_ENABLED)" = "1" ]; then \
		echo ""; \
		echo "Trace contract summary:"; \
		node scripts/compare-traces --summary-file /dev/stdout "$(TRACE_CONTRACT)" "$$tmp_log"; \
	else \
		echo "Dry-run mode: skipping trace contract summary generation."; \
	fi; \
	rm -f "$$tmp_log"
	@echo "Updated $(TRACE_CONTRACT)"

check-rust-toolchain-version: ## &test Verify rust-toolchain.toml and Cargo.toml rust-version stay aligned
	@./scripts/check-rust-toolchain-version

all-ci-checks: ## &test Run all CI checks
	@$(MAKE) check-rust-toolchain-version
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
