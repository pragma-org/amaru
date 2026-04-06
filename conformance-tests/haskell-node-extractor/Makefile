SHELL := /bin/sh

BREW_PREFIX := $(shell brew --prefix 2>/dev/null)
PREFIX ?= $(if $(BREW_PREFIX),$(BREW_PREFIX),/usr/local)
LIBDIR ?= $(PREFIX)/lib
INCLUDEDIR ?= $(PREFIX)/include
PKGCONFIGDIR ?= $(LIBDIR)/pkgconfig

CABAL ?= cabal
INSTALL ?= install
DEPS_DIR ?= .build/deps

SECP256K1_DIR := $(DEPS_DIR)/secp256k1
SECP256K1_REV := ac83be33d0956faf6b7f61a60ab524ef7d6a473a

BLST_DIR := $(DEPS_DIR)/blst
BLST_VERSION := v0.3.10
BLST_PC := $(BLST_DIR)/libblst.pc

.PHONY: all build run help blst secp256k1 pre

all: help

help:
	@printf "\033[1;37mMakefile Targets\033[00m\n"
	@grep -E '^[a-z]+[^:]+:.*## &cmd '  Makefile | sort | while read -r l; do printf "  \033[1;32m$$(echo $$l | cut -f 1 -d':')\033[00m:$$(echo $$l | cut -f 3- -d'#' | sed 's/^ \&cmd//')\n"; done
	@echo ""

pre: ## &cmd check required tools, libraries, and local build configuration
	@printf '%s\n' 'Checking build prerequisites'
	@set -eu; \
	os_name="$$(uname -s)"; \
	case "$$os_name" in \
		Darwin) install_cmd='brew install' ;; \
		Linux) install_cmd='sudo apt-get install' ;; \
		*) install_cmd='install manually' ;; \
	esac; \
	failed=0; \
	check_cmd() { \
		label="$$1"; \
		cmd="$$2"; \
		hint="$$3"; \
		if command -v "$$cmd" >/dev/null 2>&1; then \
			printf '\033[1;32m%s\033[00m %s\n' '  ✓' "$$label"; \
		else \
			printf '\033[1;31m%s %s\033[00m → \033[1;37m%s %s\033[00m\n' '  ✗' "$$label" "$$install_cmd" "$$hint"; \
			failed=1; \
		fi; \
	}; \
	check_pkg() { \
		label="$$1"; \
		hint="$$2"; \
		shift; \
		shift; \
		found=0; \
		for pkg_name in "$$@"; do \
			if pkg-config --exists "$$pkg_name"; then \
				found=1; \
				break; \
			fi; \
		done; \
		if [ "$$found" -eq 1 ]; then \
			printf '\033[1;32m%s\033[00m %s\n' '  ✓' "$$label"; \
		else \
			printf '\033[1;31m%s %s\033[00m → \033[1;37m%s %s\033[00m\n' '  ✗' "$$label" "$$install_cmd" "$$hint"; \
			failed=1; \
		fi; \
	}; \
	check_path() { \
		label="$$1"; \
		path="$$2"; \
		hint="$$3"; \
		if [ -e "$$path" ]; then \
			printf '%s %s\n' '  ✓' "$$label"; \
		else \
			printf '%s %s (%s)\n' '  ✗' "$$label" "$$hint"; \
			failed=1; \
		fi; \
	}; \
	check_cmd 'git' git git; \
	check_cmd 'cabal' cabal 'ghcup && ghcup tui'; \
	check_cmd 'ghc' ghc 'ghcup && ghcup tui'; \
	check_cmd 'pkg-config' pkg-config pkg-config; \
	check_cmd 'autoconf' autoconf autoconf; \
	check_cmd 'libtool' libtool libtool; \
	if [ "$$os_name" = 'Darwin' ]; then \
		check_cmd 'install_name_tool' install_name_tool CommandLineTools; \
	fi; \
	if command -v pkg-config >/dev/null 2>&1; then \
		check_pkg 'libffi' libffi libffi; \
		check_pkg 'gmp' gmp gmp; \
		check_pkg 'pcre' pcre libpcre pcre; \
		check_pkg 'libsodium' libsodium libsodium; \
		check_pkg 'zlib' zlib zlib; \
		check_pkg 'openssl' openssl openssl; \
		check_pkg 'secp256k1' libsecp256k1 libsecp256k1; \
		check_pkg 'blst' libblst libblst; \
	fi; \
	check_cmd 'llvm-config' llvm-config llvm; \
	if [ "$$os_name" = 'Linux' ]; then \
		check_path 'libnuma headers' '/usr/include/numa.h' 'sudo apt-get install libnuma-dev'; \
		check_path 'libsystemd headers' '/usr/include/systemd/sd-daemon.h' 'sudo apt-get install libsystemd-dev'; \
	fi; \

build: ## &cmd compile the main binary
	$(CABAL) build exe:haskell-node-extractor

secp256k1: ## &cmd install secp256k1 as required for Cardano
	mkdir -p "$(DEPS_DIR)"
	if [ ! -d "$(SECP256K1_DIR)" ]; then git clone https://github.com/bitcoin-core/secp256k1.git "$(SECP256K1_DIR)"; fi
	cd "$(SECP256K1_DIR)" && git fetch --all --tags
	cd "$(SECP256K1_DIR)" && git checkout "$(SECP256K1_REV)"
	cd "$(SECP256K1_DIR)" && ./autogen.sh
	cd "$(SECP256K1_DIR)" && ./configure --prefix="$(PREFIX)" --libdir="$(LIBDIR)" --enable-module-schnorrsig --enable-experimental
	$(MAKE) -C "$(SECP256K1_DIR)"
	$(MAKE) -C "$(SECP256K1_DIR)" check
	$(MAKE) -C "$(SECP256K1_DIR)" install
	if [ "$$(uname -s)" = "Darwin" ] && [ -f "$(LIBDIR)/libsecp256k1.0.dylib" ]; then install_name_tool -id "$(LIBDIR)/libsecp256k1.0.dylib" "$(LIBDIR)/libsecp256k1.0.dylib"; fi

blst: ## &cmd install blst as required for Cardano
	mkdir -p "$(DEPS_DIR)"
	if [ ! -d "$(BLST_DIR)" ]; then git clone https://github.com/supranational/blst "$(BLST_DIR)"; fi
	cd "$(BLST_DIR)" && git fetch --all --tags
	cd "$(BLST_DIR)" && git checkout "$(BLST_VERSION)"
	cd "$(BLST_DIR)" && ./build.sh
	mkdir -p "$(PKGCONFIGDIR)" "$(INCLUDEDIR)" "$(LIBDIR)"
	printf '%s\n' \
		'prefix=$(PREFIX)' \
		'exec_prefix=$${prefix}' \
		'libdir=$${exec_prefix}/lib' \
		'includedir=$${prefix}/include' \
		'Name: libblst' \
		'Description: Multilingual BLS12-381 signature library' \
		'URL: https://github.com/supranational/blst' \
		'Version: 0.3.10' \
		'Cflags: -I$${includedir}' \
		'Libs: -L$${libdir} -lblst' \
		> "$(BLST_PC)"
	$(INSTALL) -m 0644 "$(BLST_PC)" "$(PKGCONFIGDIR)/libblst.pc"
	$(INSTALL) -m 0644 "$(BLST_DIR)/bindings/blst_aux.h" "$(INCLUDEDIR)/blst_aux.h"
	$(INSTALL) -m 0644 "$(BLST_DIR)/bindings/blst.h" "$(INCLUDEDIR)/blst.h"
	$(INSTALL) -m 0644 "$(BLST_DIR)/bindings/blst.hpp" "$(INCLUDEDIR)/blst.hpp"
	$(INSTALL) -m 0644 "$(BLST_DIR)/libblst.a" "$(LIBDIR)/libblst.a"
