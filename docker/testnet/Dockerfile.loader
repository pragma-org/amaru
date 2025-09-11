FROM docker.io/debian@sha256:8810492a2dd16b7f59239c1e0cc1e56c1a1a5957d11f639776bd6798e795608b

# Set time zone
ENV TZ="UTC"
RUN ln -snf /usr/share/zoneinfo/${TZ} /etc/localtime && \
    echo ${TZ} > /etc/timezone

RUN apt update && apt install -y jq coreutils bc
COPY . /data/
RUN chmod 0755 /data/cardano-loader.sh
RUN chmod 0755 /data/amaru-loader.sh

# Create cardano group and user compatible with running cardano-node
RUN groupadd --gid 10000 cardano && \
    useradd --comment 'cardano' --create-home --gid 10000 --password '!' --shell '/bin/bash' --uid 10000 cardano
