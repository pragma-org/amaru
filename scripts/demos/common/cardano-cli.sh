#!/usr/bin/env bash

# Downloads the pinned cardano-cli release used to build and sign the demo transactions.
#
# cardano-cli publishes its own releases, one static binary per platform, so this is a small
# checksum-verified download. It is deliberately not a flox package: the only way to express it
# as one is through the cardano-node flake, and realizing that closure requires a machine which
# already trusts IOG's binary cache. Without it nix falls back to compiling GHC from source, so
# the demo worked on machines that happened to have that cache configured and nowhere else.
#
# Callers must set CARDANO_CLI_RELEASE_VERSION, CARDANO_CLI_HOME, CARDANO_CLI and LOGDIR.

CARDANO_CLI_RELEASE_BASE_URL="https://github.com/IntersectMBO/cardano-cli/releases/download"

# Release assets are named after the nix-style platform pair. There is no x86_64-darwin asset,
# so Intel Macs have to bring their own cardano-cli.
cardano_cli_archive_platform() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os:$arch" in
    Darwin:arm64 | Darwin:aarch64) echo "aarch64-darwin" ;;
    Linux:aarch64 | Linux:arm64) echo "aarch64-linux" ;;
    Linux:x86_64) echo "x86_64-linux" ;;
    *) die "no cardano-cli $CARDANO_CLI_RELEASE_VERSION release for $os/$arch; install cardano-cli yourself and set CARDANO_CLI" ;;
  esac
}

# Downloads, verifies and installs cardano-cli into CARDANO_CLI_HOME/bin. The archive holds a
# single binary named after its platform, which is renamed on the way in.
download_cardano_cli() {
  local platform archive_name archive_path checksums_path expected actual target
  local version="$CARDANO_CLI_RELEASE_VERSION"
  local base_url="$CARDANO_CLI_RELEASE_BASE_URL/cardano-cli-$version"

  platform="$(cardano_cli_archive_platform)"
  archive_name="cardano-cli-$version-$platform.tar.gz"
  archive_path="$LOGDIR/$archive_name"
  checksums_path="$LOGDIR/cardano-cli-$version-sha256sums.txt"
  target="$CARDANO_CLI_HOME/bin/cardano-cli"

  have curl || die "curl not found; cannot download cardano-cli $version"
  have tar || die "tar not found; cannot unpack cardano-cli $version"

  mkdir -p "$LOGDIR"
  if [[ ! -f "$archive_path" ]]; then
    echo "[setup] downloading $archive_name"
    curl -fL --progress-bar "$base_url/$archive_name" -o "$archive_path"
  else
    echo "[setup] using cached $archive_path"
  fi
  if [[ ! -f "$checksums_path" ]]; then
    curl -fsSL "$base_url/cardano-cli-$version-sha256sums.txt" -o "$checksums_path"
  fi

  expected="$(awk -v name="$archive_name" '$2 == name { print $1 }' "$checksums_path")"
  [[ -n "$expected" ]] || die "checksum for $archive_name not found in $checksums_path"
  actual="$(sha256sum "$archive_path" | awk '{ print $1 }')"
  [[ "$actual" == "$expected" ]] || die "checksum mismatch for $archive_path"

  local work
  work="$(mktemp -d)"
  tar -xzf "$archive_path" -C "$work"
  mkdir -p "$(dirname "$target")"
  install -m 0755 "$work/cardano-cli-$platform" "$target"
  rm -rf "$work"

  # Gatekeeper refuses freshly downloaded macOS binaries until they carry a signature.
  if [[ "$(uname -s)" == Darwin ]] && have codesign; then
    codesign --force --sign - "$target" >/dev/null 2>&1 || true
  fi
  echo "[setup] cardano-cli $version installed at $target"
}

# Makes sure CARDANO_CLI resolves to an executable, downloading the pinned release when it does
# not. A cardano-cli already on PATH, or one named explicitly, is used as it is.
ensure_cardano_cli() {
  if [[ -x "${CARDANO_CLI:-}" ]]; then
    echo "[setup] using cardano-cli at $CARDANO_CLI"
    return 0
  fi
  download_cardano_cli
}
