# Amaru

Amaru is a [Cardano](https://cardano.org) node client written in Rust. It is an ambitious project which aims to bring more diversity to the infrastructure operating the Cardano network.

## Getting Started

> [!WARNING]
>
> Amaru is still in an exploratory phase. Our development strategy favors early
> integration of components, so that progress is instantly visible, even though
> features might be limited or incomplete.

### Installing

```console
cargo build --release
```

### Running (demo)

> [!IMPORTANT]
> These instructions assume one starts from scratch, and has access to a running [cardano-node](https://github.com/IntersectMBO/cardano-node/)
on the [preprod](https://book.world.dev.cardano.org/env-preprod.html) network.
>
> To run a local peer, refer to [Cardano's developers portal](https://developers.cardano.org/docs/get-started/cardano-node/running-cardano).

1. Bootstrap the node:

```bash
make bootstrap
```

2. _(Optional)_ Setup observability backends:

```console
docker-compose -f monitoring/jaeger/docker-compose.yml up
```

3. Run Amaru:

```console
cargo run --release -- daemon \
  --peer-address=127.0.0.1:3000 \
  --listen-address=0.0.0.0:3001 \
  --network=preprod
```

Replace `--peer-address` with your Cardano node peer address. It can be either
a local or remote node (i.e. any existing node relay), and you can even add
multiple peers by replicating the option.

> [!TIP]
> To ensure logs are forwarded to telemetry backend, pass `--with-open-telemetry` as an option _before_ the `daemon` sub-command, eg.
>
> ```console
> cargo run --release -- --with-open-telemetry daemon \
>  --peer-address=127.0.0.1:3000 \
>  --network=preprod
> ```

### Monitoring

See [monitoring/README.md](./monitoring/README.md).

## Going further

Amaru is the integration point of several other projects / repositories. Amongst them, we find in particular:

| Repository                                                      | Purpose                                                                                                                                               |
| ---                                                             | ---                                                                                                                                                   |
| [txpipe/pallas](https://github.com/txpipe/pallas)               | Hosts many Rust primitives and building blocks for the node already powering tools like Dolos. In particular, the networking and serialization logic. |
| [pragma-org/uplc](https://github.com/pragma-org/uplc)           | A highly performant UPLC parser and CEK machine.                                                                                                      |

<hr/>

<p align="center">
  :boat: <a href="https://github.com/orgs/pragma-org/projects/1/views/1">Roadmap</a>
  |
  :triangular_ruler: <a href="CONTRIBUTING.md">Contributing</a>
  |
  <a href="https://discord.gg/3nZYCHW9Ns"><img src=".github/discord.svg" alt="Discord" /> Discord</a>
</p>
