.PHONY: help xz unxz tar untar download upload check-aws-env

NETWORK := $(notdir $(CURDIR))
BUCKET := cardano-stake-distributions/$(NETWORK)
ARCHIVE := all.tar.zst
WHAT ?= tar

help:
	@echo "\033[1;4mCommands:\033[00m"
	@grep -E '^[a-z]+[^:]+:.*## '  ../Makefile | while read -r l; do printf "  \033[1;32m%s\033[00m:%s\n" "$$(echo $$l | cut -f 1 -d':')" "$$(echo $$l | cut -f 3- -d'#')"; done
	@echo ""
	@echo "\033[1;4mConfiguration:\033[00m"
	@grep -E '^[a-zA-Z0-9_]+ \?= '  ../Makefile | sort | while read -r l; do printf "  \033[36m%s\033[00m=%s\n" "$$(echo $$l | cut -f 1 -d'=')" "$$(echo $$l | cut -f 2- -d'=')"; done

xz: ## Compress individual .json files using xz compression
	@set -- *.json; \
	if [ ! -e "$$1" ]; then \
		exit 0; \
	fi; \
	for file in "$$@"; do \
		xz -f "$$file"; \
	done

unxz: ## Uncompress individual .json.xz files using xz compression
	@set -- *.json.xz; \
	if [ ! -e "$$1" ]; then \
		exit 0; \
	fi; \
	for file in "$$@"; do \
		xz -d -f "$$file"; \
	done

tar: ## Create a single space-optimised archive with all files
	@set -- *.json; \
	if [ ! -e "$$1" ]; then \
		exit 0; \
	fi; \
	tar --zstd -cf "$(ARCHIVE)" "$$@"

untar: ## Unpack a single space-optimised archive with all files
	@tar --zstd -xf "$(ARCHIVE)"

check-aws-env:
	@test -n "$(ENDPOINT)" || { echo "Missing ENDPOINT"; exit 1; }
	@test -n "$(AWS_ACCESS_KEY_ID)" || { echo "Missing AWS_ACCESS_KEY_ID"; exit 1; }
	@test -n "$(AWS_SECRET_ACCESS_KEY)" || { echo "Missing AWS_SECRET_ACCESS_KEY"; exit 1; }

download: check-aws-env ## Download an global archive or individual .xz from an S3 bucket
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

upload: check-aws-env ## Upload a global archive or individual .xz to an S3 bucket
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
