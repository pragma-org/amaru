# GLiveView

Docker setup for running [GLiveView](https://cardano-community.github.io/guild-operators/Scripts/gliveview/) alongside [Amaru](https://github.com/pragma-org/amaru).

GLiveView relies on prometheus metrics exposed by Amaru to display live chain and peer metrics.

## Prerequisites

- A running Amaru node with an accessible Prometheus metrics endpoint
- Docker installed

## Usage

Build the image:

```bash
make build
```

Build and run the interactive view:

```bash
make dev
```

The build defaults to `preprod`. Select another network by setting `AMARU_NETWORK`:

```bash
AMARU_NETWORK=preview make dev
```

## Environment Variables

Set runtime values before the Make target to override defaults:

```bash
PROM_HOST=192.168.1.10 PROM_PORT=8889 make dev
```

Docker defaults are defined in `env` and added to the upstream environment helper during the image build.

| Variable | Default | Description |
|---|---|---|
| `PROM_HOST` | `host.docker.internal` | Host running Amaru's Prometheus metrics |
| `PROM_PORT` | `8889` | Prometheus port exposed by the OTLP collector |
| `BLOCKLOG_DIR` | `/opt/cardano/blocklog` | Block log storage |
