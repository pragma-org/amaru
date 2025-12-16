FROM docker.io/debian:stable-slim

# Metadata
LABEL org.opencontainers.image.source="https://github.com/pragma-org/amaru" \
      org.opencontainers.image.description="A fully open source node client for Cardano, written in Rust "

# Install TLS certificates
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Set time zone
ENV TZ="UTC"
ENV LANG=C.UTF-8
ENV LC_ALL=C.UTF-8
RUN ln -snf /usr/share/zoneinfo/${TZ} /etc/localtime \
 && echo ${TZ} > /etc/timezone

# Create a non-root amaru group and user
RUN groupadd --gid 10000 amaru \
 && useradd -u 10000 -g 10000 \
    --system \
    --shell '/usr/sbin/nologin' \
    --password '!' \
    --no-create-home \
    amaru

RUN mkdir -p /var/lib/amaru && chown amaru:amaru /var/lib/amaru \
 && mkdir -p /etc/amaru && chown amaru:amaru /etc/amaru

# Copy the pre-compiled binary
COPY --chown=amaru:amaru ./dist/bin/amaru /usr/local/bin/amaru
COPY --chown=amaru:amaru ./dist/etc/* /etc/amaru
RUN chmod 0755 /usr/local/bin/amaru

# Initialize entrypoint & default command
USER amaru
WORKDIR /var/lib/amaru
VOLUME ["/var/lib/amaru"]
ENTRYPOINT ["/usr/local/bin/amaru"]
CMD ["run"]
