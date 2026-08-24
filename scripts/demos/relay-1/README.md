# Relay demo: Cardano upstream → Amaru → Amaru

This demo shows the use of an Amaru node between an upstream Cardano node and another Amaru node
(downstream). By default the upstream is a public well-known relay, so only the two Amaru relays run
locally:

```text
public relay ──────→ amaru-middle ─────→ amaru-downstream
(e.g. preprod-node.  listen: 4001        peer: 4001
 play.dev.cardano.                       listen: 4002
 org:3001)
```

With `CARDANO_UPSTREAM_MODE=local`, a local Haskell cardano-node runs as the upstream instead:

```text
cardano-node ──────→ amaru-middle ─────→ amaru-downstream
port: 3001           peer: 3001          peer: 4001
                     listen: 4001        listen: 4002
```

## Quick start

Everything below needs only Docker. The image carries the whole demo, and the container bootstraps
its databases from the public snapshot CDN and follows a public preprod relay, so there is nothing
to configure.

**1. Start the monitoring stack**, so the nodes have somewhere to send metrics, logs and traces.
It lives in [`monitoring`](../../../monitoring) and is shared with the rest of the project:

```bash
docker compose -f ../../../monitoring/docker-compose.yml up -d
```

**2. Start the demo**, joined to that stack's network so the collector resolves by name:

```bash
docker run -d \
 --name relay-1 \
 --network monitoring \
 --volume amaru-relay-1:/data \
 ghcr.io/pragma-org/amaru-relay-1-demo:latest
```

Both relays are listening within a couple of minutes: about 90 seconds to fetch and import three
epoch snapshots, then they catch up to the network tip. The demo submits a preprod transaction on
its own once they are synced.

The volume is what makes step 5 non-destructive: the bootstrapped databases outlive the container,
so a later run starts synced instead of downloading the snapshots again. Add `-p 8091:8091` if you
also want to submit transactions from your own machine rather than from inside the container.

**3. Watch it in Grafana** at [http://localhost](http://localhost). Three views matter here:

- [Relay consensus performance](http://localhost/d/amaru-relay-consensus-perf): header outcomes,
  block fetch and forwarding latencies, and a table of individual header lifecycle events.
- [Relay mempool](http://localhost/d/amaru-relay-mempool): mempool insertions, size and
  revalidation, which is where a submitted transaction shows up.
- [Amaru overview](http://localhost/d/amaru-overview): node metrics, live logs and recent traces.

Each has a **Service** selector at the top: leave it on `All` to see `amaru-middle` and
`amaru-downstream` side by side, which is the point of a two-relay demo.

**4. Drive it from the Process Compose TUI**, which shows every process and its live logs:

```bash
docker exec -it relay-1 process-compose attach
```

- Arrow keys walk the process list, and the pane on the right follows the selected process.
- `F6` toggles log wrapping. The `9-watch` process is much easier to read unwrapped.
- `F10` shuts the demo down. Otherwise, quitting with `Ctrl-C` instead detaches and leaves it running.

The `9-watch` process follows both relays' logs at once and marks the transaction
path, `>>> TX >>>` when one is submitted or enters a mempool and `>>> TX IN BLOCK >>>` when a node
sees it come back in a block.

To submit another transaction, restart that process, from the TUI with ctrl+R on the `submit-tx` process.

**5. Stop it**, keeping the volume so a restart skips the bootstrap:

```bash
docker rm -f relay-1
```

To run the demo from this checkout instead, so you can exercise your own changes, read on. The
container is covered in more detail in [docker/README.md](docker/README.md).

## Reference

Everything from here on is reference material for running the demo from this checkout: what each
process does, how to configure it, and how transaction submission works. The
[quick start](#quick-start) is enough to watch the demo run.

- [Configuration](#configuration): the variables specific to this demo
- [Usage](#usage): starting and stopping, upstream modes, logging, bootstrapping, the processes
- [Running in Docker](#running-in-docker): building the image yourself
- [Telemetry](#telemetry): what is exported and which Grafana tabs to open
- [Transaction submission](#transaction-submission): how transactions are built, claimed and submitted

See the [demos README](../README.md) for the prerequisites, the common `process-compose.sh` commands,
the shared `common/` scripts, and the environment variables shared by all demos.

## Configuration

The following variables configure this demo's topology and its two Amaru nodes:

| Variable                                             | Default                                                                       | Description                                             |
|------------------------------------------------------|-------------------------------------------------------------------------------|---------------------------------------------------------|
| `LISTEN_PORT`                                        | 4001                                                                          | Port for the amaru-middle listener (used by downstream) |
| `DOWNSTREAM_LISTEN_PORT`                             | 4002                                                                          | Port for the amaru-downstream listener                  |
| `DOWNSTREAM_SUBMIT_API_ADDRESS`                      | 127.0.0.1:8091                                                                | HTTP submit API address for amaru-downstream            |
| `MIDDLE_SUBMIT_API_ADDRESS`                          | 127.0.0.1:8090                                                                | HTTP submit API address for amaru-middle                |
| `TX_SUBMIT_API_ADDRESS`                              | `$DOWNSTREAM_SUBMIT_API_ADDRESS`                                              | Where the submit-tx processes post transactions         |
| `AMARU_MAX_EXTRA_LEDGER_SNAPSHOTS`                   | `0`                                                                           | Extra historical ledger snapshots retained per node     |
| `AMARU_DEMO_TRACE`                                   | `info,amaru=trace`                                                            | Shared default trace filter for both nodes              |
| `AMARU_DEMO_WITH_OPEN_TELEMETRY`                     | `auto`                                                                        | `auto` exports only when the OTLP collector answers     |
| `AMARU_{MIDDLE,DOWNSTREAM}_LOG`                      | `info`                                                                        | Console/log-file filter per node                        |
| `AMARU_{MIDDLE,DOWNSTREAM}_TRACE`                    | `$AMARU_DEMO_TRACE`                                                           | Telemetry trace filter per node                         |
| `AMARU_{MIDDLE,DOWNSTREAM}_WITH_OPEN_TELEMETRY`      | resolved from `AMARU_DEMO_WITH_OPEN_TELEMETRY`                                | Export OpenTelemetry metrics, logs, and spans per node  |
| `AMARU_{MIDDLE,DOWNSTREAM}_WITH_JSON_TRACES`         | `false`                                                                       | Emit local JSON span enter/exit events per node         |
| `AMARU_{MIDDLE,DOWNSTREAM}_OTEL_SERVICE_NAME`        | `amaru-middle` / `amaru-downstream`                                           | OTLP service name per node                              |
| `AMARU_{MIDDLE,DOWNSTREAM}_OTEL_SERVICE_INSTANCE_ID` | `relay-1-middle-$LISTEN_PORT` / `relay-1-downstream-$DOWNSTREAM_LISTEN_PORT`  | OTLP service instance id per node                       |
| `AMARU_{MIDDLE,DOWNSTREAM}_LOG_FILE`                 | `$LOGDIR/amaru-middle.log` / `$LOGDIR/amaru-downstream.log`                   | Node log file                                           |
| `AMARU_{MIDDLE,DOWNSTREAM}_DATA_DIR`                 | `$RUNDIR/amaru` / `$RUNDIR/amaru-downstream`                                  | Node chain and ledger run directories                   |

The demo keeps no extra historical ledger snapshots, matching the node's own default, because each retained snapshot
costs approximately 2 GB on mainnet and `initialize` gives both nodes their own copy of the databases. Set
`AMARU_MAX_EXTRA_LEDGER_SNAPSHOTS` to a number to keep that many, or to `all` to keep every one.

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
process-compose file omits the local `3-cardano-node` process: UTxOs, protocol parameters and transaction submission go
through Koios instead of a local socket. On mainnet, beware that `submit-tx` then spends real ada from the configured
payment key.

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
chain synchronization. The exported trace filter is `info,amaru=trace`, because most of what the Grafana trace panels
show is emitted below info. Local JSON span enter/exit events are disabled by default.

The filter deliberately enables the whole `amaru` tree rather than naming targets. A subsystem's spans are spread across
several targets, so a filter listing a few of them reads as precise while hiding most of the work: validating one block
covers `amaru::ledger::state`, `amaru::ledger::block` and the validation-context targets, and naming only the first
leaves a `block.validate` span with almost nothing under it.

👉 Set `AMARU_DEMO_TRACE` to narrow or widen what is exported, keeping in mind that narrowing by target tends to remove
more than intended. Set `AMARU_MIDDLE_WITH_JSON_TRACES=true` or `AMARU_DOWNSTREAM_WITH_JSON_TRACES=true` to write local
JSON span events.

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

The `0-setup` process is a one-shot tool bootstrap step. It downloads the cardano-node configuration
files for the network, and a pinned cardano-cli release whenever transactions are generated and no
usable `cardano-cli` is already configured. The much larger cardano-node release follows only in
local upstream mode, which needs the `cardano-node` binary itself. Every archive is verified against
the release checksums.

Both land in `/tmp/amaru-relay-1`, so they are re-downloaded after a reboot or a periodic temp
cleanup. When `CARDANO_NODE_HOME` is set, the configured directory is used as-is.

The `2-initialize` process is a one-shot preparation step that runs after `0-setup` and `1-bootstrap` and before any
long-running node process starts. It validates the cardano-node configuration directory, transaction generation
settings, and Amaru source databases.

After validation, it clears the relay demo log files and synchronizes the bootstrapped chain and ledger databases into
separate isolated directories for `amaru-middle` and `amaru-downstream`. Each source marker remains beside its working
database. Thus, a normal restart preserves its chain progress and historical ledger snapshots.

An explicit refresh creates a new source marker. The next initialize then replaces the working databases with that new
bootstrap. The node processes depend on initialize, so an initialization failure prevents the relay processes from
starting with missing databases.

Process Compose readiness probes then gate relay startup (the `3-cardano-node` process only exists in local upstream
mode):

- `4-amaru-middle` starts after `3-cardano-node` answers local `cardano-cli query tip` calls.
- `5-amaru-downstream` starts after `4-amaru-middle` prints its listening log line.
- `6-prepare-wallet` prepares clean wallet UTxOs for submit traffic, after `2-initialize` and, in local mode, after
  `3-cardano-node` is healthy.
- `7-submit-tx` starts after `4-amaru-middle` and `5-amaru-downstream` are healthy, `6-prepare-wallet` has completed
  successfully, and in local mode after `3-cardano-node` is healthy.

This removes fixed submit startup sleeps. Transaction generation still waits for the selected UTxO to be available in
the downstream Amaru ledger before submitting.

The configured processes are:

- `0-setup`
- `1-bootstrap`
- `2-initialize`
- `3-cardano-node` (local upstream mode only)
- `4-amaru-middle`
- `5-amaru-downstream`
- `6-prepare-wallet`
- `7-submit-tx`
- `9-watch`

## Running in Docker

The [quick start](#quick-start) runs the published image. To build one from this checkout instead,
with your own changes or an unreleased amaru, use `docker/build.sh`; it prints the ways to start
what it built. The image is the demo flox environment exported with `flox containerize`, plus
pinned amaru and cardano-cli releases and these scripts, so it runs the same toolset as the host
demo. See [docker/README.md](docker/README.md) for the details, including the published tags, the
telemetry wiring and how to wipe and re-bootstrap a container. Running on mainnet with your own
payment key is worked through in
[Mainnet, with your own wallet](docker/README.md#mainnet-with-your-own-wallet).

## Telemetry

The demo exports to the [unified monitoring stack](../../../monitoring), which it neither starts nor stops. Start it
whenever you want telemetry, before or after the demo:

```bash
docker compose -f ../../../monitoring/docker-compose.yml up -d
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
docker compose -f ../../../monitoring/docker-compose.yml down
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

To generate fresh transactions at runtime, the script uses a network-specific payment signing key. `TX_PAYMENT_SKEY`
names it outright; otherwise it checks `$RUNDIR/$AMARU_NETWORK-wallet/payment.skey` (which is
`scripts/demos/relay-1/run/` on the host and the data volume in the container), then the committed
`scripts/demos/relay-1/keys/$AMARU_NETWORK/payment.skey`, then falls back to
`scripts/demos/relay-1/keys/payment.skey`.

`TX_PAYMENT_SKEY` may also hold the key itself instead of a path to it, which is how a container can run without
mounting anything. The two are told apart by the shape of the value, not by trying the path first: anything starting
with `addr_xsk1`, `root_xsk1` or `{` is key material, and anything else is a path that must exist. A mistyped path
reports a missing file rather than an unintelligible key.

👉 Prefer a path for real funds. A value passed this way is readable for the life of the container through
`docker inspect`, and lands in shell history and in the environment of every process the demo starts; a read-only mount
exposes only the path.

The script derives the address from the key, queries either the local upstream Haskell node or Koios for UTxOs at that
address, builds up to `TX_GENERATED_COUNT` independent self-transfer transactions with 1 ada outputs, signs them as
canonical CBOR, and submits them to the node at `TX_SUBMIT_API_ADDRESS`, the downstream one unless it is pointed
elsewhere.

Every generated transaction carries `TX_METADATA_MESSAGE` under metadata label 674, the CIP-20 label for human-readable
transaction messages, so a transaction that travelled through the relays can be identified in a public explorer by its
comment. It costs about 3200 lovelace of extra fee and can be turned off with `TX_METADATA_MESSAGE=`.

To use your own funded key, place it in the git-ignored wallet directory, where it takes precedence over the committed
demo key:

```bash
mkdir -p "run/${AMARU_NETWORK:-preprod}-wallet"
cp /path/to/payment.skey "run/${AMARU_NETWORK:-preprod}-wallet/payment.skey"
./process-compose.sh up
```

### Which address the demo funds

The address is always derived from the signing key, never configured separately, so the demo cannot build a transaction
it is unable to sign. That derived address is an *enterprise* address, with no stake part, which is not the address a
wallet application shows for the same key. Fund the derived one; print it with:

```bash
cardano-cli conway key verification-key \
  --signing-key-file "run/${AMARU_NETWORK:-preprod}-wallet/payment.skey" \
  --verification-key-file /tmp/payment.vkey
cardano-cli conway address build --payment-verification-key-file /tmp/payment.vkey --mainnet
```

### Keys derived from a mnemonic

A `cardano-address` payment key works as-is, with no conversion step: point `TX_PAYMENT_SKEY` at the `addr_xsk` file, or
copy it into the wallet directory as `payment.skey`. Each process converts it to the cardano-cli format in its own
generated directory, which is cleared on every run, so no extra copy of the key is left behind.

The `root_xsk` file is rejected on startup. `cardano-cli` would convert it just as readily as a payment key, to an
address that never holds the demo funds, so pass the `addr_xsk` derived from it instead.

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

This queries the upstream (the cardano-node socket in local mode, Koios with a public upstream), spends enough current
UTxOs from the configured payment key, creates `TX_REFUEL_UTXO_COUNT` self-outputs of `TX_REFUEL_OUTPUT_LOVELACE`,
submits the transaction upstream, clears local `submit-tx` claim state, and waits until the clean outputs are visible.

By default this gives the next 10-replica `submit-tx` run ten fresh 2 ada inputs. Running it between batches is how
repeated concurrent submissions work: each round consumes the clean inputs, and the next preparation rebuilds them from
what is left, so the wallet only needs topping up when fees have eaten through it.
Wallet preparation picks the largest UTxOs first so the transaction stays small and reliable.

It submits a transaction only when it must. A round of `submit-tx` drains each input into a slightly smaller output that
is still spendable, so the next few rounds need no preparation at all; preparation happens once too few outputs clear
`TX_OUTPUT_LOVELACE + TX_FEE_BUFFER_LOVELACE` threshold. Set `TX_REFUEL_FORCE=true` to rebuild anyway, or
`TX_REFUEL_UTXO_COUNT=0` to skip the step for a single-transaction demo.

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
not truncate log lines. The Process Compose TUI keeps the last 1000 log lines in memory for this demo
(`log_length` in `process-compose.yaml`); the full history stays in the log files under `LOGDIR`.
Set `WATCH_COLOR=never` to disable ANSI colors.
