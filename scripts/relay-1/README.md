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
- A directory with cardano-node configuration files (`config.json`, `topology.json`)
- The amaru node checked-out from https://github.com/pragma-org/amaru
- Refreshed demo databases, created automatically by `./process-compose.sh up`

## Configuration

The following environment variables configure the demo:

| Variable                        | Required | Default                                  | Description                                           |
|---------------------------------|----------|------------------------------------------|-------------------------------------------------------|
| `CARDANO_NODE`                  | **Yes**  | -                                        | Path to the cardano-node executable                   |
| `CARDANO_NODE_CONFIG_DIR`       | **Yes**  | -                                        | Directory containing config.json, topology.json, etc. |
| `CARDANO_NODE_SOCKET_FILE`      | No       | `scripts/relay-1/run/generated/cardano-node.socket` | Socket path used by the upstream cardano-node |
| `CARDANO_CLI`                   | No       | `cardano-cli` on `PATH`                  | Path to the cardano-cli executable                    |
| `CARDANO_TESTNET_MAGIC`         | No       | `NetworkMagic` from config.json          | Testnet magic for transaction generation              |
| `AMARU_DIR`                     | No       | Current directory                        | Path to the amaru project directory                   |
| `BUILD_PROFILE`                 | No       | `release`                                | Cargo profile used for refresh and Amaru nodes        |
| `REFRESH_FROM_MITHRIL`          | No       | `true`                                   | Refresh Amaru databases from Mithril before startup   |
| `MITHRIL_REFRESH_DIR`           | No       | `scripts/relay-1/run/mithril-refresh`    | Directory containing refreshed Amaru databases        |
| `AMARU_CHAIN_SOURCE_DIR`        | No       | `$MITHRIL_REFRESH_DIR/chain.preprod.db`  | Source chain DB copied into demo run dirs             |
| `AMARU_LEDGER_SOURCE_DIR`       | No       | `$MITHRIL_REFRESH_DIR/ledger.preprod.db` | Source ledger DB copied into demo run dirs            |
| `UPSTREAM_PORT`                 | No       | 3001                                     | Port for cardano-node listener                        |
| `LISTEN_PORT`                   | No       | 4001                                     | Port for amaru listener (for downstream)              |
| `DOWNSTREAM_LISTEN_PORT`        | No       | 4002                                     | Port for amaru downstream listener                    |
| `DOWNSTREAM_SUBMIT_API_ADDRESS` | No       | 127.0.0.1:8091                           | HTTP submit API address for amaru-downstream          |
| `TX_PAYMENT_SKEY`               | No       | `scripts/relay-1/keys/payment.skey`      | Payment signing key for runtime tx generation         |
| `TX_GENERATED_COUNT`            | No       | 1                                        | Number of runtime transactions to build per process   |
| `TX_OUTPUT_LOVELACE`            | No       | 1000000                                  | Lovelace sent to the self-transfer output             |
| `TX_FEE_BUFFER_LOVELACE`        | No       | 300000                                   | Fee buffer used to skip UTxOs that are too small      |
| `TX_CHANGE_BUFFER_LOVELACE`     | No       | 900000                                   | Change buffer used to keep change above min UTxO      |

The `CARDANO_NODE_CONFIG_DIR` should contain:

- `config.json` - node configuration
- `topology.json` - network topology
- `db/` - database directory (will be created if it doesn't exist)

## Usage

```bash
export CARDANO_NODE=/path/to/cardano-node
export CARDANO_NODE_CONFIG_DIR=/path/to/config-dir
./process-compose.sh up      # start the demo
./process-compose.sh down    # stop the demo
./process-compose.sh status  # check process status
```

Running `./process-compose.sh up` opens the process-compose TUI, refreshes the Amaru databases from Mithril, prepares
the isolated run directories, and starts the 3 nodes. Use the wrapper or run `process-compose -f process-compose.yaml up`
from this directory so that the configured process dependencies are used.

To refresh the Amaru chain and ledger databases from the latest Mithril snapshot before starting the demo:

```bash
./process-compose.sh refresh
```

This writes refreshed databases to `scripts/relay-1/run/mithril-refresh`. The demo uses those databases by default and
copies them into isolated per-node run directories when starting. The refresh records the Mithril snapshot hash in a
metadata file, so running refresh again exits quickly when those databases already match the latest Mithril snapshot.
Use `FORCE_REFRESH=true` to rebuild them anyway. The demo refreshes automatically before starting by default. In that
mode, the refresh runs as the `mithril-refresh` process in process-compose, followed by `setup`, and the long-running
demo processes wait for those one-shot processes to finish successfully. To skip the automatic refresh and use existing
databases, set `REFRESH_FROM_MITHRIL=false`:

```bash
REFRESH_FROM_MITHRIL=false ./process-compose.sh up
```

Refresh, setup, and the Amaru nodes use `BUILD_PROFILE=release` by default. Setup builds the `amaru` node binary once
with that profile, and the long-running node processes execute the built binary directly instead of invoking
`cargo run`. Override `BUILD_PROFILE` only when you intentionally want another Cargo profile.

### Setup process

The `setup` process is a one-shot preparation step that runs after `mithril-refresh` and before any long-running node
process starts. It validates the configured cardano-node executable, cardano-node configuration directory, transaction
generation settings, and Amaru source databases.

After validation, it clears the relay demo log files, recreates the isolated Amaru run directories, and copies the
refreshed chain and ledger databases into separate directories for `amaru-middle` and `amaru-downstream`. The node
processes depend on `setup` completing successfully, so a setup failure prevents the demo from starting with missing or
stale run directories.

Process Compose readiness probes then gate relay startup:

- `amaru-middle` starts after `cardano-upstream` answers local `cardano-cli query tip` calls.
- `amaru-downstream` starts after `amaru-middle` prints its listening log line.
- `submit-tx` starts after `cardano-upstream`, `amaru-middle`, and `amaru-downstream` are healthy.

This removes fixed submit startup sleeps; transaction generation still waits for the selected UTxO to be available in
the downstream Amaru ledger before submitting.

The configured processes are:

- `1-mithril-refresh`
- `2-setup`
- `cardano-upstream`
- `amaru-middle`
- `amaru-downstream`
- `watch`
- `submit-tx`

## Transaction Submission

The downstream Amaru node exposes the local Submit API at `DOWNSTREAM_SUBMIT_API_ADDRESS`.

To generate fresh transactions at runtime, the script uses `scripts/relay-1/keys/payment.skey` by default. You can
override it with another preprod payment signing key.
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
MAGIC="${CARDANO_TESTNET_MAGIC:-1}"

cardano-cli conway address key-gen \
  --verification-key-file payment.vkey \
  --signing-key-file payment.skey

cardano-cli conway address build \
  --payment-verification-key-file payment.vkey \
  --testnet-magic "$MAGIC" \
  --out-file payment.addr
```

Start the demo and wait until `cardano-upstream` is ready. Starting with an empty address is fine; the initial
`submit-tx` process will log that there is nothing to submit.

```bash
export TX_PAYMENT_SKEY="$PWD/payment.skey"
./process-compose.sh up
```

Fund the generated `payment.addr` on the same preprod network using
a [test faucet](https://docs.cardano.org/cardano-testnets/tools/faucet). Then query the funded UTxOs from another shell
in `scripts/relay-1`:

```bash
ADDRESS="$(cat payment.addr)"
SOCKET="${CARDANO_NODE_SOCKET_FILE:-run/generated/cardano-node.socket}"
MAGIC="${CARDANO_TESTNET_MAGIC:-1}"

cardano-cli conway query utxo \
  --testnet-magic "$MAGIC" \
  --socket-path "$SOCKET" \
  --address "$ADDRESS"
```

If the faucet gives you one large UTxO, split it into ten 3 ada UTxOs before scaling `submit-tx`. Replace `TX_IN` with
the funded transaction input shown by `cardano-cli query utxo`:

```bash
ADDRESS="$(cat payment.addr)"
RUNDIR="${RUNDIR:-run}"
SOCKET="${CARDANO_NODE_SOCKET_FILE:-run/generated/cardano-node.socket}"
MAGIC="${CARDANO_TESTNET_MAGIC:-1}"
SKEY="${TX_PAYMENT_SKEY:-payment.skey}"
TX_IN="replace-with-funded-tx-hash#0"

cardano-cli conway transaction build \
  --testnet-magic "$MAGIC" \
  --socket-path "$SOCKET" \
  --tx-in "$TX_IN" \
  --tx-out "$ADDRESS+3000000" \
  --tx-out "$ADDRESS+3000000" \
  --tx-out "$ADDRESS+3000000" \
  --tx-out "$ADDRESS+3000000" \
  --tx-out "$ADDRESS+3000000" \
  --tx-out "$ADDRESS+3000000" \
  --tx-out "$ADDRESS+3000000" \
  --tx-out "$ADDRESS+3000000" \
  --tx-out "$ADDRESS+3000000" \
  --tx-out "$ADDRESS+3000000" \
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

Confirm that the address now has ten 3 ada UTxOs:

```bash
cardano-cli conway query utxo \
  --testnet-magic "$MAGIC" \
  --socket-path "$SOCKET" \
  --address "$ADDRESS" \
  --output-json \
  | jq -r 'to_entries[] | select(.value.value.lovelace == 3000000) | .key'
```

After the ten split outputs are visible, scale `submit-tx` to ten replicas from the Process Compose TUI or CLI. Each
replica can then claim a different 3 ada UTxO and submit concurrently.

```bash
process-compose process scale submit-tx 10
```

The transactions must be valid for the downstream node's current ledger state. When accepted, the `watch` process shows
Submit API, mempool, and tx-submission logs (`RequestTx*` / `ReplyTx*`) as the middle Amaru node pulls the transaction
from the downstream Amaru node.

`submit-tx` can be scaled from Process Compose. By default each replica builds one transaction, claims the smallest
available UTxO that can cover the transaction before building, and writes generated transaction files under its own
`run/generated/submit-tx-*` directory. Accepted
transaction claims are kept for the current run because cardano-node may still show the spent input until the ledger
catches up. Restart the full demo setup to clear stale claims. Set `TX_GENERATED_COUNT` only when you intentionally want
each replica to build multiple transactions. If more replicas are started than there are spendable UTxOs, the extra
replicas log that there is nothing to submit and exit successfully.

The `watch` process marks transaction-path events with a cyan `>>> TX >>>` prefix. This covers transaction generation,
Submit API HTTP 202 responses, downstream and middle Amaru mempool acceptance, upstream cardano-node
`TraceMempoolAddedTx`, and cardano-node tx-submission request/reply propagation events. Errors and rejections are red.

Process Compose exposes log wrapping as the F6 `log_wrap` TUI toggle. With current Process Compose releases this is not
a persisted project setting, so press F6 once in the TUI to switch the watch view to Unwrap. The `watch` process does not
truncate log lines.
Set `WATCH_COLOR=never` to disable ANSI colors.

If the address only has UTxOs smaller than 3 ada, the generator falls back to one spendable input. Inputs smaller than
`TX_OUTPUT_LOVELACE + TX_FEE_BUFFER_LOVELACE + TX_CHANGE_BUFFER_LOVELACE` are skipped because they cannot cover the
self-transfer output, fee, and a change output above the min-UTxO threshold.

To see several transactions in the mempool at once, fund the address with several separate UTxOs.
