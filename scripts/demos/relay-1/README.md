# Relay demo: Cardano upstream -> Amaru -> Amaru

This demo shows the use of an Amaru node between an upstream Cardano node and another Amaru node
(downstream). By default the upstream is a public well-known relay, so only the two Amaru relays run
locally:

```
public relay ──────→ amaru-middle ─────→ amaru-downstream
(e.g. preprod-node.  listen: 4001        peer: 4001
 play.dev.cardano.                       listen: 4002
 org:3001)
```

With `CARDANO_UPSTREAM_MODE=local`, a local Haskell cardano-node runs as the upstream instead:

```
cardano-node ──────→ amaru-middle ─────→ amaru-downstream
port: 3001           peer: 3001          peer: 4001
                     listen: 4001        listen: 4002
```

## Prerequisites and shared configuration

See the [demos README](../README.md) for the prerequisites, the common `process-compose.sh` commands,
and the environment variables shared by all demos (upstream cardano-node, bootstrap and databases,
transaction generation, wallet preparation, telemetry, OpenTelemetry export, watch). To run this demo
with nothing but docker, see [docker/README.md](docker/README.md).

## Configuration

The following variables configure this demo's topology and its two Amaru nodes:

| Variable                                             | Default                                                                                                                     | Description                                             |
|------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------|
| `LISTEN_PORT`                                        | 4001                                                                                                                        | Port for the amaru-middle listener (used by downstream) |
| `DOWNSTREAM_LISTEN_PORT`                             | 4002                                                                                                                        | Port for the amaru-downstream listener                  |
| `DOWNSTREAM_SUBMIT_API_ADDRESS`                      | 127.0.0.1:8091                                                                                                              | HTTP submit API address for amaru-downstream            |
| `MIDDLE_SUBMIT_API_ADDRESS`                          | 127.0.0.1:8090                                                                                                              | HTTP submit API address for amaru-middle                |
| `TX_SUBMIT_API_ADDRESS`                              | `$DOWNSTREAM_SUBMIT_API_ADDRESS`                                                                                            | Where the submit-tx processes post transactions         |
| `AMARU_MAX_EXTRA_LEDGER_SNAPSHOTS`                   | `all`                                                                                                                       | Historical ledger snapshots retained per node           |
| `AMARU_DEMO_TRACE`                                   | `info,amaru::consensus=debug,amaru::mempool=trace,amaru::ledger::state=trace`                                                | Shared default trace filter for both nodes              |
| `AMARU_DEMO_WITH_OPEN_TELEMETRY`                     | `auto`                                                                                                                      | `auto` exports only when the OTLP collector answers     |
| `AMARU_{MIDDLE,DOWNSTREAM}_LOG`                      | `info`                                                                                                                      | Console/log-file filter per node                        |
| `AMARU_{MIDDLE,DOWNSTREAM}_TRACE`                    | `$AMARU_DEMO_TRACE`                                                                                                         | Telemetry trace filter per node                         |
| `AMARU_{MIDDLE,DOWNSTREAM}_WITH_OPEN_TELEMETRY`      | resolved from `AMARU_DEMO_WITH_OPEN_TELEMETRY`                                                                               | Export OpenTelemetry metrics, logs, and spans per node  |
| `AMARU_{MIDDLE,DOWNSTREAM}_WITH_JSON_TRACES`         | `false`                                                                                                                     | Emit local JSON span enter/exit events per node         |
| `AMARU_{MIDDLE,DOWNSTREAM}_OTEL_SERVICE_NAME`        | `amaru-middle` / `amaru-downstream`                                                                                         | OTLP service name per node                              |
| `AMARU_{MIDDLE,DOWNSTREAM}_OTEL_SERVICE_INSTANCE_ID` | `relay-1-middle-$LISTEN_PORT` / `relay-1-downstream-$DOWNSTREAM_LISTEN_PORT`                                                | OTLP service instance id per node                       |
| `AMARU_{MIDDLE,DOWNSTREAM}_LOG_FILE`                 | `$LOGDIR/amaru-middle.log` / `$LOGDIR/amaru-downstream.log`                                                                 | Node log file                                           |
| `AMARU_{MIDDLE,DOWNSTREAM}_DATA_DIR`                 | `$RUNDIR/amaru` / `$RUNDIR/amaru-downstream`                                                                                | Node chain and ledger run directories                   |

The demo retains all historical ledger snapshots. Set `AMARU_MAX_EXTRA_LEDGER_SNAPSHOTS` to a number to limit the
additional snapshots. A mainnet snapshot uses approximately 2 GB.

## Usage

### Starting and stopping

```bash
export AMARU_NETWORK=preprod
./process-compose.sh up      # start the demo
./process-compose.sh down    # stop the demo
./process-compose.sh status  # check process status
```

Running `./process-compose.sh up` opens the process-compose TUI. The setup and initialization processes bootstrap the
Amaru databases from the public snapshot CDN if no complete local bootstrap exists, download cardano-node tools when
they are needed, prepare the isolated run directories, and start the relay processes. Use the wrapper instead of
running `process-compose up` directly so the configured process dependencies are used.

The demo does not manage the monitoring stack: start it separately when you want telemetry (see
[Telemetry](#telemetry)). The nodes detect the collector and export only when it answers.

Stopping the demo from the Process Compose TUI, for example with `F10`, uses ordered shutdown. Downstream Amaru stops
before the middle relay, and the middle relay stops before the upstream `cardano-node` when one runs locally. The
local `cardano-node` process gets a longer SIGTERM grace period so it can flush its database cleanly after replay has
completed.

### Public and local upstream modes

By default (`CARDANO_UPSTREAM_MODE=public`) the middle relay connects to a public well-known relay and the generated
process-compose file omits the local `3-cardano-node` and `6-prepare-wallet` processes: UTxOs and protocol parameters
are queried from Koios instead of a local socket. On mainnet, beware that `submit-tx` then spends real ada from the
configured payment key.

With `CARDANO_UPSTREAM_MODE=local`, `0-setup` downloads the pinned cardano-node release and `3-cardano-node` runs it
as the upstream. Its database starts empty, which means a full network synchronization, unless
`cardano-node-config/$AMARU_NETWORK/db` is already populated from a previous run or another source.

### Logging and tracing

Both Amaru nodes export OpenTelemetry whenever the collector answers, using the service names `amaru-middle` and
`amaru-downstream`.

👉 Set `AMARU_<NODE>_WITH_OPEN_TELEMETRY=false` to disable OTLP export for one of the nodes, or
`AMARU_DEMO_WITH_OPEN_TELEMETRY=false` for both. Console and process log output is controlled separately by
`AMARU_MIDDLE_LOG` and `AMARU_DOWNSTREAM_LOG`.

The console filters default to `info`, which limits log traffic and keeps the Process Compose TUI responsive during
chain synchronization. The exported trace filter is more detailed, because the spans the Grafana dashboards query are
emitted at debug level. Local JSON span enter/exit events are disabled by default.

👉 Set `AMARU_DEMO_TRACE` to narrow or widen what is exported. Set `AMARU_MIDDLE_WITH_JSON_TRACES=true` or
`AMARU_DOWNSTREAM_WITH_JSON_TRACES=true` to write local JSON span events.

The watch process needs `amaru::ledger::state=trace` to show submitted transaction IDs found in blocks. Add that target
to `AMARU_MIDDLE_LOG` and `AMARU_DOWNSTREAM_LOG` when you need this detail.

### Bootstrapping the Amaru databases

The `1-bootstrap` process bootstraps the Amaru chain and ledger databases with `amaru bootstrap`, which downloads the
three most recent epoch snapshots from the public snapshot CDN and imports them into
`scripts/demos/relay-1/run/bootstrap`. The demo uses those databases by default and copies them into isolated per-node
run directories when starting.

A completed bootstrap is recorded in a marker file, so restarting the demo reuses the existing databases and never
bootstraps again on its own. To bootstrap again explicitly, for example to jump closer to the network tip:

```bash
./process-compose.sh refresh
```

A refresh recreates the databases but reuses the snapshot archives already downloaded into
`run/bootstrap/snapshots/$AMARU_NETWORK`, so only missing archives are fetched. An interrupted bootstrap leaves no
marker and is redone on the next start, also reusing the cached archives.

👉 Set `BOOTSTRAP_AMARU_DATABASES=false` to skip the bootstrap step explicitly (startup then requires existing
bootstrapped databases), or `AMARU_BOOTSTRAP_EPOCH=<epoch>` to bootstrap from a specific epoch instead of the latest
available one.

### Build profile

Bootstrap, initialize, and the Amaru nodes use `BUILD_PROFILE=dev` by default so local demo iteration rebuilds faster.
Initialize builds the `amaru` node binary once with that profile, and the long-running node processes execute the built
binary directly instead of invoking `cargo run`. Set `AMARU_NODE_BINARY` to use a prebuilt binary instead of building
one (the docker image does this).

👉Override `BUILD_PROFILE` only when you intentionally want another Cargo profile, for example `BUILD_PROFILE=release`.

### Setup and Initialize Processes

The `0-setup` process is a one-shot tool bootstrap step: it downloads the cardano-node configuration files for the network and, only when they
are needed (local upstream mode, or transaction signing without a usable `cardano-cli`), the pinned cardano-node
release into `/tmp/amaru-relay-1`, verifying the archive checksum. Because that directory lives under `/tmp`, the
tools are re-downloaded after a reboot or periodic temp cleanup. When `CARDANO_NODE_HOME` is set, the configured
directory is used as-is.

The `2-initialize` process is a one-shot preparation step that runs after `0-setup` and `1-bootstrap` and before any
long-running node process starts. It validates the cardano-node configuration directory, transaction generation
settings, and Amaru source databases.

After validation, it clears the relay demo log files and synchronizes the bootstrapped chain and ledger databases into
separate isolated directories for `amaru-middle` and `amaru-downstream`. Each source marker remains beside its working
database. Thus, a normal restart preserves its chain progress and historical ledger snapshots.

An explicit refresh creates a new source marker. The next initialize then replaces the working databases with that new
bootstrap. The node processes depend on initialize, so an initialization failure prevents the relay processes from
starting with missing databases.

Process Compose readiness probes then gate relay startup (the `3-cardano-node` and `6-prepare-wallet` processes only
exist in local upstream mode):

- `4-amaru-middle` starts after `3-cardano-node` answers local `cardano-cli query tip` calls.
- `5-amaru-downstream` starts after `4-amaru-middle` prints its listening log line.
- `6-prepare-wallet` starts after `3-cardano-node` is healthy and prepares clean wallet UTxOs for submit traffic.
- `7-submit-tx` starts after `4-amaru-middle` and `5-amaru-downstream` are healthy (and, in local mode, after
  `3-cardano-node` is healthy and `6-prepare-wallet` has completed successfully).

This removes fixed submit startup sleeps. Transaction generation still waits for the selected UTxO to be available in
the downstream Amaru ledger before submitting.

The configured processes are:

- `0-setup`
- `1-bootstrap`
- `2-initialize`
- `3-cardano-node` (local upstream mode only)
- `4-amaru-middle`
- `5-amaru-downstream`
- `6-prepare-wallet` (local upstream mode only)
- `7-submit-tx`
- `9-watch`

## Running in Docker

The same demo runs in a single container, needing only docker on the machine that runs it: the
image is the demo flox environment exported with `flox containerize`, plus a published amaru
release and these scripts. Build it with `docker/build.sh`, which prints the ways to start it.
See [docker/README.md](docker/README.md) for the details, including how to attach the Process
Compose TUI and how to export telemetry to a monitoring stack running on the host.

## Telemetry

The demo exports to the [unified monitoring stack](../../../monitoring), which it neither starts nor stops. Start it
whenever you want telemetry, before or after the demo:

```bash
cd ../../../monitoring && docker compose up -d
```

- Grafana: [http://localhost](http://localhost)
- Prometheus: [http://localhost:9090](http://localhost:9090)
- Loki: [http://localhost:3100](http://localhost:3100)
- OTLP collector: `localhost:4317`, for all three signals

That stack provisions the two relay dashboards (`amaru-relay-mempool` and `amaru-relay-consensus-perf`) along with the
Amaru overview. The nodes probe the collector once at startup and export only when it answers, so starting the stack
after the demo means restarting `4-amaru-middle` and `5-amaru-downstream` to pick it up.

Open the useful browser tabs for the demo:

```bash
./process-compose.sh telemetry-open
```

This opens:

- Grafana Explore on Tempo, auto-refreshing every 5 seconds, for recent `amaru-middle` `roll_forward.process` spans.
- Grafana Explore on Loki, auto-refreshing every 5 seconds, for logs from both Amaru nodes.
- The relay mempool dashboard, backed by Prometheus, for mempool insertions, mempool bytes, block height,
  resident memory, and CPU.

After the middle relay has synced a few blocks or accepted submitted transactions, the tabs should update automatically.
The trace tab populates when matching spans are emitted. The transaction trace and mempool dashboard populate after
`submit-tx` submits a transaction to the downstream node and the middle node pulls it into its mempool.

Use `./process-compose.sh telemetry-open` when you want to open those tabs during the live demo, or
`./process-compose.sh telemetry-urls` to just print the URLs.

To stop the monitoring stack:

```bash
cd ../../../monitoring && docker compose down
```

## Transaction Submission

Both Amaru nodes expose a Submit API: the downstream one at `DOWNSTREAM_SUBMIT_API_ADDRESS` and
the middle one at `MIDDLE_SUBMIT_API_ADDRESS`. The submit-tx processes post to
`TX_SUBMIT_API_ADDRESS`, which defaults to the downstream node.

👉 Set `TX_SUBMIT_API_ADDRESS="$MIDDLE_SUBMIT_API_ADDRESS"` to submit through the middle relay
instead. Only the node that receives a transaction holds it in its mempool: transactions travel
from a connection's initiator to its server, so a transaction handed to the middle relay reaches
the public network but never the downstream node's mempool, and only the middle relay reports it
coming back inside a block.

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

The `cardano-cli query ...` and `transaction submit` examples below go through the local upstream node's socket, so
they assume `CARDANO_UPSTREAM_MODE=local`. In the default public mode, fund the committed demo key (or your own) from
the faucet and let `submit-tx` query Koios instead.

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

Alternatively, start the disabled `7-submit-tx-batch` process from the Process Compose TUI. It builds
`TX_BATCH_COUNT` transactions from distinct UTxOs and submits the signed CBOR files concurrently, defaulting to
`TX_BATCH_DEFAULT_COUNT` (5) when nothing is set. The count has to come from the environment because process-compose
gives its processes no terminal to prompt on:

```bash
TX_BATCH_COUNT=10 ./process-compose.sh up
```

Run it as a one-off, outside process-compose, to pass the count directly or be prompted for it:

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

A transaction coming back in a block gets its own cyan `>>> TX IN BLOCK >>>` prefix, on the line where the node evicts
it from its mempool because it appeared in a block the node adopted. Errors and rejections are red.

Process Compose exposes log wrapping as the F6 `log_wrap` TUI toggle. With current Process Compose releases this is not
a persisted project setting, so press F6 once in the TUI to switch the watch view to Unwrap. The `watch` process does
not truncate log lines. The Process Compose TUI keeps the last 50000 log lines in memory for this demo.
Set `WATCH_COLOR=never` to disable ANSI colors.
