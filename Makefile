NETWORK ?= preprod
HASKELL_NODE_CONFIG_DIR ?= cardano-node-config
HASKELL_NODE_CONFIG_SOURCE := https://book.world.dev.cardano.org/environments

.PHONY: help bootstrap run import-snapshots import-headers import-nonces download-haskell-config
all: help

help:
	@echo "Build and publish playground components"
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n \033[36m\033[0m\n"} /^[0-9a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-25s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

snapshots: ## Download snapshots
	mkdir -p $@
	curl -s -o - "https://raw.githubusercontent.com/pragma-org/amaru/refs/heads/main/data/snapshots.json" \
		| jq -r '.[] | "\(.point) \(.url)"' \
		| while read p u; do \
			echo "Fetching $$p.cbor"; \
			curl --progress-bar -o - $$u | gunzip > $@/$$p.cbor; \
		done

download-haskell-config: ## Download Cardano Haskell configuration for ${NETWORK}
	mkdir -p $(HASKELL_NODE_CONFIG_DIR)
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/config.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/topology.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/byron-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/shelley-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/alonzo-genesis.json
	curl -O --output-dir $(HASKELL_NODE_CONFIG_DIR) $(HASKELL_NODE_CONFIG_SOURCE)/$(NETWORK)/conway-genesis.json

import-snapshots: snapshots ## Import snapshots
	cargo run --release -- import-ledger-state \
	--snapshot $^/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912.cbor \
	--snapshot $^/69638382.5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7.cbor \
	--snapshot $^/70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d.cbor

import-headers: enforce-peer-address ## Import headers from ${AMARU_PEER_ADDRESS}
	cargo run --release -- import-headers \
	--peer-address ${AMARU_PEER_ADDRESS} \
	--starting-point 69638365.4ec0f5a78431fdcc594eab7db91aff7dfd91c13cc93e9fbfe70cd15a86fadfb2 \
	--count 2
	cargo run --release -- import-headers \
	--peer-address ${AMARU_PEER_ADDRESS} \
	--starting-point 70070331.076218aa483344e34620d3277542ecc9e7b382ae2407a60e177bc3700548364c \
	--count 2

import-nonces: ## Import nonces
	cargo run --release -- import-nonces \
	--at 70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d \
	--active a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b50a060a7a \
	--candidate 74fe03b10c4f52dd41105a16b5f6a11015ec890a001a5253db78a779fe43f6b6 \
	--evolving 24bb737ee28652cd99ca41f1f7be568353b4103d769c6e1ddb531fc874dd6718 \
	--tail 5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7

bootstrap: import-headers import-nonces import-snapshots ## Bootstrap the node

enforce-peer-address:
	@if [ -z ${AMARU_PEER_ADDRESS} ]; then \
		echo "Error: AMARU_PEER_ADDRESS environment variable is not set."; \
		exit 1; \
	fi

run: enforce-peer-address ## Run the node
	AMARU_TRACE=amaru=debug cargo run --release -- --with-json-traces daemon --peer-address=${AMARU_PEER_ADDRESS} ${AMARU_RUN_EXTRA}
