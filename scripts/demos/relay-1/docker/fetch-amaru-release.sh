#!/usr/bin/env bash

# Downloads a published amaru release binary for the container's architecture and verifies it
# against the release checksum manifest.
#
# Usage: fetch-amaru-release.sh <version-without-v> <target-file>
#
# This runs on the build host inside the demo flox environment (curl, rg, tar, coreutils all
# come from there), rather than inside the image: a `RUN` in a flox-containerized image does not
# go through the activation entrypoint, so it has neither the environment's PATH nor its CA
# bundle. Keeping the download here also leaves the image build free of network access.

set -euo pipefail

version="${1:?usage: fetch-amaru-release.sh <version> <target-file>}"
target="${2:?usage: fetch-amaru-release.sh <version> <target-file>}"

# `flox containerize` builds an image for the host's architecture; amaru release assets are
# named after the machine architecture as Linux reports it.
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) echo "error: unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

base="https://github.com/pragma-org/amaru/releases/download/v$version"
archive="amaru-$version-linux-$arch.tar.gz"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "[build] downloading $archive"
curl -fL --progress-bar "$base/$archive" -o "$work/$archive"
curl -fsSL "$base/amaru-$version-checksums.manifest" -o "$work/checksums.manifest"
rg --no-filename " $archive\$" "$work/checksums.manifest" | (cd "$work" && sha256sum -c -)

tar -xzf "$work/$archive" -C "$work"
mkdir -p "$(dirname "$target")"
install -m 0755 "$work/amaru-$version-linux-$arch/bin/amaru" "$target"
echo "[build] amaru binary staged at $target"
