# Relay demo: Haskell -> Amaru -> Amaru

This demo shows the use of an Amaru node between a Haskell node (upstream) and another Amaru node (downstream).

## Prerequisites

- A `cardano-node` executable (installed via package manager, built from source, etc.)
- A directory with cardano-node configuration files (`config.json`, `topology.json`)
- The amaru node checked-out from https://github.com/pragma-org/amaru and bootstrapped with
  `make AMARU_NETWORK=preprod bootstrap`

## Configuration

The following environment variables configure the demo:

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `CARDANO_NODE` | **Yes** | - | Path to the cardano-node executable |
| `CARDANO_NODE_CONFIG_DIR` | **Yes** | - | Directory containing config.json, topology.json, etc. |
| `AMARU_DIR` | No | Current directory | Path to the amaru project directory |
| `UPSTREAM_PORT` | No | 3001 | Port for cardano-node listener |
| `LISTEN_PORT` | No | 4001 | Port for amaru listener (for downstream) |
| `DOWNSTREAM_LISTEN_PORT` | No | 4002 | Port for amaru downstream listener |

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
