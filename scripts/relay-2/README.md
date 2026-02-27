# Relay demo: Haskell -> Amaru -> Haskell

This demo shows the use of an Amaru node acting as a relay between two Haskell cardano-nodes:

```text
cardano-node ──────→ amaru ──────→ cardano-node
(port: 3001)         (peer: 3001,   (port: 3002,
                      listen: 4001)  topology→4001)
```

3 nodes total: 1 cardano-node source, 1 Amaru relay, 1 cardano-node downstream.

This differs from relay-1 which has `cardano-node -> Amaru -> Amaru`.

## Prerequisites

- A `cardano-node` executable (installed via package manager, built from source, etc.)
- **Two separate** directories with cardano-node configuration files:
    - Upstream: `config.json`, `topology.json` pointing to public network peers
    - Downstream: `config.json`, `topology.json` pointing to Amaru (127.0.0.1:4001)
- The amaru node checked-out from https://github.com/pragma-org/amaru and bootstrapped with
  `make AMARU_NETWORK=preprod bootstrap`

## Configuration

The following environment variables configure the demo:

| Variable                             | Required | Default        | Description                                                            |
|--------------------------------------|----------|----------------|------------------------------------------------------------------------|
| `CARDANO_NODE`                       | **Yes**  | -              | Path to the cardano-node executable                                    |
| `CARDANO_NODE_CONFIG_DIR`            | **Yes**  | -              | Directory for upstream cardano-node (config.json, topology.json, etc.) |
| `CARDANO_NODE_DOWNSTREAM_CONFIG_DIR` | **Yes**  | -              | Directory for downstream cardano-node                                  |
| `AMARU_DIR`                          | No       | Script-derived | Path to the amaru project directory                                    |
| `UPSTREAM_PORT`                      | No       | 3001           | Port for upstream cardano-node listener                                |
| `LISTEN_PORT`                        | No       | 4001           | Port for amaru listener (for downstream)                               |
| `DOWNSTREAM_PORT`                    | No       | 3002           | Port for downstream cardano-node listener                              |

## Downstream Topology Configuration

The downstream cardano-node's `topology.json` must point to amaru's listen address:

```json
{
  "bootstrapPeers": null,
  "localRoots": [
    {
      "accessPoints": [
        {
          "address": "127.0.0.1",
          "port": 4001
        }
      ],
      "advertise": false,
      "trustable": true,
      "valency": 1
    }
  ],
  "publicRoots": [],
  "useLedgerAfterSlot": -1
}
```

## Usage

```bash
export CARDANO_NODE=/path/to/cardano-node
export CARDANO_NODE_CONFIG_DIR=/path/to/upstream-config
export CARDANO_NODE_DOWNSTREAM_CONFIG_DIR=/path/to/downstream-config
./demo.sh           # start the demo
./demo.sh stop      # stop the demo
./demo.sh status    # check status
./demo.sh restart amaru       # restart the amaru node
./demo.sh restart downstream  # restart the downstream cardano-node
```

Running `./demo.sh` will open a tmux session starting the 3 nodes. The data flow is:

1. Upstream cardano-node starts and syncs from the network
2. Amaru connects to upstream and begins syncing blocks
3. Downstream cardano-node connects to Amaru and receives blocks through it

We should see blocks flowing: upstream -> amaru -> downstream.
