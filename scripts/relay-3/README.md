# Relay demo: Haskell -> Amaru -> Amaru -> Amaru -> Haskell

This demo shows a chain of 3 Amaru relay nodes between two Haskell cardano-nodes:

```
cardano-node ──────→ amaru-upstream
(port: 3001)  │     (peer: 3001, listen: 4001)
              │            │
              │            ▼
              └────→ amaru-center ──────┬──→ amaru-downstream
                    (peers: 3001,       │   (peer: 4002, listen: 4003)
                     4001)              │
                    (listen: 4002)      └──→ cardano-node-downstream
                                            (port: 3002, topology→4002)
```

5 nodes total: 1 cardano-node source, 3 Amaru relays, 1 cardano-node downstream.

## Prerequisites

- A `cardano-node` executable (installed via package manager, built from source, etc.)
- **Two separate** directories with cardano-node configuration files:
  - Upstream: `config.json`, `topology.json` pointing to public network peers
  - Downstream: `config.json`, `topology.json` pointing to Amaru center (127.0.0.1:4002)
- The amaru node checked-out from https://github.com/pragma-org/amaru and bootstrapped with
  `make AMARU_NETWORK=preprod bootstrap`

## Configuration

The following environment variables configure the demo:

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `CARDANO_NODE` | **Yes** | - | Path to the cardano-node executable |
| `CARDANO_NODE_CONFIG_DIR` | **Yes** | - | Directory for upstream cardano-node (config.json, topology.json, etc.) |
| `CARDANO_NODE_DOWNSTREAM_CONFIG_DIR` | **Yes** | - | Directory for downstream cardano-node |
| `AMARU_DIR` | No | Script-derived | Path to the amaru project directory |
| `UPSTREAM_PORT` | No | 3001 | Port for upstream cardano-node listener |
| `AMARU_UPSTREAM_LISTEN_PORT` | No | 4001 | Port for amaru-upstream listener |
| `CENTER_LISTEN_PORT` | No | 4002 | Port for amaru-center listener |
| `AMARU_DOWNSTREAM_LISTEN_PORT` | No | 4003 | Port for amaru-downstream listener |
| `DOWNSTREAM_PORT` | No | 3002 | Port for downstream cardano-node listener |

## Downstream Topology Configuration

The downstream cardano-node's `topology.json` must point to amaru-center's listen address:

```json
{
  "bootstrapPeers": null,
  "localRoots": [
    {
      "accessPoints": [
        { "address": "127.0.0.1", "port": 4002 }
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
./demo.sh restart amaru-center      # restart amaru-center
./demo.sh restart cardano-downstream  # restart downstream cardano-node
```

Running `./demo.sh` will open a tmux session starting all 5 nodes. The data flow is:

1. Upstream cardano-node starts and syncs from the network
2. Amaru-upstream connects to upstream cardano-node and begins syncing blocks
3. Amaru-center connects to both upstream cardano-node and amaru-upstream
4. Amaru-downstream connects to amaru-center and receives blocks through it
5. Downstream cardano-node connects to amaru-center and receives blocks through it

We should see blocks flowing through the full relay chain.
