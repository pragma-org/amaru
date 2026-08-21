#!/usr/bin/env bash

# Downloads a published cardano-cli release binary for the container's architecture and verifies
# it against the release checksum manifest.
#
# Usage: fetch-cardano-cli.sh <version> <target-file>
#
# Runs on the build host inside the demo flox environment, for the same reasons as
# fetch-amaru-release.sh: a `RUN` in a flox-containerized image has neither the environment's PATH
# nor its CA bundle, and keeping the download here leaves the image build without network access.
#
# TARGET_ARCH (amd64|arm64) selects the architecture, defaulting to the host's.

set -euo pipefail

version="${1:?usage: fetch-cardano-cli.sh <version> <target-file>}"
target="${2:?usage: fetch-cardano-cli.sh <version> <target-file>}"

case "${TARGET_ARCH:-$(uname -m)}" in
  arm64 | aarch64) platform=aarch64-linux ;;
  x86_64 | amd64) platform=x86_64-linux ;;
  *) echo "error: unsupported architecture: ${TARGET_ARCH:-$(uname -m)}" >&2; exit 1 ;;
esac

base="https://github.com/IntersectMBO/cardano-cli/releases/download/cardano-cli-$version"
archive="cardano-cli-$version-$platform.tar.gz"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "[build] downloading $archive"
curl -fL --retry 3 --retry-delay 2 --progress-bar "$base/$archive" -o "$work/$archive"
curl -fsSL --retry 3 --retry-delay 2 "$base/cardano-cli-$version-sha256sums.txt" -o "$work/sha256sums.txt"
rg --no-filename "  $archive\$" "$work/sha256sums.txt" | (cd "$work" && sha256sum -c -)

# The archive holds a single binary named after its platform.
tar -xzf "$work/$archive" -C "$work"
mkdir -p "$(dirname "$target")"
install -m 0755 "$work/cardano-cli-$platform" "$target"
echo "[build] cardano-cli binary staged at $target"
