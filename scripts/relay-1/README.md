# Relay demo: Haskell -> Amaru -> Amaru

This demo shows the use of an Amaru node between a Haskell node (upstream) and another Amaru node (downstream):

```
cardano-node ──────→ amaru ──────→ amaru-downstream
(port: 3001)         (peer: 3001,   (peer: 4001,
                      listen: 4001)  listen: 4002)
```

3 nodes total: 1 cardano-node source, 2 Amaru relays.

## Prerequisites

- A `cardano-node` executable (installed via package manager, built from source, etc.)
- A `cardano-cli` executable on `PATH`, or set `CARDANO_CLI`, when generating transactions at runtime
- A directory with cardano-node configuration files (`config.json`, `topology.json`)
- The amaru node checked-out from https://github.com/pragma-org/amaru and bootstrapped with
  `make AMARU_NETWORK=preprod bootstrap`

## Configuration

The following environment variables configure the demo:

| Variable                  | Required | Default           | Description                                           |
|---------------------------|----------|-------------------|-------------------------------------------------------|
| `CARDANO_NODE`            | **Yes**  | -                 | Path to the cardano-node executable                   |
| `CARDANO_NODE_CONFIG_DIR` | **Yes**  | -                 | Directory containing config.json, topology.json, etc. |
| `CARDANO_CLI`             | No       | `cardano-cli` on `PATH` | Path to the cardano-cli executable              |
| `CARDANO_TESTNET_MAGIC`   | No       | `NetworkMagic` from config.json | Testnet magic for transaction generation |
| `AMARU_DIR`               | No       | Current directory | Path to the amaru project directory                   |
| `UPSTREAM_PORT`           | No       | 3001              | Port for cardano-node listener                        |
| `LISTEN_PORT`             | No       | 4001              | Port for amaru listener (for downstream)              |
| `DOWNSTREAM_LISTEN_PORT`  | No       | 4002              | Port for amaru downstream listener                    |
| `DOWNSTREAM_SUBMIT_API_ADDRESS` | No | 127.0.0.1:8091    | HTTP submit API address for amaru-downstream          |
| `TX_CBOR_FILE`            | No       | -                 | Transaction CBOR file to submit after startup         |
| `TX_CBOR_FILES`           | No       | `TX_CBOR_FILE`    | Whitespace-separated transaction CBOR files to submit |
| `TX_SUBMIT_DELAY`         | No       | 45                | Seconds to wait before auto-submitting transactions   |
| `TX_PAYMENT_SKEY`         | No       | -                 | Funded payment signing key for runtime tx generation  |
| `TX_PAYMENT_VKEY`         | No       | Derived from signing key | Payment verification key for address derivation |
| `TX_PAYMENT_ADDRESS`      | No       | Derived from verification key | Funded payment address to query              |
| `TX_GENERATED_COUNT`      | No       | 3                 | Number of runtime transactions to build and submit    |
| `TX_OUTPUT_LOVELACE`      | No       | 1000000           | Self-transfer output amount in each generated tx      |
| `TX_MIN_INPUT_LOVELACE`   | No       | 3000000           | Preferred minimum input for multi-tx generation       |

The `CARDANO_NODE_CONFIG_DIR` should contain:

- `config.json` - node configuration
- `topology.json` - network topology
- `db/` - database directory (will be created if it doesn't exist)

## Usage

```bash
export CARDANO_NODE=/path/to/cardano-node
export CARDANO_NODE_CONFIG_DIR=/path/to/config-dir
./demo.sh           # start the demo
./demo.sh stop      # stop the demo
./demo.sh status    # check status
./demo.sh restart amaru  # restart the amaru node
```

Running `./demo.sh` will open a tmux session starting the 3 nodes and we should see new headers eventually being
received by the downstream amaru node.

## Transaction Submission

The downstream Amaru node exposes the local Submit API at `DOWNSTREAM_SUBMIT_API_ADDRESS`. Submit valid preprod
CBOR-encoded transactions to inject them into the downstream mempool:

```bash
./demo.sh submit /path/to/tx-1.cbor /path/to/tx-2.cbor
```

Or submit automatically from a tmux pane after startup:

```bash
export TX_CBOR_FILES="/path/to/tx-1.cbor /path/to/tx-2.cbor"
./demo.sh
```

To generate fresh transactions at runtime, provide a funded preprod payment signing key. The script derives the address
when needed, queries the upstream Haskell node for UTxOs at that address, builds up to `TX_GENERATED_COUNT` independent
self-transfer transactions, signs them as canonical CBOR, and submits them through downstream Amaru:

```bash
export TX_PAYMENT_SKEY=/path/to/payment.skey
./demo.sh
```

The transactions must be valid for the downstream node's current ledger state. When accepted, the watch pane shows
Submit API, mempool, and tx-submission logs (`RequestTx*` / `ReplyTx*`) as the upstream Amaru node pulls the
transaction from the downstream Amaru node.

If the address only has a smaller UTxO, the generator falls back to a single transaction from the largest input it can
find. To require a lower threshold for the multi-transaction path, set `TX_MIN_INPUT_LOVELACE` accordingly.

A hardcoded transaction is unlikely to be accepted on a live network because transaction inputs are consumed once and
validity intervals expire. To see several transactions in the mempool at once, fund the address with several separate
UTxOs.
