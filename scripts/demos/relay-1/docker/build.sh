#!/usr/bin/env bash

# Builds the relay-1 demo image in two steps:
#   1. `flox containerize` exports the demo flox environment (scripts/demos/.flox) as a
#      base image, so the container runs the exact same pinned toolset as the host demo.
#   2. A thin Dockerfile layers the amaru binary and the demo scripts on top; every tool the
#      demo runs (cardano-cli included) comes from the flox environment itself.
#
# flox and docker are required to BUILD the image; running it only needs docker.
#
# The image always matches the build host's architecture, and cross-building is not possible:
# `flox containerize` has no architecture flag, and forcing its helper container to another
# platform fails because nix cannot load its seccomp filter under emulation. Both architectures
# are built on native runners instead, by
# .github/workflows/publish-relay-1-demo-image.yml.
#
# By default the image embeds a published amaru release, downloaded and checksum-verified by
# fetch-amaru-release.sh before the image build starts. Pass --local to compile the binary from
# this source tree instead (see Dockerfile.local); that takes considerably longer on a cold
# cargo cache.
#
# Configuration (flag, or environment variable):
#   --local | AMARU_LOCAL=true    compile amaru from this source tree instead of downloading a release
#   AMARU_VERSION                 amaru release version, without the leading v (default: latest release)
#   BASE_TAG                      tag for the containerized flox environment, reused when it already
#                                 exists (default: amaru-demos-flox:<digest of .flox/env/manifest.lock>)
#   IMAGE_TAG                     tag for the demo image (default: amaru-relay-1:latest)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMOS_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
AMARU_DIR="$(cd "$DEMOS_DIR/../.." && pwd)"

AMARU_LOCAL="${AMARU_LOCAL:-false}"
AMARU_VERSION="${AMARU_VERSION:-latest}"
# Keep in sync with the demo default in relay-1/process-compose.sh.
CARDANO_CLI_RELEASE_VERSION="${CARDANO_CLI_RELEASE_VERSION:-11.2.1.0}"

BASE_FALLBACK_TAG="${BASE_FALLBACK_TAG:-amaru-demos-flox:latest}"
IMAGE_TAG="${IMAGE_TAG:-amaru-relay-1:latest}"
LOCAL_TAG="${LOCAL_TAG:-amaru-relay-1-binary:latest}"

die() { echo "error: $*" >&2; exit 1; }

# Tagging the base image with a digest of the flox lock file makes reuse safe: a tag that exists is
# by construction an image built from the current environment, and any change to the lock produces a
# tag that does not exist yet.
FLOX_MANIFEST_LOCK="$DEMOS_DIR/.flox/env/manifest.lock"
[[ -f "$FLOX_MANIFEST_LOCK" ]] || die "flox lock file not found: $FLOX_MANIFEST_LOCK"
FLOX_ENV_DIGEST="$(sha256sum "$FLOX_MANIFEST_LOCK" | cut -c1-12)"
BASE_TAG="${BASE_TAG:-amaru-demos-flox:$FLOX_ENV_DIGEST}"

host_arch() {
  case "$(uname -m)" in
    arm64 | aarch64) echo arm64 ;;
    x86_64 | amd64) echo amd64 ;;
    *) die "unsupported host architecture: $(uname -m)" ;;
  esac
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --local) AMARU_LOCAL=true ;;
    # Prints the header comment, stopping at the first line that is not one: a hardcoded line range
    # silently truncates the help text whenever the header grows.
    -h | --help) awk 'NR >= 3 { if (!/^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
  shift
done

# A docker default platform other than the host's would make `flox containerize` fail deep inside
# nix, and would silently pair the base image with an amaru binary for the wrong architecture. Say
# so here instead: this script only ever builds for the host.
TARGET_ARCH="$(host_arch)"
export TARGET_ARCH
if [[ -n "${DOCKER_DEFAULT_PLATFORM:-}" && "$DOCKER_DEFAULT_PLATFORM" != "linux/$TARGET_ARCH" ]]; then
  die "DOCKER_DEFAULT_PLATFORM=$DOCKER_DEFAULT_PLATFORM but this host builds linux/$TARGET_ARCH images only;
  unset it, or build the other architecture on a native runner (see the publish workflow)"
fi

command -v flox >/dev/null 2>&1 || die "flox not found; it is required to containerize the demo environment"
command -v docker >/dev/null 2>&1 || die "docker not found"
for tool in curl jq rg tar sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool not found; run this script inside the demo flox environment"
done

case "$AMARU_LOCAL" in
  true | 1 | yes | on)
    echo "[build] amaru binary: compiled from the source tree at $AMARU_DIR"
    docker build \
      -f "$SCRIPT_DIR/Dockerfile.local" \
      --target binary \
      -t "$LOCAL_TAG" \
      "$AMARU_DIR"
    # The binary is copied out of a container rather than exported with `--output`: exporting
    # needs the build client to still be connected, which a long compilation can outlive.
    mkdir -p "$SCRIPT_DIR/.build"
    local_container="$(docker create "$LOCAL_TAG" /amaru)"
    docker cp "$local_container:/amaru" "$SCRIPT_DIR/.build/amaru"
    docker rm -f "$local_container" >/dev/null
    ;;
  false | 0 | no | off)
    AMARU_VERSION="$("$SCRIPT_DIR/resolve-amaru-version.sh" "$AMARU_VERSION")"
    echo "[build] amaru binary: release $AMARU_VERSION"
    "$SCRIPT_DIR/fetch-amaru-release.sh" "$AMARU_VERSION" "$SCRIPT_DIR/.build/amaru"
    ;;
  *) die "AMARU_LOCAL must be true or false" ;;
esac

echo "[build] cardano-cli binary: release $CARDANO_CLI_RELEASE_VERSION"
"$SCRIPT_DIR/fetch-cardano-cli.sh" "$CARDANO_CLI_RELEASE_VERSION" "$SCRIPT_DIR/.build/cardano-cli"

# The base image only has to be rebuilt when the flox environment changes, and containerizing is by
# far the slowest and most fragile step, so it is keyed on a digest of the lock file: an existing
# image for the current lock is reused, and a changed lock misses the tag and rebuilds.
if docker image inspect "$BASE_TAG" >/dev/null 2>&1; then
  echo "[build] reusing $BASE_TAG, the flox environment is unchanged"
else
  # The containerized image is named after the flox environment ("demos"); retag it so the
  # Dockerfile can reference a stable name.
  echo "[build] containerizing the demo flox environment"
  flox containerize -d "$DEMOS_DIR" --runtime docker --mode run
  docker tag demos:latest "$BASE_TAG"
fi
docker tag "$BASE_TAG" "$BASE_FALLBACK_TAG"

echo "[build] building $IMAGE_TAG"
docker build \
  -f "$SCRIPT_DIR/Dockerfile" \
  --build-arg BASE_IMAGE="$BASE_TAG" \
  -t "$IMAGE_TAG" \
  "$AMARU_DIR"

echo "[build] smoke-testing $IMAGE_TAG"
docker run --rm --entrypoint /usr/local/bin/amaru "$IMAGE_TAG" --version

# Asserts the version rather than just printing it, and asks the demo's own CARDANO_CLI rather than
# PATH: a cardano-cli inside the flox environment resolves ahead of /usr/local/bin and would
# otherwise replace the pinned, checksum-verified binary without a word.
cli_version="$(docker run --rm "$IMAGE_TAG" bash -c '"$CARDANO_CLI" --version' | head -1)"
echo "[build] $cli_version"
[[ "$cli_version" == *"cardano-cli $CARDANO_CLI_RELEASE_VERSION"* ]] ||
  die "the image resolves cardano-cli to '$cli_version', not the pinned $CARDANO_CLI_RELEASE_VERSION"

echo "[build] done: $IMAGE_TAG"
cat <<EOF
[build] run the demo in the Process Compose TUI with:

  docker run -it --name relay-1 -v amaru-relay-1:/data $IMAGE_TAG

[build] or in the background, and attach the TUI to it later:

  docker run -d --name relay-1 -v amaru-relay-1:/data $IMAGE_TAG
  docker exec -it relay-1 process-compose attach

[build] start the monitoring stack and join its network to get metrics, logs, and spans in
[build] Grafana at http://localhost; the demo detects the collector and exports on its own.

  docker compose -f $AMARU_DIR/monitoring/docker-compose.yml up -d --remove-orphans
  docker run -d --name relay-1 --network monitoring \\
    -v amaru-relay-1:/data $IMAGE_TAG
  docker exec -it relay-1 process-compose attach

[build] the demo submits its transactions from inside the container; add -p 8091:8091 to post them
[build] from the host, to the downstream node's submit API on http://localhost:8091
EOF
