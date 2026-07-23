# Relay demo: Haskell -> Amaru -> Amaru

This demo shows the use of an Amaru node between a Haskell node (upstream) and another Amaru node (downstream):

```
cardano-node ──────→ amaru-middle ─────→ amaru-downstream
port: 3001           peer: 3001          peer: 4001
                     listen: 4001        listen: 4002
```

3 nodes total: 1 cardano-node source, 2 Amaru relays.

## Prerequisites and shared configuration

See the [demos README](../README.md) for the prerequisites, the common `process-compose.sh` commands,
and the environment variables shared by all demos (upstream cardano-node, Mithril refresh,
transaction generation, wallet preparation, telemetry, OpenTelemetry export, watch).

## Configuration

The following variables configure this demo's topology and its two Amaru nodes:

| Variable                                             | Default                                                                                                                     | Description                                             |
|------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------|
| `LISTEN_PORT`                                        | 4001                                                                                                                        | Port for the amaru-middle listener (used by downstream) |
| `DOWNSTREAM_LISTEN_PORT`                             | 4002                                                                                                                        | Port for the amaru-downstream listener                  |
| `DOWNSTREAM_SUBMIT_API_ADDRESS`                      | 127.0.0.1:8091                                                                                                              | HTTP submit API address for amaru-downstream            |
| `AMARU_DEMO_TRACE`                                   | `debug` plus trace-level filters for the demo subsystems (see `process-compose.sh`)                                         | Shared default trace filter for both nodes              |
| `AMARU_{MIDDLE,DOWNSTREAM}_LOG`                      | `info,amaru::ledger::state=trace`                                                                                           | Console/log-file filter per node                        |
| `AMARU_{MIDDLE,DOWNSTREAM}_TRACE`                    | `$AMARU_DEMO_TRACE`                                                                                                         | Telemetry trace filter per node                         |
| `AMARU_{MIDDLE,DOWNSTREAM}_WITH_OPEN_TELEMETRY`      | `true`                                                                                                                      | Export OpenTelemetry traces and metrics per node        |
| `AMARU_{MIDDLE,DOWNSTREAM}_WITH_JSON_TRACES`         | `false`                                                                                                                     | Emit local JSON span enter/exit events per node         |
| `AMARU_{MIDDLE,DOWNSTREAM}_OTEL_SERVICE_NAME`        | `amaru-middle` / `amaru-downstream`                                                                                         | OTLP service name per node                              |
| `AMARU_{MIDDLE,DOWNSTREAM}_OTEL_SERVICE_INSTANCE_ID` | `relay-1-middle-$LISTEN_PORT` / `relay-1-downstream-$DOWNSTREAM_LISTEN_PORT`                                                | OTLP service instance id per node                       |
| `AMARU_{MIDDLE,DOWNSTREAM}_LOG_FILE`                 | `$LOGDIR/amaru-middle.log` / `$LOGDIR/amaru-downstream.log`                                                                 | Node log file                                           |
| `AMARU_{MIDDLE,DOWNSTREAM}_DATA_DIR`                 | `$RUNDIR/amaru` / `$RUNDIR/amaru-downstream`                                                                                | Node chain and ledger run directories                   |

## Usage

### Starting and stopping

```bash
export AMARU_NETWORK=preprod
./process-compose.sh up      # start the demo
./process-compose.sh down    # stop the demo
./process-compose.sh status  # check process status
```

Running `./process-compose.sh up` opens the process-compose TUI. The `8-telemetry-setup` process starts the
Grafana/Tempo/Prometheus telemetry stack while the setup and initialization processes download local cardano-node tools
when needed, refresh the Amaru databases from Mithril if no complete local refresh exists, prepare the isolated run
directories, and start the relay processes. Use the wrapper instead of running `process-compose up` directly so telemetry
and the configured process dependencies are used.

👉 Set `START_TELEMETRY=false` to keep the demo in node-only mode.

Starting the demo resets the telemetry stack with `docker compose down --volumes`, which removes the previous Tempo and
Prometheus volumes, so each demo run starts from fresh spans and metrics. `./process-compose.sh down` also stops the
telemetry stack unless `START_TELEMETRY=false`, even when the stack was started separately with
`./process-compose.sh telemetry-up`.

Stopping the demo from the Process Compose TUI, for example with `F10`, uses ordered shutdown. Downstream Amaru stops
before the middle relay, and the middle relay stops before the upstream `cardano-node`. The local `cardano-node` process
gets a longer SIGTERM grace period so it can flush its database cleanly after replay has completed.

### Running on mainnet

On mainnet, the demo uses a public upstream peer by default and generates a process-compose file without the local
`cardano-node` and `6-prepare-wallet` processes: UTxOs and protocol parameters are queried from Koios instead of a
local socket. Beware that `submit-tx` then spends real ada from the configured payment key.

### Logging and tracing

Both Amaru nodes export OpenTelemetry by default, using the service names `amaru-middle` and `amaru-downstream`.

👉 Set `AMARU_<NODE>_WITH_OPEN_TELEMETRY=false` to disable OTLP export for one of the nodes. Console and process log
output is controlled separately by `AMARU_MIDDLE_LOG` and `AMARU_DOWNSTREAM_LOG`.

Both default to `info,amaru::ledger::state=trace` so the watch process can show matching
submitted transaction IDs found in blocks. Local JSON span output is disabled by default because the same trace-level
spans are exported through OpenTelemetry.

👉You can enable it with `AMARU_MIDDLE_WITH_JSON_TRACES=true` or
`AMARU_DOWNSTREAM_WITH_JSON_TRACES=true` when you want local JSON span enter/exit events.

Both Amaru nodes enable trace-level telemetry for the public consensus, store, mempool, ledger state/context/governance,
and protocol-manager schemas, so the demo captures header decode/validation traces, new-header storage traces,
transaction mempool traces, ledger activity, and peer-manager activity without enabling trace-level telemetry for every
subsystem.

### Refreshing the Amaru databases

To refresh the Amaru chain and ledger databases from the latest Mithril snapshot before starting the demo:

```bash
./process-compose.sh refresh
```

This writes refreshed databases to `scripts/demos/relay-1/run/mithril-refresh`. The demo uses those databases by default and
copies them into isolated per-node run directories when starting. When a refresh replaces existing refreshed databases,
the previous ones are moved to `run/mithril-refresh.backup` and only that most recent backup is kept.

The refresh records the Mithril snapshot hash in a metadata file, so running refresh again exits quickly when those
databases already match the latest Mithril snapshot.

Interrupted initial refreshes leave `scripts/demos/relay-1/run/mithril-refresh.in-progress`. The next refresh resumes from
those databases instead of bootstrapping from scratch. Use `FORCE_REFRESH=true` to rebuild them anyway.

The demo uses existing refreshed databases by default. `REFRESH_FROM_MITHRIL=auto` refreshes only when the local
refreshed databases or metadata are missing or incomplete.

👉Set `REFRESH_FROM_MITHRIL=true` to check Mithril before startup even when local
databases exist, or `REFRESH_FROM_MITHRIL=false` to skip the refresh step explicitly. The refresh runs as the
`1-mithril-refresh` process in process-compose, followed by `2-initialize`, and the long-running demo processes wait for
those one-shot processes to finish successfully:

```bash
REFRESH_FROM_MITHRIL=true ./process-compose.sh up
```

When using a local upstream node, `CARDANO_NODE_INIT_FROM_MITHRIL=auto` initializes
`cardano-node-config/$AMARU_NETWORK/db/immutable` from the selected Mithril snapshot only when that database has not
already been initialized from the same snapshot. This preserves cardano-node's rebuilt ledger and volatile state across
demo restarts.

👉 Set `CARDANO_NODE_INIT_FROM_MITHRIL=true` to re-initialize the immutable database on every start, which
also deletes the rebuilt ledger and volatile state, or `CARDANO_NODE_INIT_FROM_MITHRIL=false` to skip this step
explicitly.

### Build profile

Refresh, initialize, and the Amaru nodes use `BUILD_PROFILE=dev` by default so local demo iteration rebuilds faster. Initialize
builds the `amaru` node binary once with that profile, and the long-running node processes execute the built binary
directly instead of invoking `cargo run`.

👉Override `BUILD_PROFILE` only when you intentionally want another Cargo profile, for example `BUILD_PROFILE=release`.

### Setup and Initialize Processes

The `8-telemetry-setup` process starts the telemetry stack when `START_TELEMETRY=true`. The `0-setup` process is a one-shot
tool bootstrap step. When `CARDANO_NODE_HOME` is unset, it downloads the configured cardano-node release into
`/tmp/amaru-relay-1`, verifies the archive checksum when `shasum` is available, and exposes `bin/cardano-node` and
`bin/db-analyser` from that temp directory. Because that directory lives under `/tmp`, the tools are re-downloaded after
a reboot or periodic temp cleanup. When `CARDANO_NODE_HOME` is set, it validates that the configured directory already
contains `bin/db-analyser` and that `CARDANO_NODE` points at an executable.

The `2-initialize` process is a one-shot preparation step that runs after `1-mithril-refresh` and before any
long-running node process starts. It validates the cardano-node configuration directory, transaction generation
settings, and Amaru source databases.

After validation, it clears the relay demo log files and synchronizes the refreshed chain and ledger databases into
separate isolated directories for `amaru-middle` and `amaru-downstream`. If initialize already synchronized a database
and no Amaru process has run against that isolated copy since then, initialize skips synchronizing it again.

Once an Amaru node starts, that copy is marked dirty; the next initialize synchronizes it back to the refreshed snapshot
and deletes stale destination files. The node processes depend on `initialize` completing successfully, so an initialize
failure prevents the demo from starting with missing or stale run directories.

Process Compose readiness probes then gate relay startup:

- `4-amaru-middle` starts after `3-cardano-node` answers local `cardano-cli query tip` calls.
- `5-amaru-downstream` starts after `4-amaru-middle` prints its listening log line.
- `6-prepare-wallet` starts after `3-cardano-node` is healthy and prepares clean wallet UTxOs for submit traffic.
- `7-submit-tx` starts after `3-cardano-node`, `4-amaru-middle`, and `5-amaru-downstream` are healthy and
  `6-prepare-wallet` has completed successfully.

This removes fixed submit startup sleeps. Transaction generation still waits for the selected UTxO to be available in
the downstream Amaru ledger before submitting.

The configured processes are:

- `0-setup`
- `1-mithril-refresh`
- `2-initialize`
- `3-cardano-node`
- `4-amaru-middle`
- `5-amaru-downstream`
- `6-prepare-wallet`
- `7-submit-tx`
- `8-telemetry-setup`
- `8-telemetry-open` (disabled by default; start it manually from the TUI to open telemetry tabs)
- `9-watch`

## Telemetry

The demo uses the [Grafana + Tempo + Prometheus](../../monitoring) stack:

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

- Grafana Explore on Tempo, auto-refreshing every 5 seconds, for recent `amaru-middle` `roll_forward.process` spans.
- Grafana Explore on Prometheus, auto-refreshing every 5 seconds, with queries for:
  mempool insertions, mempool bytes, block height, resident memory, and CPU.

After the middle relay has synced a few blocks or accepted submitted transactions, the tabs should update automatically.
The trace tab populates when matching spans are emitted. The transaction trace and metrics view populate after
`submit-tx` submits a transaction to the downstream node and the middle node pulls it into its mempool.

Use `./process-compose.sh telemetry-open` or start `8-telemetry-open` when you want to open the telemetry tabs during the live demo.

To stop the telemetry stack:

```bash
./process-compose.sh telemetry-down
```

## Transaction Submission

The downstream Amaru node exposes the local Submit API at `DOWNSTREAM_SUBMIT_API_ADDRESS`.

To generate fresh transactions at runtime, the script uses a network-specific payment signing key. It first checks
`scripts/demos/relay-1/run/$AMARU_NETWORK-wallet/payment.skey`, then the committed
`scripts/demos/relay-1/keys/$AMARU_NETWORK/payment.skey`, then falls back to `scripts/demos/relay-1/keys/payment.skey`.

The script derives the address from the key, queries either the local upstream Haskell node or Koios for UTxOs at that
address, builds up to `TX_GENERATED_COUNT` independent self-transfer transactions with 1 ada outputs, signs them as
canonical CBOR, and submits them through downstream Amaru.

To use your own funded key, place it in the git-ignored wallet directory, where it takes precedence over the committed
demo key:

```bash
mkdir -p "run/${AMARU_NETWORK:-preprod}-wallet"
cp /path/to/payment.skey "run/${AMARU_NETWORK:-preprod}-wallet/payment.skey"
./process-compose.sh up
```

The committed keys hold testnet ada from the faucet and are public: anyone with the repository can spend from them, and
everyone running the demo at the same time shares the same wallet. The UTxO claim mechanism only coordinates replicas
on one machine, so concurrent demo runs on different machines may race on the same UTxOs and see submit rejections. If
the wallet runs dry or is too contended, replenish it from the faucet or use a key of your own in the wallet directory.

### From scratch: fund and split for concurrent submissions

#### Create a payment key and address

```bash
NETWORK="${AMARU_NETWORK:-preprod}"
KEY_DIR="run/$NETWORK-wallet"
mkdir -p "$KEY_DIR"
MAGIC="$(jq -r '.networkMagic' "../../../cardano-node-config/${AMARU_NETWORK:-preprod}/shelley-genesis.json")"

cardano-cli conway address key-gen \
  --verification-key-file "$KEY_DIR/payment.vkey" \
  --signing-key-file "$KEY_DIR/payment.skey"

cardano-cli conway address build \
  --payment-verification-key-file "$KEY_DIR/payment.vkey" \
  --testnet-magic "$MAGIC" \
  --out-file "$KEY_DIR/payment.addr"
```

#### Start the demo

Start the demo and wait until `cardano-upstream` is ready. Starting with an empty address is fine; the initial
`submit-tx` process will log that there is nothing to submit.

```bash
./process-compose.sh up
```

#### Fund the address

Fund the generated `run/$AMARU_NETWORK-wallet/payment.addr` on the same test network using
a [test faucet](https://docs.cardano.org/cardano-testnets/tools/faucet). Then query the funded UTxOs from another shell
in `scripts/demos/relay-1`:

```bash
ADDRESS="$(cat "run/${AMARU_NETWORK:-preprod}-wallet/payment.addr")"
SOCKET="run/generated/cardano-node.socket"
MAGIC="$(jq -r '.networkMagic' "../../../cardano-node-config/${AMARU_NETWORK:-preprod}/shelley-genesis.json")"

cardano-cli conway query utxo \
  --testnet-magic "$MAGIC" \
  --socket-path "$SOCKET" \
  --address "$ADDRESS"
```

#### Split the funds into clean UTxOs

If the faucet gives you one large UTxO, split it into ten 2 ada UTxOs before scaling `submit-tx`. Replace `TX_IN` with
the funded transaction input shown by `cardano-cli query utxo`:

```bash
ADDRESS="$(cat "run/${AMARU_NETWORK:-preprod}-wallet/payment.addr")"
RUNDIR="${RUNDIR:-run}"
SOCKET="run/generated/cardano-node.socket"
MAGIC="$(jq -r '.networkMagic' "../../../cardano-node-config/${AMARU_NETWORK:-preprod}/shelley-genesis.json")"
SKEY="run/${AMARU_NETWORK:-preprod}-wallet/payment.skey"
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

#### Scale submit-tx

After the ten split outputs are visible, scale `7-submit-tx` to ten replicas from the Process Compose TUI or CLI. Each
replica can then claim a different 2 ada UTxO and submit concurrently.

```bash
process-compose process scale 7-submit-tx 10
```

Client commands like `process-compose process ...` attach to the already-running demo instance, so they are safe to run
directly; only `up` needs the `./process-compose.sh` wrapper.

Alternatively, start the disabled `7-submit-tx-batch` process from the Process Compose TUI. It prompts for the number of
independent transactions to submit, then builds that many transactions from distinct UTxOs and submits the signed CBOR
files concurrently. Press `Tab` to focus the interactive terminal before typing the count. For non-interactive use:

```bash
./process-compose.sh run submit-tx-batch 10
```

For repeat batches after `7-submit-tx` has already completed once, restart every scaled submit replica so `00` through
`09` all run again:

```bash
./process-compose.sh submit-tx-restart-all
```

### Preparing the wallet

Before `7-submit-tx` starts, `6-prepare-wallet` automatically ensures the payment address has enough clean UTxOs
for concurrent submit replicas. After repeated submissions, you can also rebuild the wallet into clean UTxOs manually
with:

```bash
./process-compose.sh prepare-wallet
```

This queries the upstream cardano-node socket, spends enough current UTxOs from the configured payment key, creates
`TX_REFUEL_UTXO_COUNT` self-outputs of `TX_REFUEL_OUTPUT_LOVELACE`, submits the transaction upstream, clears local
`submit-tx` claim state, and waits until the clean outputs are visible.

By default this gives the next 10-replica `submit-tx` run ten fresh 2 ada inputs.
Wallet preparation picks the largest UTxOs first so the transaction stays small and reliable.
The command is idempotent: if enough clean outputs already exist, it clears local `submit-tx` claim state and
exits without submitting a new transaction. Set `TX_REFUEL_FORCE=true` to rebuild clean outputs anyway.

👉 Set `TX_REFUEL_SELECTION=smallest` only when you specifically want to consolidate tiny outputs; if the wallet has many tiny
outputs, increase `TX_REFUEL_MAX_INPUTS`; if the transaction becomes too large, use a fresh funded key instead. Wallet
preparation logs are written to `/tmp/amaru-relay-1/prepare-wallet.log` by default.

The Process Compose TUI also lists this as `6-prepare-wallet`. Restart it manually when you want to rebuild clean
submit inputs before another scaled batch.

### Submission behavior and UTxO claims

The transactions must be valid for the downstream node's current ledger state. When accepted, the `watch` process shows
Submit API, mempool, and tx-submission logs (`RequestTx*` / `ReplyTx*`) as the middle Amaru node pulls the transaction
from the downstream Amaru node.

`submit-tx` can be scaled from Process Compose. By default each replica builds one transaction, claims a distinct UTxO
that can cover the transaction output plus fee buffer, and writes generated transaction files under its own
`run/generated/submit-tx-*` directory.

`submit-tx-batch` uses the same input selection and claim state, but claims and builds all requested transactions in one
process before submitting them concurrently. For UTxOs below the preferred 3 ada threshold, both modes drain the input
into one self-output minus the calculated fee instead of requiring a separate change output.

Accepted transaction claims are kept for the current run because cardano-node may still show the spent
input until the ledger catches up. Restarting `submit-tx` or `submit-tx-batch` clears stale claims once before replicas
select UTxOs.

👉 Set `TX_GENERATED_COUNT` only when you intentionally want each replica to build multiple transactions.

With ten spendable UTxOs, scaling `submit-tx` to ten replicas lets each replica claim a different input and submit one
 transaction. If more replicas are started than there are spendable UTxOs, the extra replicas log that there is nothing
 to submit and exit successfully.

If the address only has UTxOs smaller than 3 ada, the generator falls back to one spendable input and drains it into a
single self-output. Inputs smaller than `TX_OUTPUT_LOVELACE + TX_FEE_BUFFER_LOVELACE` are skipped because they cannot
cover the output and expected fee. To see several transactions in the mempool at once, fund the address with several
separate UTxOs.

### Watching transactions

The `watch` process marks transaction-path events with a cyan `>>> TX >>>` prefix. This covers transaction generation,
generated transaction IDs, Submit API HTTP 202 responses, wallet preparation transactions, Amaru mempool acceptance,
upstream cardano-node `TraceMempoolAddedTx`, and Amaru ledger logs that list submitted transaction IDs found in a block.

Those block transaction lines are visible by default and highlighted only when the transaction ID matches a transaction
built by `submit-tx` during the current `watch` session.

After one or more transactions enter a node's mempool, `watch` also marks that node's next adopted block (or the
upstream cardano-node's next chain extension) with a green `>>> BLOCK AFTER N TX >>>` prefix. That marker shows the
first block the node observed after accepting those transactions; use matching `>>> TX >>>` block transaction lines for
exact transaction inclusion. Errors and rejections are red.

Process Compose exposes log wrapping as the F6 `log_wrap` TUI toggle. With current Process Compose releases this is not
a persisted project setting, so press F6 once in the TUI to switch the watch view to Unwrap. The `watch` process does
not truncate log lines. The Process Compose TUI keeps the last 50000 log lines in memory for this demo.
Set `WATCH_COLOR=never` to disable ANSI colors.
