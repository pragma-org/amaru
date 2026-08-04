#!/usr/bin/env bash

# Builds the relay-1 demo image in two steps:
#   1. `flox containerize` exports the demo flox environment (scripts/demos/.flox) as a
#      base image, so the container runs the exact same pinned toolset as the host demo.
#   2. A thin Dockerfile layers the amaru binary and the demo scripts on top; every tool the
#      demo runs (cardano-cli included) comes from the flox environment itself.
#
# flox and docker are required to BUILD the image; running it only needs docker.
# The produced image matches the build host's linux architecture.
#
# By default the image embeds a published amaru release, downloaded and checksum-verified by
# fetch-amaru-release.sh before the image build starts. Pass --local to compile the binary from
# this source tree instead (see Dockerfile.local); that takes considerably longer on a cold
# cargo cache.
#
# Configuration (flag, or environment variable):
#   --local | AMARU_LOCAL=true    compile amaru from this source tree instead of downloading a release
#   AMARU_VERSION                 amaru release version, without the leading v (default: latest release)
#   BASE_TAG                      tag for the containerized flox environment (default: amaru-demos-flox:latest)
#   IMAGE_TAG                     tag for the demo image (default: amaru-relay-1:latest)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMOS_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
AMARU_DIR="$(cd "$DEMOS_DIR/../.." && pwd)"

AMARU_LOCAL="${AMARU_LOCAL:-false}"
AMARU_VERSION="${AMARU_VERSION:-latest}"
BASE_TAG="${BASE_TAG:-amaru-demos-flox:latest}"
IMAGE_TAG="${IMAGE_TAG:-amaru-relay-1:latest}"
LOCAL_TAG="${LOCAL_TAG:-amaru-relay-1-binary:latest}"

die() { echo "error: $*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --local) AMARU_LOCAL=true ;;
    -h | --help) awk 'NR >= 3 && NR <= 20 { sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
  shift
done

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
    if [[ "$AMARU_VERSION" == latest ]]; then
      # Every amaru release is marked pre-release, so the /releases/latest endpoint returns 404;
      # take the most recent entry of the release list instead.
      tag="$(curl -fsSL 'https://api.github.com/repos/pragma-org/amaru/releases?per_page=1' | jq -r '.[0].tag_name // empty')"
      [[ -n "$tag" ]] || die "could not resolve the latest amaru release from the GitHub API"
      AMARU_VERSION="${tag#v}"
    fi
    echo "[build] amaru binary: release $AMARU_VERSION"
    "$SCRIPT_DIR/fetch-amaru-release.sh" "$AMARU_VERSION" "$SCRIPT_DIR/.build/amaru"
    ;;
  *) die "AMARU_LOCAL must be true or false" ;;
esac

# The containerized image is named after the flox environment ("demos"); retag it so the
# Dockerfile can reference a stable name.
echo "[build] containerizing the demo flox environment"
flox containerize -d "$DEMOS_DIR" --runtime docker --mode run
docker tag demos:latest "$BASE_TAG"

echo "[build] building $IMAGE_TAG"
docker build \
  -f "$SCRIPT_DIR/Dockerfile" \
  --build-arg BASE_IMAGE="$BASE_TAG" \
  -t "$IMAGE_TAG" \
  "$AMARU_DIR"

echo "[build] smoke-testing $IMAGE_TAG"
docker run --rm --entrypoint /usr/local/bin/amaru "$IMAGE_TAG" --version
docker run --rm "$IMAGE_TAG" cardano-cli --version

echo "[build] done: $IMAGE_TAG"
cat <<EOF
[build] run the demo in the Process Compose TUI with:

  docker run -it --name relay-1 -p 8091:8091 -v amaru-relay-1:/data $IMAGE_TAG

[build] or in the background, and attach the TUI to it later:

  docker run -d --name relay-1 -p 8091:8091 -v amaru-relay-1:/data $IMAGE_TAG
  docker exec -it relay-1 process-compose attach

[build] start the monitoring stack and join its network to get metrics, logs, and spans in
[build] Grafana at http://localhost; the demo detects the collector and exports on its own.

  (cd $AMARU_DIR/monitoring && docker compose up -d --remove-orphans)
  docker run -d --name relay-1 --network monitoring_default \\
    -p 8091:8091 -v amaru-relay-1:/data $IMAGE_TAG
  docker exec -it relay-1 process-compose attach

[build] the downstream node answers transaction submissions on http://localhost:8091
EOF
