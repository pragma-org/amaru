# Amaru demos

Each demo lives in its own directory with a `process-compose.sh` wrapper and a `process-compose.yaml`
process definition, and reuses the shared machinery in [`common/`](common):

- `common.sh` small shell utilities.
- `cardano-cli.sh` the pinned cardano-cli release download.
- `cardano-node.sh` cardano-node configuration, queries, tool download, and the upstream node process.
- `amaru.sh` Amaru log parsing and sync waiting.
- `amaru-node.sh` building and running Amaru nodes.
- `databases.sh` Amaru database bootstrapping from the public snapshot CDN and database
  synchronization into per-node run directories.
- `tx.sh` transaction generation, submission, and wallet preparation.
- `telemetry.sh` OTLP collector detection and Grafana URL builders.
- `watch.sh` colorized log following.
- `orchestration.sh` process-compose file generation and the up/down/status commands.

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

## Prerequisites

There are two ways to run a demo. Both need flox somewhere, so start there.

**Directly**, from this checkout of [amaru](https://github.com/pragma-org/amaru):

- [flox](https://flox.dev): the demos run inside the flox environment in this directory
  (`flox activate`), which provides every required tool (`process-compose`, `jq`, `curl`, `rsync`,
  `ripgrep`, `xxd`, `rustup`, bash and the GNU core utilities, among others); startup fails when
  the environment is not active

**In a container**, which the relay-1 demo ships as a single image:

- Docker to run it, and flox to *build* it: the image is the demo flox environment exported with
  `flox containerize`, which is how it ends up with the identical pinned toolset. Once an image
  exists, running it needs nothing but Docker, so one person can build it and hand it around.
  See [`relay-1/docker`](relay-1/docker/README.md).

In both cases Docker is also what runs the optional shared monitoring stack in
[`monitoring`](../../monitoring); the demos export to it but never start it themselves.

Everything else is downloaded automatically, each pinned and checksum-verified: the Amaru databases
are bootstrapped from the public snapshot CDN on `up`, while `setup` fetches the cardano-node
configuration files and a cardano-cli release to build transactions with. The much larger
cardano-node release is downloaded only for the opt-in local upstream mode, which needs the
`cardano-node` binary itself.

## Configuration

The variables below apply to every demo. Ports, node names, and the per-node `AMARU_<NAME>_*` values
are demo-specific; see each demo's README. Defaults written as `<demo>` depend on the demo directory.

### General

| Variable        | Default             | Description                                               |
|-----------------|---------------------|-----------------------------------------------------------|
| `AMARU_NETWORK`             | `preprod`           | Target network                                                          |
| `AMARU_DIR`                 | repository root     | Path to the amaru project directory                                     |
| `BUILD_PROFILE`             | `dev`               | Cargo profile used to build the Amaru node binary                       |
| `AMARU_NODE_BINARY`         | unset               | Prebuilt Amaru binary to use instead of building with cargo             |
| `AMARU_DEMO_SKIP_TOOL_CHECK`| `false`             | Skip the flox environment and required-tool preflight (not recommended) |
| `LOGDIR`                    | `/tmp/amaru-<demo>` | Directory for demo logs and downloaded cardano-node tools               |
| `RUNDIR`                    | `<demo dir>/run`    | Directory for generated files and per-node databases                    |

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
| `PUBLIC_UPSTREAM_PEER_ADDRESS`        | per-network public relay                                                                     | Public Cardano peer used when `CARDANO_UPSTREAM_MODE=public`                   |
| `UPSTREAM_PORT`                       | 3001                                                                                         | Port for the local cardano-node listener                                       |

In `public` mode (the default) no local cardano-node runs: the middle Amaru relay connects directly
to a well-known public relay (`backbone.cardano.iog.io` on mainnet, the `play.dev.cardano.org` relays
on preprod and preview), and the cardano-node release download is skipped entirely.
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
lovelace) so that each concurrent submit replica can claim its own input. Once the local cardano-node
is close to the network tip, it queries the wallet and, when too few clean outputs exist, consolidates
current UTxOs into one self-transaction recreating them, submits it upstream, clears the local UTxO
claim state, and waits until the new outputs are visible. The step is idempotent: with enough clean
outputs already in the wallet it only clears the claim state.

| Variable                                 | Default   | Description                                                         |
|------------------------------------------|-----------|---------------------------------------------------------------------|
| `TX_REFUEL_UTXO_COUNT`                   | 10        | Clean UTxOs created by `prepare-wallet`                             |
| `TX_REFUEL_OUTPUT_LOVELACE`              | 2000000   | Lovelace per clean refuel UTxO                                      |
| `TX_REFUEL_MAX_INPUTS`                   | 80        | Maximum current UTxOs to consume while preparing the wallet         |
| `TX_REFUEL_SELECTION`                    | `largest` | Pick `largest` or `smallest` UTxOs first while preparing the wallet |
| `TX_REFUEL_FORCE`                        | `false`   | Rebuild clean UTxOs even if enough already exist                    |
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
| `KOIOS_API_URL`                   | per-network Koios endpoint                            | Koios API base URL used when `TX_QUERY_SOURCE=koios`                                                                                                     |
| `KOIOS_TIMEOUT_SECONDS`           | 30                                                    | Timeout for individual Koios HTTP requests                                                                                                               |
| `TX_GENERATED_COUNT`              | 1                                                     | Number of runtime transactions to build per process                                                                                                      |
| `TX_QUERY_SOURCE`                 | `koios` when using public upstream, `local` otherwise | Source for UTxOs, protocol parameters, and upstream tip                                                                                                  |
| `TX_OUTPUT_LOVELACE`              | 1000000                                               | Lovelace sent to the self-transfer output                                                                                                                |
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
cd monitoring && docker compose up -d
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
