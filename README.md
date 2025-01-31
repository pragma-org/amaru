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

1. Import the first three [ledger snapshots](./data/README.md#cardano-ledger-snapshots):

```console
cargo run --release -- import \
  --snapshot 69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912.cbor

cargo run --release -- import \
  --snapshot 69638382.5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7.cbor

cargo run --release -- import \
  --snapshot 70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d.cbor
```

2. Setup observability backends:

```console
docker-compose -f monitoring/jaeger/docker-compose.yml up
```

3. Run the node:

```console
AMARU_DEV_LOG=warn cargo run --release -- daemon \
  --peer-address=127.0.0.1:3000 \
  --network=preprod
```

> [!TIP]
> Replace `--peer-address` with your Cardano node peer address. It can be either
> a local or remote node (i.e. any existing node relay).

### Monitoring

See [monitoring/README.md](./monitoring/README.md).

## Going further

Amaru is the integration point of several other projects / repositories. Amongst them, we find in particular:

| Repository                                                      | Purpose                                                                                                                                               |
| ---                                                             | ---                                                                                                                                                   |
| [txpipe/pallas](https://github.com/txpipe/pallas)               | Hosts many Rust primitives and building blocks for the node already powering tools like Dolos. In particular, the networking and serialization logic. |
| [pragma-org/ouroboros](https://github.com/pragma-org/ouroboros) | Rust libraries/building blocks to implement an Ouroboros (Praos & Genesis) consensus.                                                                 |
| [pragma-org/uplc](https://github.com/pragma-org/uplc)           | A highly performant UPLC parser and CEK machine.                                                                                                      |

<hr/>

<p align="center">
  :boat: <a href="https://github.com/orgs/pragma-org/projects/1/views/1">Roadmap</a>
  |
  :triangular_ruler: <a href="CONTRIBUTING.md">Contributing</a>
  |
  <a href="https://discord.gg/3nZYCHW9Ns"><img src=".github/discord.svg" alt="Discord" /> Discord</a>
</p>
