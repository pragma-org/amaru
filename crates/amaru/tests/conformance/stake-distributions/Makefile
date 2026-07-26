.PHONY: help zst unzst list download upload check-aws-env check-network-dir

THIS_MAKEFILE := $(lastword $(MAKEFILE_LIST))

NETWORKS := preview preprod mainnet
NETWORK := $(notdir $(CURDIR))
IS_NETWORK_DIR := $(filter $(NETWORK),$(NETWORKS))
BUCKET := cardano-stake-distributions/$(NETWORK)

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
	fi

check-network-dir:
	@test -n "$(IS_NETWORK_DIR)" || { \
		echo "This shared Makefile must be run from one of: $(NETWORKS)"; \
		echo "Example: cd preview && make <target>"; \
		exit 1; \
	}

zst: check-network-dir ## Compress individual .json files using zstd compression
	@set -- *.json; \
	if [ ! -e "$$1" ]; then \
		exit 0; \
	fi; \
	for file in "$$@"; do \
		zstd -f -q --rm "$$file"; \
	done

unzst: check-network-dir ## Uncompress individual .json.zst files using zstd compression
	@set -- *.json.zst; \
	if [ ! -e "$$1" ]; then \
		exit 0; \
	fi; \
	for file in "$$@"; do \
		zstd -d -f -q --rm "$$file"; \
	done

check-aws-env:
	@test -n "$(ENDPOINT)" || { echo "Missing ENDPOINT"; exit 1; }
	@test -n "$(AWS_ACCESS_KEY_ID)" || { echo "Missing AWS_ACCESS_KEY_ID"; exit 1; }
	@test -n "$(AWS_SECRET_ACCESS_KEY)" || { echo "Missing AWS_SECRET_ACCESS_KEY"; exit 1; }

list: check-network-dir check-aws-env ## List all objects in the network-specific S3 bucket
	@aws s3 ls --recursive --human-readable --summarize --endpoint-url "$(ENDPOINT)" "s3://$(BUCKET)/"

download: check-network-dir check-aws-env ## Download missing 'epoch_*.json.zst' fixtures from the network-specific S3 bucket
	@remote_files=$$(aws s3 ls --recursive --endpoint-url "$(ENDPOINT)" "s3://$(BUCKET)/" | awk '{print $$4}' | sed 's@^[^/]*/@@' | grep '^epoch_.*\.json\.zst$$' || true); \
	if [ -z "$$remote_files" ]; then \
		echo "No epoch_*.json.zst files found in s3://$(BUCKET)/"; \
		exit 0; \
	fi; \
	downloaded=0; \
	skipped=0; \
	for file in $$remote_files; do \
		json_file=$${file%.zst}; \
		if [ -e "$$file" ] || [ -e "$$json_file" ]; then \
			skipped=$$((skipped + 1)); \
			continue; \
		fi; \
		aws s3 cp --endpoint-url "$(ENDPOINT)" "s3://$(BUCKET)/$$file" "$$file"; \
		downloaded=$$((downloaded + 1)); \
	done; \
	echo "Downloaded $$downloaded file(s); skipped $$skipped already present locally."

upload: check-network-dir check-aws-env ## Upload missing local 'epoch_*.json.zst' fixtures to the network-specific S3 bucket
	@set -- epoch_*.json.zst; \
	if [ ! -e "$$1" ]; then \
		echo "No epoch_*.json.zst files found"; \
		exit 0; \
	fi; \
	remote_files=$$(aws s3 ls --recursive --endpoint-url "$(ENDPOINT)" "s3://$(BUCKET)/" | awk '{print $$4}' | sed 's@^[^/]*/@@' | grep '^epoch_.*\.json\.zst$$' || true); \
	uploaded=0; \
	skipped=0; \
	for file in "$$@"; do \
		if printf '%s\n' "$$remote_files" | grep -qx "$$file"; then \
			skipped=$$((skipped + 1)); \
			continue; \
		fi; \
		aws s3 cp --endpoint-url "$(ENDPOINT)" "$$file" "s3://$(BUCKET)/$$file"; \
		uploaded=$$((uploaded + 1)); \
	done; \
	echo "Uploaded $$uploaded file(s); skipped $$skipped already present in the bucket."
