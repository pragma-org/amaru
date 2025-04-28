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
on the [preprod](https://book.world.dev.cardano.org/env-preprod.html) network (see bottom of page for more info).

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

2. Import the snapshots just downloaded:

```console
cargo run --release -- import-ledger-state \
  --snapshot snapshots/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912.cbor \
  --snapshot snapshots/69638382.5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7.cbor \
  --snapshot snapshots/70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d.cbor
```

3. Import chain data from remote peer:

> [!TIP]
>
> You only need two headers to bootstrap Amaru:
>
> - The last header of your imported snapshots
> - The last header of the epoch before your last imported epoch
>
> Although, importing _more_ headers won't hurt. So you can do a single
> synchronization of an entire epoch (~21600 blocks on good days, likely less
> on PreProd and Preview networks).

```console
cargo run --release -- import-headers \
  --peer-address 127.0.0.1:3000 \
  --starting-point 69638365.4ec0f5a78431fdcc594eab7db91aff7dfd91c13cc93e9fbfe70cd15a86fadfb2 \
  --count 21600
```

4. Import VRF nonces information

You need nonces states corresponding to the last header from the snapshots
(i.e. the last block of PreProd's epoch 165 → `70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d`):

```console
cargo run --release -- import-nonces \
  --at 70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d \
  --active a7c4477e9fcfd519bf7dcba0d4ffe35a399125534bc8c60fa89ff6b50a060a7a \
  --candidate 74fe03b10c4f52dd41105a16b5f6a11015ec890a001a5253db78a779fe43f6b6 \
  --evolving 24bb737ee28652cd99ca41f1f7be568353b4103d769c6e1ddb531fc874dd6718 \
  --tail 5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7
```

5. _(Optional)_ Setup observability backends:

```console
docker-compose -f monitoring/jaeger/docker-compose.yml up
```

6. Run the node:

```console
cargo run --release -- daemon \
  --peer-address=127.0.0.1:3000 \
  --network=preprod
```

> [!TIP]
> Replace `--peer-address` with your Cardano node peer address. It can be either
> a local or remote node (i.e. any existing node relay), and you can even add multiple peers.

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

## Spinning up a Cardano node on `preprod`

One option to do this is by using the docker image mentioned above:

```sh
docker pull ghcr.io/intersectmbo/cardano-node:8.9.1
```

(you may want to check available versions)
This image requires two volumes:

- `cardano-data:/data` for the blockchain db
- `cardano-ipc:/ipc` for IPC with other tools

Since we want to use network connections to interact with this node, you’ll want to expose port 3001, at least on `localhost`.

The final and crucial ingredient is to supply the environment variable `NETWORK=preprod`.
With this, the node with start up, fetch the required config files, and then start syncing blocks from the network; this will take some time.

## Using radicle 
 
Amaru is also compatible with Radicle, a decentralized code collaboration platform. This allows developers to collaborate on Amaru's development in a decentralized manner.
 
Follow the [radicle user guide](https://radicle.xyz/guides/user) to create a local clone of the Amaru repository, which you can use to collaborate with other developers on the project.

