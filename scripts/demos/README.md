# Amaru demos

Each demo lives in its own directory, with a `process-compose.sh` wrapper and a
`process-compose.yaml` process definition, and reuses the shared machinery in [`common/`](common).

## Prerequisites

There are two ways to run a demo. Pick either.

**From a published image**, needing only Docker:

```bash
docker run -it -v amaru-relay-1:/data ghcr.io/pragma-org/amaru-relay-1-demo:latest
```

`:latest` is a manifest list, so Docker resolves the right image for the machine. The
architecture-specific `:amd64` and `:arm64` tags are there too, for the rare case of pinning one.
That image *is* the flox environment below, exported with `flox containerize`, so it runs the
identical pinned toolset. See [`relay-1/docker`](relay-1/docker/README.md).

**From this checkout** of [amaru](https://github.com/pragma-org/amaru), which is what you want in
order to run your own changes:

- [flox](https://flox.dev): the demos run inside the flox environment in this directory
  (`flox activate`), which provides every required tool (`process-compose`, `jq`, `curl`, `rsync`,
  `ripgrep`, `xxd`, `rustup`, bash and the GNU core utilities, among others); startup fails when
  the environment is not active

Building the image yourself needs `flox` too, since `flox containerize` is what produces its base.

In both cases Docker is also what runs the optional shared monitoring stack in
[`monitoring`](../../monitoring); the demos export to it but never start it themselves.

Everything else is downloaded automatically, each pinned and checksum-verified: the Amaru databases
are bootstrapped from the public snapshot CDN on `up`, while `setup` fetches the cardano-node
configuration files and a cardano-cli release to build transactions with. The much larger
cardano-node release is downloaded only for the opt-in local upstream mode, which needs the
`cardano-node` binary itself.

## Commands

Every demo wrapper supports the same commands:

```bash
./process-compose.sh up                  # start the demo (default command)
./process-compose.sh down                # stop the demo
./process-compose.sh status              # list the demo processes
./process-compose.sh refresh             # re-bootstrap the Amaru databases from the snapshot CDN
./process-compose.sh setup               # download cardano-cli and the cardano-node config
./process-compose.sh initialize          # validate config and prepare the run directories
./process-compose.sh telemetry-open      # open the demo's Grafana tabs
./process-compose.sh telemetry-urls      # print those Grafana URLs
./process-compose.sh run <process>       # run one process in the foreground
./process-compose.sh ready <process>     # readiness probe for one process
```

## Shared scripts

Everything in [`common/`](common) is sourced by a demo's `process-compose.sh`, which sets the
configuration variables each one expects. They are plain bash, with no state of their own beyond
the variables described under [Configuration](#configuration).

| Script                                        | What it provides                                                                                                                   |
|-----------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------|
| [`common.sh`](common/common.sh)               | Directory creation, `die`/`have`/`truthy` helpers, and the flox environment and required-tool preflight                            |
| [`orchestration.sh`](common/orchestration.sh) | `up`/`down`/`status`, and the per-network `process-compose.yaml` generated for the upstream mode                                   |
| [`amaru-node.sh`](common/amaru-node.sh)       | Resolving, building and running an Amaru node, plus its readiness probes                                                           |
| [`amaru.sh`](common/amaru.sh)                 | Parsing Amaru logs: adopted slot, network, and waiting for a node to reach a slot                                                  |
| [`databases.sh`](common/databases.sh)         | Bootstrapping the chain and ledger databases from the snapshot CDN, and copying them into per-node run directories                 |
| [`cardano-cli.sh`](common/cardano-cli.sh)     | Downloading and verifying the pinned cardano-cli release                                                                           |
| [`cardano-node.sh`](common/cardano-node.sh)   | cardano-node configuration and genesis checks, socket queries, the pinned release download, and the local upstream process         |
| [`tx.sh`](common/tx.sh)                       | Building, signing and submitting demo transactions, the claim state that keeps concurrent submitters apart, and wallet preparation |
| [`telemetry.sh`](common/telemetry.sh)         | Detecting whether the OTLP collector is reachable, and building the Grafana URLs                                                   |
| [`watch.sh`](common/watch.sh)                 | Following several logs at once, aligning their labels and highlighting transaction events                                          |

## Configuration

The variables below apply to every demo. Ports, node names, and the per-node `AMARU_<NAME>_*` values
are demo-specific; see each demo's README. Defaults written as `<demo>` depend on the demo directory.

### General

| Variable                     | Default             | Description                                                             |
|------------------------------|---------------------|-------------------------------------------------------------------------|
| `AMARU_NETWORK`              | `preprod`           | Target network                                                          |
| `AMARU_DIR`                  | repository root     | Path to the amaru project directory                                     |
| `BUILD_PROFILE`              | `dev`               | Cargo profile used to build the Amaru node binary                       |
| `AMARU_NODE_BINARY`          | unset               | Prebuilt Amaru binary to use instead of building with cargo             |
| `AMARU_DEMO_SKIP_TOOL_CHECK` | `false`             | Skip the flox environment and required-tool preflight (not recommended) |
| `LOGDIR`                     | `/tmp/amaru-<demo>` | Directory for demo logs and downloaded cardano-node tools               |
| `RUNDIR`                     | `<demo dir>/run`    | Directory for generated files and per-node databases                    |

### Upstream cardano-node

| Variable                              | Default                                                                                      | Description                                                                    |
|---------------------------------------|----------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------|
| `CARDANO_UPSTREAM_MODE`               | `public`                                                                                     | Use a public upstream peer, or a local cardano-node with `local`               |
| `CARDANO_NODE`                        | `$CARDANO_NODE_HOME/bin/cardano-node`                                                        | Path to the cardano-node executable                                            |
| `CARDANO_NODE_HOME`                   | `$LOGDIR/cardano-node-$CARDANO_NODE_RELEASE_VERSION`                                         | Directory containing `bin/cardano-node` and `bin/cardano-cli`                  |
| `CARDANO_NODE_RELEASE_VERSION`        | `11.0.1`                                                                                     | cardano-node release downloaded by `setup` when its tools are needed           |
| `CARDANO_NODE_SOCKET_TIMEOUT_SECONDS` | 1800                                                                                         | Wait for the cardano-node socket file to appear                                |
| `CARDANO_NODE_QUERY_TIMEOUT_SECONDS`  | 1800                                                                                         | Wait for the cardano-node socket to answer local queries                       |
| `CARDANO_CLI`                         | `cardano-cli` on `PATH`, else `$CARDANO_CLI_HOME/bin/cardano-cli`                            | Path to the cardano-cli executable                                             |
| `CARDANO_CLI_RELEASE_VERSION`         | `11.2.1.0`                                                                                   | cardano-cli release downloaded by `setup` when transactions are generated      |
| `CARDANO_CLI_HOME`                    | `$LOGDIR/cardano-cli-$CARDANO_CLI_RELEASE_VERSION`                                           | Where that download is installed, as `bin/cardano-cli`                         |
| `PUBLIC_UPSTREAM_PEER_ADDRESS`        | per-network public relay                                                                     | Public Cardano peers used when `CARDANO_UPSTREAM_MODE=public`, space-separated for several |
| `UPSTREAM_PORT`                       | 3001                                                                                         | Port for the local cardano-node listener                                       |

In `public` mode (the default) no local cardano-node runs: the middle Amaru relay connects directly
to a well-known public relay (`backbone.cardano.iog.io` on mainnet, the `play.dev.cardano.org` relays
on preprod and preview), and the cardano-node release download is skipped entirely.
`PUBLIC_UPSTREAM_PEER_ADDRESS` takes a whitespace-separated list, so the relay can be pointed at
several upstreams at once, each becoming its own `--peer-address`:

```bash
PUBLIC_UPSTREAM_PEER_ADDRESS="backbone.cardano.iog.io:3001 my-own-relay.example:3001"
```
`local` mode runs a cardano-node from the downloaded release; its
database starts empty (a full network synchronization) unless `$CARDANO_NODE_CONFIG_DIR/db` is
already populated. Custom testnets have no public relay and no published snapshots, so they need
`CARDANO_UPSTREAM_MODE=local` and `BOOTSTRAP_AMARU_DATABASES=false` with pre-seeded databases.

### Bootstrap and databases

The demo databases are bootstrapped with `amaru bootstrap`, which downloads the three most recent
epoch snapshots from the public snapshot CDN over HTTPS and imports them into fresh chain and ledger
databases under `AMARU_BOOTSTRAP_DIR`, together with a marker file recording the completed bootstrap.
Restarting a demo never re-bootstraps: the databases are reused as long as the marker is present, and
even an explicit refresh reuses the snapshot archives already downloaded into
`$AMARU_BOOTSTRAP_DIR/snapshots`. On startup, `initialize` copies the bootstrapped databases into
isolated per-node run directories. Each source marker stays beside its working database. When the marker matches the
current bootstrap, `initialize` preserves the advanced database. A new bootstrap replaces each working database on the
next startup.
Published snapshots exist for mainnet, preprod, and preview.

| Variable                     | Default                | Description                                                                       |
|------------------------------|------------------------|------------------------------------------------------------------------------------|
| `BOOTSTRAP_AMARU_DATABASES`  | `auto`                 | Bootstrap the Amaru databases when they are missing or incomplete                 |
| `FORCE_REFRESH`              | `false`                | Discard the bootstrapped databases and bootstrap again                            |
| `AMARU_BOOTSTRAP_DIR`        | `$RUNDIR/bootstrap`    | Directory containing the bootstrapped Amaru databases and the snapshot cache      |
| `AMARU_BOOTSTRAP_EPOCH`      | latest available       | Target bootstrap epoch (at least 3 published past epochs must exist)              |


### Wallet preparation

Before transactions are generated, the `prepare-wallet` step makes sure the payment address holds
enough clean UTxOs of a fixed size (`TX_REFUEL_UTXO_COUNT` outputs of `TX_REFUEL_OUTPUT_LOVELACE`
lovelace) so that each concurrent submit replica can claim its own input. It queries the wallet and,
when too few clean outputs exist, consolidates current UTxOs into one self-transaction recreating
them, submits it upstream, clears the local UTxO claim state, and waits until the new outputs are
visible. Running it again after a batch is what makes repeated submit runs possible.

It only spends a fee when it has to. The decision uses the threshold `submit-tx` itself applies when
selecting an input (`TX_OUTPUT_LOVELACE + TX_FEE_BUFFER_LOVELACE`), not the exact refuel size, so a
wallet whose outputs have merely been drained by earlier rounds is left alone: those outputs are still
spendable, and rebuilding them would cost a fee for nothing. Preparation happens only once too few
outputs clear that threshold. `TX_REFUEL_FORCE=true` overrides the decision, and
`TX_REFUEL_UTXO_COUNT=0` skips the step outright, which suits a single-transaction demo.

A wallet that cannot fund the requested outputs is a warning, not a failure: the existing UTxOs are
left untouched and `submit-tx` still runs with whatever is there.

It works in both upstream modes. With a local cardano-node it waits for
`TX_REFUEL_CARDANO_SYNC_PROGRESS` and lets the node balance and submit the transaction. With a public
upstream it queries Koios, calculates the fee and change itself from the protocol parameters, and
submits through the Koios `submittx` endpoint. Because the refuel outputs have to match
`TX_REFUEL_OUTPUT_LOVELACE` exactly for the demo to recognise them, any remainder becomes a separate
change output, or is added to the fee when it would be smaller than
`TX_REFUEL_MIN_CHANGE_LOVELACE`.

| Variable                                 | Default   | Description                                                         |
|------------------------------------------|-----------|---------------------------------------------------------------------|
| `TX_REFUEL_UTXO_COUNT`                   | 10        | Clean UTxOs `prepare-wallet` ensures are spendable; `0` skips it entirely |
| `TX_REFUEL_OUTPUT_LOVELACE`              | 2000000   | Lovelace per clean refuel UTxO                                      |
| `TX_REFUEL_MAX_INPUTS`                   | 80        | Maximum current UTxOs to consume while preparing the wallet         |
| `TX_REFUEL_SELECTION`                    | `largest` | Pick `largest` or `smallest` UTxOs first while preparing the wallet |
| `TX_REFUEL_FORCE`                        | `false`   | Rebuild clean UTxOs even if enough already exist                    |
| `TX_REFUEL_MIN_CHANGE_LOVELACE`          | 1000000   | Below this, the change is added to the fee instead of becoming a dust output |
| `TX_REFUEL_CONFIRM_TIMEOUT_SECONDS`      | 300       | Wait for clean refuel outputs to appear after wallet preparation    |
| `TX_REFUEL_CARDANO_SYNC_PROGRESS`        | 99.9      | Minimum cardano-node sync progress before preparing the wallet      |
| `TX_REFUEL_CARDANO_SYNC_TIMEOUT_SECONDS` | 14400     | Timeout for reaching the required cardano-node sync progress        |

### Transaction generation

Each `submit-tx` replica derives the payment address from the demo's payment signing key, queries its UTxOs and
the protocol parameters from the local cardano-node socket (or from Koios in public-upstream mode),
and claims a distinct spendable UTxO through on-disk claim state shared by all replicas. It builds a
self-transfer from the claimed input (or drains small inputs into a single output), signs it as
canonical CBOR, waits for the ledger of the Amaru node receiving the transaction to reach the slot
where the input exists, and submits it to that node's submit API, retrying rejections caused by
not-yet-visible inputs.
Accepted claims are kept for the rest of the run so replicas never double-spend an input; failed
transactions release their claims for other replicas.

| Variable                          | Default                                               | Description                                                                                                                                              |
|-----------------------------------|-------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------|
| `TX_PAYMENT_SKEY`                 | per-demo key lookup                                   | Signing key used for generated transactions, in cardano-cli or `cardano-address` `addr_xsk` format: either a path to it, or the key itself. The payment address is derived from it |
| `KOIOS_API_URL`                   | per-network Koios endpoint                            | Koios API base URL used when `TX_QUERY_SOURCE=koios`                                                                                                     |
| `KOIOS_TIMEOUT_SECONDS`           | 30                                                    | Timeout for individual Koios HTTP requests                                                                                                               |
| `TX_GENERATED_COUNT`              | 1                                                     | Number of runtime transactions to build per process                                                                                                      |
| `TX_QUERY_SOURCE`                 | `koios` when using public upstream, `local` otherwise | Source for UTxOs, protocol parameters, and upstream tip                                                                                                  |
| `TX_OUTPUT_LOVELACE`              | 1000000                                               | Lovelace sent to the self-transfer output                                                                                                                |
| `TX_METADATA_MESSAGE`             | `made by amaru with 💜`                                | Message attached to every generated transaction under metadata label 674 (CIP-20), which explorers render as a transaction comment; set empty to attach none. Max 64 **bytes**, so emoji count for several |
| `TX_FEE_BUFFER_LOVELACE`          | 300000                                                | Fee buffer used to skip UTxOs that are too small                                                                                                         |
| `TX_FEE_MARGIN_LOVELACE`          | 2000                                                  | Added to the calculated minimum fee: `calculate-min-fee` under-estimates the signed transaction by a few bytes, and the ledger rejects any fee below the minimum |
| `TX_SUBMIT_API_ADDRESS`           | the downstream node's submit API                      | Where the submitting processes post transactions; point it at another node's API to submit through that node instead                                      |
| `TX_BATCH_COUNT`                  | `TX_BATCH_DEFAULT_COUNT`                              | Number of transactions the batch process submits                                                                                                         |
| `TX_BATCH_DEFAULT_COUNT`          | 5                                                     | Batch size used when `TX_BATCH_COUNT` is unset and there is no terminal to prompt on, which is the case under process-compose                              |
| `TX_SYNC_TIMEOUT_SECONDS`         | 14400                                                 | Timeout for pre-submit sync waits                                                                                                                        |
| `TX_SYNC_POLL_INTERVAL_SECONDS`   | 15                                                    | Poll interval while waiting for Amaru to catch up                                                                                                        |
| `TX_SUBMIT_RETRY_LIMIT`           | 12                                                    | Submit attempts per transaction for retryable rejections                                                                                                 |
| `TX_SUBMIT_RETRY_DELAY`           | 30                                                    | Seconds between submit retries                                                                                                                           |


### Telemetry

The demos do not run a telemetry stack of their own. They export to the shared monitoring stack in
the [`monitoring`](../../monitoring) directory, which is started and stopped independently:

```bash
docker compose -f monitoring/docker-compose.yml up -d
```

That stack provisions the demo dashboards along with the rest, so nothing has to be layered onto it.
The nodes export metrics, logs, and spans over OTLP/gRPC, and `./process-compose.sh telemetry-open`
opens the matching Grafana tabs (`telemetry-urls` just prints them).

| Variable                              | Default                 | Description                                                              |
|---------------------------------------|-------------------------|--------------------------------------------------------------------------|
| `AMARU_DEMO_WITH_OPEN_TELEMETRY`      | `auto`                  | `auto` exports only when the collector answers; `true`/`false` force it   |
| `OTEL_EXPORTER_OTLP_ENDPOINT`         | `http://localhost:4317` | OTLP/gRPC endpoint for traces and logs                                   |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | `http://localhost:4317` | OTLP/gRPC endpoint for metrics                                            |
| `TELEMETRY_GRAFANA_URL`               | `http://localhost`      | Grafana URL opened by `telemetry-open`                                    |

`auto` is what makes running without the stack painless: the exporters would otherwise log a failure
on every export interval. It probes `OTEL_EXPORTER_OTLP_ENDPOINT` once at startup, so starting the
monitoring stack after the nodes needs a restart of the two node processes.

### Watch and readiness

| Variable      | Default  | Description                                             |
|---------------|----------|----------------------------------------------------------|
| `WATCH_COLOR` | `always` | Set `never` to disable ANSI colors in the watch process |

## Writing a new demo

A demo's `process-compose.sh` sets its configuration variables, sources the `common/` scripts, and
dispatches commands. Beyond the variables above, the shared scripts expect the demo to define:

- `DEMO_NAME` used in the marker files recording which bootstrap a database copy came from.
- `DEMO_LOG_FILES` the log files followed by the watch process, in label/color order.
- `PUBLIC_UPSTREAM_EXCLUDED_PROCESSES` processes removed from the generated `process-compose` file in
  `public-upstream` mode.
- `AMARU_<NAME>_*` variable blocks for each Amaru node, and thin `run_*`/`ready_*` wrappers calling
  `run_amaru_node`, `ready_amaru_node_listening`, and `ready_amaru_submit_api`.
- `validate_config` the demo-specific configuration checks run before startup and refreshes.
- `telemetry_urls` the Grafana URLs opened by `telemetry-open`.

See [`relay-1/process-compose.sh`](relay-1/process-compose.sh) for a complete example.
