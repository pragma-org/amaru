FROM debian:bookworm-slim AS gliveview-builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/cardano

RUN git clone --depth 1 https://github.com/cardano-community/guild-operators.git \
    && cp guild-operators/scripts/cnode-helper-scripts/gLiveView.sh . \
    && cp guild-operators/scripts/cnode-helper-scripts/env . \
    && rm -rf guild-operators \
    && chmod +x gLiveView.sh

COPY patches /opt/cardano/patches
COPY env /opt/cardano/local-env
RUN git apply --check patches/*.patch \
    && git apply patches/*.patch \
    && sed -i '2r local-env' env

FROM debian:bookworm-slim

ARG AMARU_NETWORK=preprod

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        bc \
        ca-certificates \
        curl \
        gawk \
        iproute2 \
        jq \
        procps \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/cardano

COPY --from=gliveview-builder /opt/cardano/gLiveView.sh /opt/cardano/gLiveView.sh
COPY --from=gliveview-builder /opt/cardano/env /opt/cardano/env
RUN case "$AMARU_NETWORK" in mainnet|preprod|preview) ;; *) echo "Unknown network: $AMARU_NETWORK" >&2; exit 1 ;; esac \
    && mkdir -p /opt/cardano/files /opt/cardano/logs /opt/cardano/blocklog \
    && for file in config.json byron-genesis.json shelley-genesis.json alonzo-genesis.json conway-genesis.json; do \
        curl -sSfL "https://book.world.dev.cardano.org/environments/$AMARU_NETWORK/$file" -o "/opt/cardano/files/$file"; \
    done

ENTRYPOINT ["/opt/cardano/gLiveView.sh"]
