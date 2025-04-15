NETWORK ?= preprod
AMARU_PEER_ADDRESS ?= 127.0.0.1:3000
HASKELL_NODE_CONFIG_DIR ?= cardano-node-config
DEMO_TARGET_EPOCH ?= 173
HASKELL_NODE_CONFIG_SOURCE := https://book.world.dev.cardano.org/environments
COVERAGE_DIR ?= coverage
COVERAGE_CRATES ?=

.PHONY: help bootstrap run import-snapshots import-headers import-nonces download-haskell-config

all: help

help:
	@echo "\033[1;4mTargets:\033[00m"
	@grep -E '^[a-zA-Z0-9 -]+:.*##'  Makefile | sort | while read -r l; do printf "  \033[1;32m$$(echo $$l | cut -f 1 -d':')\033[00m:$$(echo $$l | cut -f 3- -d'#')\n"; done
	@echo ""
	@echo "\033[1;4mConfiguration:\033[00m"
	@grep -E '^[a-zA-Z0-9_]+ \?= '  Makefile | sort | while read -r l; do printf "  \033[36m$$(echo $$l | cut -f 1 -d'=')\033[00m=$$(echo $$l | cut -f 2- -d'=')\n"; done

snapshots: ## Download snapshots
	mkdir -p $@
	curl -s -o - "https://raw.githubusercontent.com/pragma-org/amaru/refs/heads/main/data/snapshots.json" \
		| jq -r '.[] | "\(.point) \(.url)"' \
		| while read p u; do \
			echo "Fetching $$p.cbor"; \
			curl --progress-bar -o - $$u | gunzip > $@/$$p.cbor; \
		done

download-haskell-config: ## Download Cardano Haskell configuration for $NETWORK
	mkdir -p $(HASKELL_NODE_CONFIG_DIR)
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/config.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/topology.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/byron-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/shelley-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/alonzo-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/conway-genesis.json

import-snapshots: snapshots ## Import PreProd snapshots for demo
	cargo run -- import-ledger-state \
	--snapshot $^/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912.cbor \
	--snapshot $^/69638382.5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7.cbor \
	--snapshot $^/70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d.cbor

import-headers: ## Import headers from $AMARU_PEER_ADDRESS for demo
	cargo run -- import-headers \
	--peer-address ${AMARU_PEER_ADDRESS} \
	--starting-point 69638365.4ec0f5a78431fdcc594eab7db91aff7dfd91c13cc93e9fbfe70cd15a86fadfb2 \
	--count 2
	cargo run -- import-headers \
	--peer-address ${AMARU_PEER_ADDRESS} \
	--starting-point 70070331.076218aa483344e34620d3277542ecc9e7b382ae2407a60e177bc3700548364c \
	--count 2

import-nonces: ## Import PreProd nonces for demo
	cargo run -- import-nonces \
	--at 70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d \
	--active a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b50a060a7a \
	--candidate 74fe03b10c4f52dd41105a16b5f6a11015ec890a001a5253db78a779fe43f6b6 \
	--evolving 24bb737ee28652cd99ca41f1f7be568353b4103d769c6e1ddb531fc874dd6718 \
	--tail 5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7

bootstrap: import-headers import-nonces import-snapshots ## Bootstrap the node

dev: ## Compile and run for development with default options
	cargo run -- daemon --peer-address=$(AMARU_PEER_ADDRESS) --network=$(NETWORK)

test-e2e: ## Run snapshot tests, assuming snapshots are available.
	cargo test -p amaru -- --ignored

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

demo: ## Synchronize Amaru until a target epoch $DEMO_TARGET_EPOCH
	./scripts/demo.sh $(AMARU_PEER_ADDRESS) $(DEMO_TARGET_EPOCH) $(NETWORK)

build-examples: ## Build all examples
	@for dir in $(wildcard examples/*/.); do \
		if [ -f $$dir/Makefile ]; then \
			echo "Building $$dir"; \
			$(MAKE) -C $$dir; \
			if [ $$? != "0" ]; then \
				exit $$?; \
			fi; \
		fi; \
	done

all-ci-checks: ## Run all CI checks
	@cargo fmt-amaru
	@cargo clippy-amaru
	@cargo test-amaru
	@$(MAKE) build-examples
