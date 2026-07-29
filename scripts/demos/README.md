# Amaru demos

Each demo lives in its own directory with a `process-compose.sh` wrapper and a `process-compose.yaml`
process definition, and reuses the shared machinery in [`common/`](common):

- `common.sh` small shell utilities.
- `cardano-node.sh` cardano-node configuration, queries, tool download, and the upstream node process.
- `amaru.sh` Amaru log parsing and sync waiting.
- `amaru-node.sh` building and running Amaru nodes.
- `databases.sh` Mithril refreshes and database synchronization into per-node run directories.
- `tx.sh` transaction generation, submission, and wallet preparation.
- `telemetry.sh` the Grafana/Tempo/Prometheus/Loki stack and Grafana URL builders.
- `watch.sh` colorized log following.
- `orchestration.sh` process-compose file generation and the up/down/status commands.

## Commands

Every demo wrapper supports the same commands:

```bash
./process-compose.sh up                  # start the demo (default command)
./process-compose.sh down                # stop the demo and the telemetry stack
./process-compose.sh status              # list the demo processes
./process-compose.sh refresh             # refresh the Amaru databases from Mithril
./process-compose.sh setup               # download the cardano-node tools
./process-compose.sh initialize          # validate config and prepare the run directories
./process-compose.sh telemetry-up        # start only the telemetry stack
./process-compose.sh telemetry-down      # stop the telemetry stack
./process-compose.sh telemetry-open      # open the demo's Grafana tabs
./process-compose.sh run <process>       # run one process in the foreground
./process-compose.sh ready <process>     # readiness probe for one process
```

## Prerequisites

- The amaru node checked-out from https://github.com/pragma-org/amaru
- A `cardano-cli` executable on `PATH`, or set `CARDANO_CLI`, when generating transactions at runtime
- `jq`, `curl`, and `rsync`, all available with `flox activate` in this directory (along with `process-compose`, `xxd`, and `rustup`)
- Docker, when using the built-in telemetry stack
- A directory with cardano-node configuration files, downloaded from the repository root with
  `make download-haskell-config AMARU_NETWORK=preprod`

A `cardano-node` executable and `db-analyser` are downloaded automatically by the `setup` step when
`CARDANO_NODE_HOME` is not set, and the demo databases are refreshed automatically on `up`.

## Configuration

The variables below apply to every demo. Ports, node names, and the per-node `AMARU_<NAME>_*` values
are demo-specific; see each demo's README. Defaults written as `<demo>` depend on the demo directory.

### General

| Variable        | Default             | Description                                               |
|-----------------|---------------------|-----------------------------------------------------------|
| `AMARU_NETWORK` | `preprod`           | Target network                                            |
| `AMARU_DIR`     | repository root     | Path to the amaru project directory                       |
| `BUILD_PROFILE` | `dev`               | Cargo profile used for refresh and Amaru nodes            |
| `LOGDIR`        | `/tmp/amaru-<demo>` | Directory for demo logs and downloaded cardano-node tools |
| `RUNDIR`        | `<demo dir>/run`    | Directory for generated files and per-node databases      |

### Upstream cardano-node

| Variable                              | Default                                                                                      | Description                                                                    |
|---------------------------------------|----------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------|
| `CARDANO_UPSTREAM_MODE`               | `public` on mainnet, `local` otherwise                                                       | Use a local cardano-node or a public upstream peer                             |
| `CARDANO_NODE`                        | `$CARDANO_NODE_HOME/bin/cardano-node`                                                        | Path to the cardano-node executable                                            |
| `CARDANO_NODE_HOME`                   | `$LOGDIR/cardano-node-$CARDANO_NODE_RELEASE_VERSION`                                         | Directory containing `bin/cardano-node` and `bin/db-analyser`                  |
| `CARDANO_NODE_RELEASE_VERSION`        | `11.0.1`                                                                                     | cardano-node release downloaded by `setup` when `CARDANO_NODE_HOME` is not set |
| `CARDANO_NODE_SOCKET_TIMEOUT_SECONDS` | 1800                                                                                         | Wait for the cardano-node socket file to appear                                |
| `CARDANO_NODE_QUERY_TIMEOUT_SECONDS`  | 1800                                                                                         | Wait for the cardano-node socket to answer local queries                       |
| `CARDANO_CLI`                         | `cardano-cli` on `PATH`                                                                      | Path to the cardano-cli executable                                             |
| `PUBLIC_UPSTREAM_PEER_ADDRESS`        | `backbone.cardano.iog.io:3001`                                                               | Public Cardano peer used when `CARDANO_UPSTREAM_MODE=public`                   |
| `UPSTREAM_PORT`                       | 3001                                                                                         | Port for the local cardano-node listener                                       |

### Mithril refresh and databases

A refresh bootstraps fresh Amaru chain and ledger databases from the latest epoch snapshots, downloads
the Mithril immutable chunks covering the blocks after the bootstrap tip, packages those blocks, and
replays them into the databases. The result lands in `MITHRIL_REFRESH_DIR` together with a metadata
file recording the Mithril snapshot hash, so running a refresh again exits quickly when the databases
already match the latest snapshot. On startup, `initialize` copies the refreshed databases into
isolated per-node run directories and re-synchronizes a copy only after a node has run against it or
the selected snapshot changed.

| Variable                         | Default                                         | Description                                                                                                             |
|----------------------------------|-------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------|
| `REFRESH_FROM_MITHRIL`           | `auto`                                          | Refresh Amaru databases from Mithril when local refreshed databases are missing or incomplete                           |
| `FORCE_REFRESH`                  | `false`                                         | Discard refreshed and in-progress databases and refresh from scratch                                                    |
| `CARDANO_NODE_INIT_FROM_MITHRIL` | `auto`                                          | Initialize the local cardano-node immutable DB from the selected Mithril snapshot; `true` re-initializes on every start |
| `MITHRIL_REFRESH_DIR`            | `$RUNDIR/mithril-refresh`                       | Directory containing refreshed Amaru databases                                                                          |
| `AMARU_MITHRIL_SNAPSHOTS_DIR`    | `mithril-snapshots`                             | Directory holding downloaded Mithril immutable chunks                                                                   |

A refresh uses `db-analyser` from `$CARDANO_NODE_HOME/bin` to create the epoch snapshots, and first
probes that it honours `--analyse-from`: the db-analyser bundled with cardano-node releases up to
11.0.1 silently ignores the option
([ouroboros-consensus#2061](https://github.com/IntersectMBO/ouroboros-consensus/pull/2061)) and
replays every epoch snapshot from genesis, ~25 minutes per epoch instead of about a minute. When the
probe fails, the refresh aborts with instructions; until a cardano-node release ships the fix, build
a fixed db-analyser and drop it into place:

```bash
nix build "github:IntersectMBO/ouroboros-consensus/aa96807e6891071c3553d19c07be2d39ab5c0a78#db-analyser"
install -m 755 result/bin/db-analyser "$CARDANO_NODE_HOME/bin/db-analyser"
```


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
| `TX_SYNC_TIMEOUT_SECONDS`         | 14400                                                 | Timeout for pre-submit sync waits                                                                                                                        |
| `TX_SYNC_POLL_INTERVAL_SECONDS`   | 15                                                    | Poll interval while waiting for Amaru to catch up                                                                                                        |
| `TX_SUBMIT_RETRY_LIMIT`           | 12                                                    | Submit attempts per transaction for retryable rejections                                                                                                 |
| `TX_SUBMIT_RETRY_DELAY`           | 30                                                    | Seconds between submit retries                                                                                                                           |


### Telemetry stack

Starting a demo brings up Grafana, Tempo, Prometheus, Loki, and an OTLP collector with Docker Compose
from the `monitoring` directory, first removing the previous volumes so every run starts from fresh
metrics, logs, and spans. The Amaru nodes export all three signals to the collector using the
OpenTelemetry settings below, and `telemetry-open` opens preconfigured Grafana Explore tabs.

| Variable                          | Default                                   | Description                                                                                              |
|-----------------------------------|-------------------------------------------|----------------------------------------------------------------------------------------------------------|
| `START_TELEMETRY`                 | `true`                                    | Start the unified Grafana, Tempo, Prometheus, Loki, and OTLP collector stack before the demo             |
| `TELEMETRY_DIR`                   | `$AMARU_DIR/monitoring`                   | Directory containing the telemetry Docker Compose files                                                  |
| `TELEMETRY_GRAFANA_URL`           | `http://localhost`                        | Grafana URL opened by `telemetry-open`                                                                   |
| `TELEMETRY_PROMETHEUS_URL`        | `http://localhost:9090`                   | Prometheus URL opened by `telemetry-open`                                                                |
| `TELEMETRY_COMPOSE_OVERRIDE_FILE` | `<demo dir>/telemetry/docker-compose.yml` | Extra Compose file layered onto the shared stack, for example to provision demo-owned Grafana dashboards |

### OpenTelemetry export

| Variable                              | Default                            | Description                                                                |
|---------------------------------------|------------------------------------|----------------------------------------------------------------------------|
| `OTEL_EXPORTER_OTLP_ENDPOINT`         | `http://localhost:4317`            | OTLP/gRPC endpoint for traces and logs                                     |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | `http://localhost:4318/v1/metrics` | OTLP/HTTP endpoint for metrics                                             |

### Watch and readiness

| Variable      | Default  | Description                                             |
|---------------|----------|----------------------------------------------------------|
| `WATCH_COLOR` | `always` | Set `never` to disable ANSI colors in the watch process |

## Writing a new demo

A demo's `process-compose.sh` sets its configuration variables, sources the `common/` scripts, and
dispatches commands. Beyond the variables above, the shared scripts expect the demo to define:

- `DEMO_NAME` used in the marker files recording which snapshot a database copy came from.
- `DEMO_LOG_FILES` the log files followed by the watch process, in label/color order.
- `PUBLIC_UPSTREAM_EXCLUDED_PROCESSES` processes removed from the generated `process-compose` file in
  `public-upstream` mode.
- `AMARU_<NAME>_*` variable blocks for each Amaru node, and thin `run_*`/`ready_*` wrappers calling
  `run_amaru_node`, `ready_amaru_node_listening`, and `ready_amaru_submit_api`.
- `validate_config` the demo-specific configuration checks run before startup and refreshes.
- `telemetry_urls` the Grafana URLs opened by `telemetry-open`.

See [`relay-1/process-compose.sh`](relay-1/process-compose.sh) for a complete example.
