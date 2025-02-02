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

1. Download at least three [ledger snapshots](./data/README.md#cardano-ledger-snapshots):
```bash
mkdir -p snapshots;
curl -s -o - "https://raw.githubusercontent.com/pragma-org/amaru/refs/heads/main/data/snapshots.json" \
  | jq -r '.[] | "\(.point)  \(.url)"' \
  | while read p u ; do  \
      echo "Fetching $p.cbor"; \
      curl --progress-bar -o - $u \
        | gunzip > snapshots/$p.cbor ; \
    done
```

2. Import the snapshots just downloaded, either individually or a full directory

```console
cargo run --release -- import \
  --snapshot snapshots/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912.cbor

# OR
cargo run --release -- import \
  --snapshot snapshots/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912.cbor \
  --snapshot snapshots/70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d.cbor

# OR
cargo run --release -- import --snapshot-dir snapshots
```

3. Import chain data from remote peer

```console
cargo run --release -- import-chain-db \
  --peer-address 127.0.0.1:3000 --starting-point 69638382.5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7
```

> [!WARNING]
> This can take a while as the code is currently non-optimised.

3. Setup observability backends:

```console
docker-compose -f monitoring/jaeger/docker-compose.yml up
```

4. Run the node:

```console
AMARU_DEV_LOG=warn cargo run --release -- daemon \
  --peer-address=127.0.0.1:3000 \
  --network=preprod
```

> [!TIP]
> Replace `--peer-address` with your Cardano node peer address. It can be either
> a local or remote node (i.e. any existing node relay), and you can even add multiple peers.

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
