# syntax=docker/dockerfile:1

# Relay-1 demo image: the demo flox environment exported with `flox containerize`
# (BASE_IMAGE), plus published amaru and cardano-cli releases and the demo scripts. Build it with
# ./build.sh, which containerizes the environment, resolves the versions and stages the binaries.
#
# Every other tool the image runs comes from the flox environment: curl, ripgrep, tar, coreutils
# and the rest are pinned by scripts/demos/.flox/env/manifest.toml, so this file installs nothing
# of its own and the build needs no network access.
#
# The build context is the repository root (to copy scripts/demos and the staged binaries); file
# exclusions live in Dockerfile.dockerignore next to this file.

ARG BASE_IMAGE=amaru-demos-flox:latest

# Final image. The flox base's entrypoint activates the demo environment and runs CMD
# inside it, so every tool resolves from the same pinned toolset as the host demo.
# build.sh downloads and checksum-verifies both binaries with the flox environment's own tools
# and stages them at the paths copied below. Both are statically linked, which matters here: the
# image has no dynamic loader for anything built against a distro libc.
FROM ${BASE_IMAGE}
COPY scripts/demos/relay-1/docker/.build/amaru /usr/local/bin/amaru
COPY scripts/demos/relay-1/docker/.build/cardano-cli /usr/local/bin/cardano-cli
COPY scripts/demos /opt/amaru/scripts/demos
# HOME lands on the data volume: process-compose looks for its configuration under
# $HOME/.config and the demo creates that directory on startup (see ensure_dirs).
# CARDANO_CLI is named explicitly for the same reason as AMARU_NODE_BINARY: the demo would otherwise
# take the first cardano-cli on PATH, and anything the flox environment happens to provide comes
# ahead of /usr/local/bin, silently replacing the pinned, checksum-verified binary staged below.
ENV HOME=/data \
    AMARU_DIR=/opt/amaru \
    AMARU_NODE_BINARY=/usr/local/bin/amaru \
    CARDANO_CLI=/usr/local/bin/cardano-cli \
    AMARU_NETWORK=preprod \
    CARDANO_UPSTREAM_MODE=public \
    RUNDIR=/data/run \
    LOGDIR=/data/logs \
    DOWNSTREAM_SUBMIT_API_ADDRESS=0.0.0.0:8091
# The collector of the shared monitoring stack (monitoring/docker-compose.yml) answers as
# otlp-collector once the container joins its network with `--network monitoring`.
# That flag is all it takes to get telemetry: the demo probes this endpoint and exports only
# when it answers (AMARU_DEMO_WITH_OPEN_TELEMETRY=auto), so a run without the network stays
# quiet instead of logging an export error per second.
ENV OTEL_EXPORTER_OTLP_ENDPOINT=http://otlp-collector:4317 \
    OTEL_EXPORTER_OTLP_METRICS_ENDPOINT=http://otlp-collector:4317
VOLUME ["/data"]
EXPOSE 8091 4001 4002
WORKDIR /opt/amaru/scripts/demos/relay-1
CMD ["bash", "./process-compose.sh", "up"]
