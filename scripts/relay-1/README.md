# Relay demo: Haskell -> Amaru -> Amaru

This demo shows the use of an Amaru node between a Haskell node (upstream) and another Amaru node (downstream):

```
cardano-node ──────→ amaru-middle ─────→ amaru-downstream
port: 3001           peer: 3001        peer: 4001
                     listen: 4001      listen: 4002
```

3 nodes total: 1 cardano-node source, 2 Amaru relays.

## Prerequisites

- A `cardano-node` executable (installed via package manager, built from source, etc.)
- A `cardano-cli` executable on `PATH`, or set `CARDANO_CLI`, when generating transactions at runtime
- `jq`
- A directory with cardano-node configuration files (`config.json`, `topology.json`)
- Docker, when using the built-in telemetry stack
- The amaru node checked-out from https://github.com/pragma-org/amaru
- Refreshed demo databases, created automatically by `./process-compose.sh up`

## Configuration

The following environment variables configure the demo:

| Variable                               | Required | Default                                                                                                                                                                                                                                                            | Description                                                              |
|----------------------------------------|----------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------|
| `CARDANO_NODE`                         | **Yes**  | -                                                                                                                                                                                                                                                                  | Path to the cardano-node executable                                      |
| `CARDANO_NODE_CONFIG_DIR`              | **Yes**  | -                                                                                                                                                                                                                                                                  | Network-specific directory containing config.json, topology.json, etc.   |
| `CARDANO_NODE_SOCKET_FILE`             | No       | `scripts/relay-1/run/generated/cardano-node.socket`                                                                                                                                                                                                                | Socket path used by the upstream cardano-node                            |
| `CARDANO_CLI`                          | No       | `cardano-cli` on `PATH`                                                                                                                                                                                                                                            | Path to the cardano-cli executable                                       |
| `CARDANO_TESTNET_MAGIC`                | No       | Network magic from config/genesis files                                                                                                                                                                                                                            | Testnet magic for transaction generation                                 |
| `AMARU_DIR`                            | No       | Current directory                                                                                                                                                                                                                                                  | Path to the amaru project directory                                      |
| `BUILD_PROFILE`                        | No       | `dev`                                                                                                                                                                                                                                                              | Cargo profile used for refresh and Amaru nodes                           |
| `REFRESH_FROM_MITHRIL`                 | No       | `true`                                                                                                                                                                                                                                                             | Refresh Amaru databases from Mithril before startup                      |
| `MITHRIL_REFRESH_DIR`                  | No       | `scripts/relay-1/run/mithril-refresh`                                                                                                                                                                                                                              | Directory containing refreshed Amaru databases                           |
| `AMARU_CHAIN_SOURCE_DIR`               | No       | `$MITHRIL_REFRESH_DIR/chain.$AMARU_NETWORK.db`                                                                                                                                                                                                                     | Source chain DB copied into demo run dirs                                |
| `AMARU_LEDGER_SOURCE_DIR`              | No       | `$MITHRIL_REFRESH_DIR/ledger.$AMARU_NETWORK.db`                                                                                                                                                                                                                    | Source ledger DB copied into demo run dirs                               |
| `UPSTREAM_PORT`                        | No       | 3001                                                                                                                                                                                                                                                               | Port for cardano-node listener                                           |
| `LISTEN_PORT`                          | No       | 4001                                                                                                                                                                                                                                                               | Port for amaru listener (for downstream)                                 |
| `DOWNSTREAM_LISTEN_PORT`               | No       | 4002                                                                                                                                                                                                                                                               | Port for amaru downstream listener                                       |
| `DOWNSTREAM_SUBMIT_API_ADDRESS`        | No       | 127.0.0.1:8091                                                                                                                                                                                                                                                     | HTTP submit API address for amaru-downstream                             |
| `TX_PAYMENT_SKEY`                      | No       | `scripts/relay-1/keys/$AMARU_NETWORK/payment.skey`                                                                                                                                                                                                                 | Payment signing key for runtime tx generation                            |
| `TX_GENERATED_COUNT`                   | No       | 1                                                                                                                                                                                                                                                                  | Number of runtime transactions to build per process                      |
| `TX_OUTPUT_LOVELACE`                   | No       | 1000000                                                                                                                                                                                                                                                            | Lovelace sent to the self-transfer output                                |
| `TX_FEE_BUFFER_LOVELACE`               | No       | 300000                                                                                                                                                                                                                                                             | Fee buffer used to skip UTxOs that are too small                         |
| `TX_DRAIN_SMALL_UTXOS`                 | No       | `true`                                                                                                                                                                                                                                                             | Build one-output drain transactions for small UTxOs                      |
| `TX_REFUEL_UTXO_COUNT`                 | No       | 10                                                                                                                                                                                                                                                                 | Clean UTxOs created by `refuel-submit-wallet`                            |
| `TX_REFUEL_OUTPUT_LOVELACE`            | No       | 2000000                                                                                                                                                                                                                                                            | Lovelace per clean refuel UTxO                                           |
| `TX_REFUEL_MAX_INPUTS`                 | No       | 80                                                                                                                                                                                                                                                                 | Maximum current UTxOs to consume while refuelling                        |
| `TX_REFUEL_SELECTION`                  | No       | `largest`                                                                                                                                                                                                                                                          | Pick `largest` or `smallest` UTxOs first while refuelling                |
| `TX_REFUEL_FORCE`                      | No       | `false`                                                                                                                                                                                                                                                            | Rebuild clean UTxOs even if enough already exist                         |
| `CLEAR_SUBMIT_TX_CLAIMS_ON_START`      | No       | `true`                                                                                                                                                                                                                                                             | Clear stale submit-tx UTxO claims when replica 0 starts                  |
| `START_TELEMETRY`                      | No       | `true`                                                                                                                                                                                                                                                             | Start Grafana, Tempo, Prometheus, and the OTLP collector before the demo |
| `TELEMETRY_GRAFANA_URL`                | No       | `http://localhost`                                                                                                                                                                                                                                                 | Grafana URL opened by `telemetry-open`                                   |
| `TELEMETRY_PROMETHEUS_URL`             | No       | `http://localhost:9090`                                                                                                                                                                                                                                            | Prometheus URL opened by `telemetry-open`                                |
| `AMARU_MIDDLE_WITH_OPEN_TELEMETRY`     | No       | `true`                                                                                                                                                                                                                                                             | Export OpenTelemetry traces and metrics from `amaru-middle`              |
| `AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY` | No       | `true`                                                                                                                                                                                                                                                             | Export OpenTelemetry traces and metrics from `amaru-downstream`          |
| `AMARU_MIDDLE_LOG`                     | No       | `info`                                                                                                                                                                                                                                                             | Console/log-file filter for `amaru-middle` process output                |
| `AMARU_DOWNSTREAM_LOG`                 | No       | `info`                                                                                                                                                                                                                                                             | Console/log-file filter for `amaru-downstream` process output            |
| `AMARU_MIDDLE_WITH_JSON_TRACES`        | No       | `false`                                                                                                                                                                                                                                                            | Emit local JSON span enter/exit events from `amaru-middle`               |
| `AMARU_DOWNSTREAM_WITH_JSON_TRACES`    | No       | `false`                                                                                                                                                                                                                                                            | Emit local JSON span enter/exit events from `amaru-downstream`           |
| `AMARU_MIDDLE_TRACE`                   | No       | `info,amaru::consensus=trace,amaru::stores::consensus=trace,amaru::stores::ledger=trace,amaru::stores::rocksdb=trace,amaru::mempool=trace,amaru::ledger::state=trace,amaru::ledger::context=trace,amaru::ledger::governance=trace,amaru::protocols::manager=trace` | Trace filter for `amaru-middle` telemetry                                |
| `AMARU_DOWNSTREAM_TRACE`               | No       | `info,amaru::consensus=trace,amaru::stores::consensus=trace,amaru::stores::ledger=trace,amaru::stores::rocksdb=trace,amaru::mempool=trace,amaru::ledger::state=trace,amaru::ledger::context=trace,amaru::ledger::governance=trace,amaru::protocols::manager=trace` | Trace filter for `amaru-downstream` telemetry                            |
| `OTEL_EXPORTER_OTLP_ENDPOINT`          | No       | `http://localhost:4317`                                                                                                                                                                                                                                            | OTLP/gRPC endpoint for traces and logs                                   |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`  | No       | `http://localhost:4318/v1/metrics`                                                                                                                                                                                                                                 | OTLP/HTTP endpoint for metrics                                           |
| `OTEL_METRIC_EXPORT_INTERVAL_MS`       | No       | `1000`                                                                                                                                                                                                                                                             | Amaru OTLP metric export interval                                        |

The `CARDANO_NODE_CONFIG_DIR` should contain:

- `config.json` - node configuration
- `topology.json` - network topology
- `db/` - database directory (will be created if it doesn't exist)

## Usage

```bash
export CARDANO_NODE=/path/to/cardano-node
export CARDANO_NODE_CONFIG_DIR="$PWD/../../cardano-node-config/${AMARU_NETWORK:-preprod}"
./process-compose.sh up      # start the demo
./process-compose.sh down    # stop the demo
./process-compose.sh status  # check process status
```

Running `./process-compose.sh up` starts the Grafana/Tempo/Prometheus telemetry stack, opens the process-compose TUI,
refreshes the Amaru databases from Mithril, prepares the isolated run directories, and starts the 3 nodes. Use the
wrapper instead of running `process-compose` directly so telemetry and the configured process dependencies are used.
Set `START_TELEMETRY=false` to keep the demo in node-only mode.

Both Amaru nodes export OpenTelemetry by default, using the service names `amaru-middle` and `amaru-downstream`.
Set `AMARU_MIDDLE_WITH_OPEN_TELEMETRY=false` or `AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY=false` to disable OTLP export for
one of the nodes. Console and process log output is controlled separately by `AMARU_MIDDLE_LOG` and
`AMARU_DOWNSTREAM_LOG`; both default to `info`. Local JSON span output is disabled by default because the same
trace-level
spans are exported through OpenTelemetry; enable it with `AMARU_MIDDLE_WITH_JSON_TRACES=true` or
`AMARU_DOWNSTREAM_WITH_JSON_TRACES=true` when you want local JSON span enter/exit events.

Both Amaru nodes enable trace-level telemetry for the public consensus, store, mempool, ledger state/context/governance,
and protocol-manager schemas, so the demo captures header decode/validation traces, new-header storage traces,
transaction mempool traces, ledger activity, and peer-manager activity without enabling trace-level telemetry for every
subsystem.

To refresh the Amaru chain and ledger databases from the latest Mithril snapshot before starting the demo:

```bash
./process-compose.sh refresh
```

This writes refreshed databases to `scripts/relay-1/run/mithril-refresh`. The demo uses those databases by default and
copies them into isolated per-node run directories when starting. The refresh records the Mithril snapshot hash in a
metadata file, so running refresh again exits quickly when those databases already match the latest Mithril snapshot.
Interrupted initial refreshes leave `scripts/relay-1/run/mithril-refresh.in-progress`; the next refresh resumes from
those databases instead of bootstrapping from scratch. Use `FORCE_REFRESH=true` to rebuild them anyway. The demo
refreshes automatically before starting by default. In that mode, the refresh runs as the `1-mithril-refresh` process
in process-compose, followed by `2-setup`, and the
long-running
demo processes wait for those one-shot processes to finish successfully. To skip the automatic refresh and use existing
databases, set `REFRESH_FROM_MITHRIL=false`:

```bash
REFRESH_FROM_MITHRIL=false ./process-compose.sh up
```

Refresh, setup, and the Amaru nodes use `BUILD_PROFILE=dev` by default so local demo iteration rebuilds faster. Setup
builds the `amaru` node binary once with that profile, and the long-running node processes execute the built binary
directly instead of invoking `cargo run`. Override `BUILD_PROFILE` only when you intentionally want another Cargo
profile, for example `BUILD_PROFILE=release`.

### Setup process

The `2-setup` process is a one-shot preparation step that runs after `1-mithril-refresh` and before any long-running
node
process starts. It validates the configured cardano-node executable, cardano-node configuration directory, transaction
generation settings, and Amaru source databases.

After validation, it clears the relay demo log files and synchronizes the refreshed chain and ledger databases into
separate isolated directories for `amaru-middle` and `amaru-downstream`. If setup already synchronized a database and no
Amaru process has run against that isolated copy since then, setup skips synchronizing it again. Once an Amaru node
starts,
that copy is marked dirty; the next setup synchronizes it back to the refreshed snapshot and deletes stale destination
files. The node processes depend on `setup` completing successfully, so a setup failure prevents the demo from starting
with missing or stale run directories.

Process Compose readiness probes then gate relay startup:

- `4-amaru-middle` starts after `3-cardano-node` answers local `cardano-cli query tip` calls.
- `5-amaru-downstream` starts after `4-amaru-middle` prints its listening log line.
- `6-refuel-submit-wallet` starts after `3-cardano-node` is healthy and prepares clean wallet UTxOs for submit traffic.
- `7-submit-tx` starts after `3-cardano-node`, `4-amaru-middle`, and `5-amaru-downstream` are healthy and
  `6-refuel-submit-wallet` has completed successfully.

This removes fixed submit startup sleeps; transaction generation still waits for the selected UTxO to be available in
the downstream Amaru ledger before submitting.

The configured processes are:

- `1-mithril-refresh`
- `2-setup`
- `3-cardano-node`
- `4-amaru-middle`
- `5-amaru-downstream`
- `6-refuel-submit-wallet`
- `7-submit-tx`
- `8-telemetry` (disabled by default; start it manually from the TUI to open telemetry tabs)
- `9-watch`

## Telemetry

The demo uses the [Grafana + Tempo + Prometheus](../../monitoring/grafana-tempo) stack:

- Grafana: [http://localhost](http://localhost)
- Prometheus: [http://localhost:9090](http://localhost:9090)
- OTLP collector: `localhost:4317` for traces/logs and `localhost:4318/v1/metrics` for metrics

Start the telemetry stack without starting the relay nodes:

```bash
./process-compose.sh telemetry-up
```

Open the useful browser tabs for the demo:

```bash
./process-compose.sh telemetry-open
```

This opens:

- Grafana Explore on Tempo, auto-refreshing every 5 seconds, for recent `amaru-middle` `decode_header` traces.
- The `Amaru Relay Mempool` Grafana dashboard, auto-refreshing every 5 seconds, with panels for:
  `insertions`, `in_mempool`, `mempool_bytes`, `processed_1m`, `processed_15m`, `revalidation_ms`, and
  `revalidation_total_ms`.

After the middle relay has synced a few blocks or accepted submitted transactions, the tabs should update automatically.
The all-traces tab should populate during normal chain sync. The transaction trace and mempool dashboard populate after
`submit-tx` submits a transaction to the downstream node and the middle node pulls it into its mempool.

The Process Compose TUI also lists a disabled `8-telemetry` process. It does not run during startup; start or restart
`8-telemetry` when you want to open the telemetry tabs during the live demo.

Stop the telemetry stack:

```bash
./process-compose.sh telemetry-down
```

## Transaction Submission

The downstream Amaru node exposes the local Submit API at `DOWNSTREAM_SUBMIT_API_ADDRESS`.

To generate fresh transactions at runtime, the script uses `scripts/relay-1/keys/$AMARU_NETWORK/payment.skey` by
default. You can override it with another funded testnet payment signing key.
The script derives the address from it, queries the upstream Haskell node for UTxOs at that address,
builds up to `TX_GENERATED_COUNT` independent self-transfer transactions with 1 ada outputs,
signs them as canonical CBOR, and submits them through downstream Amaru:

```bash
export TX_PAYMENT_SKEY=/path/to/payment.skey
./process-compose.sh up
```

### From scratch: fund and split for concurrent submissions

Create a payment key and address:

```bash
NETWORK="${AMARU_NETWORK:-preprod}"
KEY_DIR="keys/$NETWORK"
mkdir -p "$KEY_DIR"
MAGIC="${CARDANO_TESTNET_MAGIC:-$(jq -r '.networkMagic' "$CARDANO_NODE_CONFIG_DIR/shelley-genesis.json")}"

cardano-cli conway address key-gen \
  --verification-key-file "$KEY_DIR/payment.vkey" \
  --signing-key-file "$KEY_DIR/payment.skey"

cardano-cli conway address build \
  --payment-verification-key-file "$KEY_DIR/payment.vkey" \
  --testnet-magic "$MAGIC" \
  --out-file "$KEY_DIR/payment.addr"
```

Start the demo and wait until `cardano-upstream` is ready. Starting with an empty address is fine; the initial
`submit-tx` process will log that there is nothing to submit.

```bash
./process-compose.sh up
```

Fund the generated `keys/$AMARU_NETWORK/payment.addr` on the same test network using
a [test faucet](https://docs.cardano.org/cardano-testnets/tools/faucet). Then query the funded UTxOs from another shell
in `scripts/relay-1`:

```bash
ADDRESS="$(cat "keys/${AMARU_NETWORK:-preprod}/payment.addr")"
SOCKET="${CARDANO_NODE_SOCKET_FILE:-run/generated/cardano-node.socket}"
MAGIC="${CARDANO_TESTNET_MAGIC:-$(jq -r '.networkMagic' "$CARDANO_NODE_CONFIG_DIR/shelley-genesis.json")}"

cardano-cli conway query utxo \
  --testnet-magic "$MAGIC" \
  --socket-path "$SOCKET" \
  --address "$ADDRESS"
```

If the faucet gives you one large UTxO, split it into ten 2 ada UTxOs before scaling `submit-tx`. Replace `TX_IN` with
the funded transaction input shown by `cardano-cli query utxo`:

```bash
ADDRESS="$(cat "keys/${AMARU_NETWORK:-preprod}/payment.addr")"
RUNDIR="${RUNDIR:-run}"
SOCKET="${CARDANO_NODE_SOCKET_FILE:-run/generated/cardano-node.socket}"
MAGIC="${CARDANO_TESTNET_MAGIC:-$(jq -r '.networkMagic' "$CARDANO_NODE_CONFIG_DIR/shelley-genesis.json")}"
SKEY="${TX_PAYMENT_SKEY:-keys/${AMARU_NETWORK:-preprod}/payment.skey}"
TX_IN="replace-with-funded-tx-hash#0"

cardano-cli conway transaction build \
  --testnet-magic "$MAGIC" \
  --socket-path "$SOCKET" \
  --tx-in "$TX_IN" \
  --tx-out "$ADDRESS+2000000" \
  --tx-out "$ADDRESS+2000000" \
  --tx-out "$ADDRESS+2000000" \
  --tx-out "$ADDRESS+2000000" \
  --tx-out "$ADDRESS+2000000" \
  --tx-out "$ADDRESS+2000000" \
  --tx-out "$ADDRESS+2000000" \
  --tx-out "$ADDRESS+2000000" \
  --tx-out "$ADDRESS+2000000" \
  --tx-out "$ADDRESS+2000000" \
  --change-address "$ADDRESS" \
  --out-file "$RUNDIR/generated/split.body"

cardano-cli conway transaction sign \
  --testnet-magic "$MAGIC" \
  --tx-body-file "$RUNDIR/generated/split.body" \
  --signing-key-file "$SKEY" \
  --out-file "$RUNDIR/generated/split.signed"

cardano-cli conway transaction submit \
  --testnet-magic "$MAGIC" \
  --socket-path "$SOCKET" \
  --tx-file "$RUNDIR/generated/split.signed"
```

Confirm that the address now has ten 2 ada UTxOs:

```bash
cardano-cli conway query utxo \
  --testnet-magic "$MAGIC" \
  --socket-path "$SOCKET" \
  --address "$ADDRESS" \
  --output-json \
  | jq -r 'to_entries[] | select(.value.value.lovelace == 2000000) | .key'
```

After the ten split outputs are visible, scale `7-submit-tx` to ten replicas from the Process Compose TUI or CLI. Each
replica can then claim a different 2 ada UTxO and submit concurrently.

```bash
process-compose process scale 7-submit-tx 10
```

For repeat batches after `7-submit-tx` has already completed once, restart every scaled submit replica so `00` through
`09` all run again:

```bash
./process-compose.sh submit-tx-restart-all
```

Before `7-submit-tx` starts, `6-refuel-submit-wallet` automatically ensures the payment address has enough clean UTxOs
for concurrent submit replicas. After repeated submissions, you can also rebuild the wallet into clean UTxOs manually
with:

```bash
./process-compose.sh refuel-submit-wallet
```

This queries the upstream cardano-node socket, spends enough current UTxOs from the configured payment key, creates
`TX_REFUEL_UTXO_COUNT` self-outputs of `TX_REFUEL_OUTPUT_LOVELACE`, submits the transaction upstream, clears local
`submit-tx` claim state, and waits until the clean outputs are visible. By default this gives the next 10-replica
`submit-tx` run ten fresh 2 ada inputs. Refuelling picks the largest UTxOs first so the transaction stays small and
reliable. The command is idempotent: if enough clean outputs already exist, it clears local `submit-tx` claim state and
exits without submitting a new transaction. Set `TX_REFUEL_FORCE=true` to rebuild clean outputs anyway. Set
`TX_REFUEL_SELECTION=smallest` only when you specifically want to consolidate tiny outputs; if the wallet has many tiny
outputs, increase `TX_REFUEL_MAX_INPUTS`; if the transaction becomes too large, use a fresh funded key instead. Refuel
logs are written to `/tmp/amaru-relay-1/refuel-submit-wallet.log` by default.
The Process Compose TUI also lists this as `6-refuel-submit-wallet`; restart it manually when you want to rebuild clean
submit inputs before another scaled batch.

The transactions must be valid for the downstream node's current ledger state. When accepted, the `watch` process shows
Submit API, mempool, and tx-submission logs (`RequestTx*` / `ReplyTx*`) as the middle Amaru node pulls the transaction
from the downstream Amaru node.

`submit-tx` can be scaled from Process Compose. By default each replica builds one transaction, claims the smallest
available UTxO that can cover the transaction output plus fee buffer, and writes generated transaction files under its
own `run/generated/submit-tx-*` directory. For UTxOs below the preferred 3 ada threshold, it drains the input into one
self-output minus the calculated fee instead of requiring a separate change output. Accepted transaction claims are kept
for the current run because cardano-node may still show the spent input until the ledger catches up. Restarting
`submit-tx` clears stale claims by default once before replicas select UTxOs; set
`CLEAR_SUBMIT_TX_CLAIMS_ON_START=false` to keep claims across `submit-tx` restarts. Set `TX_GENERATED_COUNT` only when
you intentionally want each replica to build multiple transactions. With ten spendable UTxOs, scaling `submit-tx` to ten
replicas lets each replica claim a different input and submit one transaction. If more replicas are started than there
are spendable UTxOs, the extra replicas log that there is nothing to submit and exit successfully.

The `watch` process marks transaction-path events with a cyan `>>> TX >>>` prefix. This covers transaction generation,
generated transaction IDs, Submit API HTTP 202 responses, downstream and middle Amaru mempool acceptance, upstream
cardano-node
`TraceMempoolAddedTx`, and trace-level ledger logs that list submitted transaction IDs found in a block. Block
transaction
lines are highlighted only when the transaction ID matches a transaction built by `submit-tx` during the current `watch`
session. After one or more transactions enter a mempool, `watch` also marks the next middle Amaru adopted block or
upstream cardano-node chain extension with a green `>>> BLOCK AFTER N TX >>>` prefix. That marker shows the first block
observed after those transactions were accepted into the mempool; use matching `>>> TX >>>` block transaction lines for
exact transaction inclusion. Errors and rejections are red.

Process Compose exposes log wrapping as the F6 `log_wrap` TUI toggle. With current Process Compose releases this is not
a persisted project setting, so press F6 once in the TUI to switch the watch view to Unwrap. The `watch` process does
not
truncate log lines. The Process Compose TUI keeps the last 50000 log lines in memory for this demo.
Set `WATCH_COLOR=never` to disable ANSI colors.

If the address only has UTxOs smaller than 3 ada, the generator falls back to one spendable input and drains it into a
single self-output. Inputs smaller than `TX_OUTPUT_LOVELACE + TX_FEE_BUFFER_LOVELACE` are skipped because they cannot
cover the output and expected fee.

To see several transactions in the mempool at once, fund the address with several separate UTxOs.
