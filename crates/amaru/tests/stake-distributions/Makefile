.PHONY: help xz unxz tar untar list download upload check-aws-env check-network-dir

THIS_MAKEFILE := $(lastword $(MAKEFILE_LIST))

NETWORKS := preview preprod mainnet
NETWORK := $(notdir $(CURDIR))
IS_NETWORK_DIR := $(filter $(NETWORK),$(NETWORKS))
BUCKET := cardano-stake-distributions/$(NETWORK)
ARCHIVE := all.tar.zst
WHAT ?= tar

help:
	@if [ -z "$(IS_NETWORK_DIR)" ]; then \
		echo "\033[1;4mDisclaimer:\033[00m\n"; \
		echo "  This Makefile is meant to be included or invoked from one of:\n"; \
		for network in $(NETWORKS); do \
			printf "    \033[1;32m%s\033[00m\n" "$$network"; \
		done; \
		echo ""; \
		echo "  Example:\n"; \
		printf "    cd preview && make help\n"; \
	else \
		echo "\033[1;4mCommands:\033[00m"; \
		grep -E '^[a-z]+[^:]+:.*## ' "$(THIS_MAKEFILE)" | while read -r l; do printf "  \033[1;32m%s\033[00m:%s\n" "$$(echo $$l | cut -f 1 -d':')" "$$(echo $$l | cut -f 3- -d'#')"; done; \
		echo ""; \
		echo "\033[1;4mConfiguration:\033[00m"; \
		grep -E '^[a-zA-Z0-9_]+ \?= ' "$(THIS_MAKEFILE)" | sort | while read -r l; do printf "  \033[36m%s\033[00m=%s\n" "$$(echo $$l | cut -f 1 -d'=')" "$$(echo $$l | cut -f 2- -d'=')"; done; \
	fi

check-network-dir:
	@test -n "$(IS_NETWORK_DIR)" || { \
		echo "This shared Makefile must be run from one of: $(NETWORKS)"; \
		echo "Example: cd preview && make <target>"; \
		exit 1; \
	}

xz: check-network-dir ## Compress individual .json files using xz compression
	@set -- *.json; \
	if [ ! -e "$$1" ]; then \
		exit 0; \
	fi; \
	for file in "$$@"; do \
		xz -f "$$file"; \
	done

unxz: check-network-dir ## Uncompress individual .json.xz files using xz compression
	@set -- *.json.xz; \
	if [ ! -e "$$1" ]; then \
		exit 0; \
	fi; \
	for file in "$$@"; do \
		xz -d -f "$$file"; \
	done

tar: check-network-dir ## Create a single space-optimised archive with all files
	@set -- *.json; \
	if [ ! -e "$$1" ]; then \
		exit 0; \
	fi; \
	tar --zstd -cf "$(ARCHIVE)" "$$@"

untar: check-network-dir ## Unpack a single space-optimised archive with all files
	@tar --zstd -xf "$(ARCHIVE)"

check-aws-env:
	@test -n "$(ENDPOINT)" || { echo "Missing ENDPOINT"; exit 1; }
	@test -n "$(AWS_ACCESS_KEY_ID)" || { echo "Missing AWS_ACCESS_KEY_ID"; exit 1; }
	@test -n "$(AWS_SECRET_ACCESS_KEY)" || { echo "Missing AWS_SECRET_ACCESS_KEY"; exit 1; }

list: check-network-dir check-aws-env ## List all objects in the network-specific S3 bucket
	@aws s3 ls --recursive --human-readable --summarize --endpoint-url "$(ENDPOINT)" "s3://$(BUCKET)/"

download: check-aws-env ## Download an global archive (WHAT=tar) or individual .xz (WHAT=xz) from an S3 bucket
	@case "$(WHAT)" in \
	tar) \
		aws s3 cp --endpoint-url "$(ENDPOINT)" "s3://$(BUCKET)/$(ARCHIVE)" "$(ARCHIVE)" \
		;; \
	xz) \
		aws s3 cp --recursive --endpoint-url "$(ENDPOINT)" --exclude "*" --include "epoch_*.json.xz" "s3://$(BUCKET)/" . \
		;; \
	*) \
		echo "WHAT must be one of: tar, xz"; \
		exit 1; \
		;; \
	esac

upload: check-aws-env ## Upload a global archive (WHAT=tar) or individual .xz (WHAT=xz) to an S3 bucket
	@case "$(WHAT)" in \
	tar) \
		test -e "$(ARCHIVE)" || { echo "Missing $(ARCHIVE)"; exit 1; }; \
		aws s3 cp --endpoint-url "$(ENDPOINT)" "$(ARCHIVE)" "s3://$(BUCKET)/$(ARCHIVE)" \
		;; \
	xz) \
		set -- epoch_*.json.xz; \
		if [ ! -e "$$1" ]; then \
			echo "No epoch_*.json.xz files found"; \
			exit 1; \
		fi; \
		aws s3 cp --recursive --endpoint-url "$(ENDPOINT)" --exclude "*" --include "epoch_*.json.xz" . "s3://$(BUCKET)/" \
		;; \
	*) \
		echo "WHAT must be one of: tar, xz"; \
		exit 1; \
		;; \
	esac
