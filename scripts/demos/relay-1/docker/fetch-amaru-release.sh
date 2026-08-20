#!/usr/bin/env bash

# Downloads a published amaru release binary for the container's architecture and verifies it
# against the release checksum manifest.
#
# Usage: fetch-amaru-release.sh <version-without-v> <target-file>
#
# TARGET_ARCH (amd64|arm64) selects the architecture, defaulting to the host's. It has to be
# explicit for a cross-build: the host architecture is the wrong answer whenever the image being
# assembled is not the host's, and the mismatch only surfaces as an exec format error at runtime.
#
# This runs on the build host inside the demo flox environment (curl, rg, tar, coreutils all
# come from there), rather than inside the image: a `RUN` in a flox-containerized image does not
# go through the activation entrypoint, so it has neither the environment's PATH nor its CA
# bundle. Keeping the download here also leaves the image build free of network access.

set -euo pipefail

version="${1:?usage: fetch-amaru-release.sh <version> <target-file>}"
target="${2:?usage: fetch-amaru-release.sh <version> <target-file>}"

# amaru release assets are named after the machine architecture as Linux reports it, which is
# not how docker spells it; accept either spelling on the way in.
case "${TARGET_ARCH:-$(uname -m)}" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) echo "error: unsupported architecture: ${TARGET_ARCH:-$(uname -m)}" >&2; exit 1 ;;
esac

base="https://github.com/pragma-org/amaru/releases/download/v$version"
archive="amaru-$version-linux-$arch.tar.gz"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "[build] downloading $archive"
curl -fL --retry 3 --retry-delay 2 --progress-bar "$base/$archive" -o "$work/$archive"
curl -fsSL --retry 3 --retry-delay 2 "$base/amaru-$version-checksums.manifest" -o "$work/checksums.manifest"
rg --no-filename " $archive\$" "$work/checksums.manifest" | (cd "$work" && sha256sum -c -)

tar -xzf "$work/$archive" -C "$work"
mkdir -p "$(dirname "$target")"
install -m 0755 "$work/amaru-$version-linux-$arch/bin/amaru" "$target"
echo "[build] amaru binary staged at $target"
