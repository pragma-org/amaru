# Bootstrap Snapshots

Amaru bootstrap expects a window of three consecutive epoch snapshots. The runtime discovers that window from `<network>/index.json` in the configured S3-compatible bucket. See [Publishing bootstrap snapshots](./PUBLISHING_SNAPSHOTS.md) to publish a generated snapshot set.

## Create a Snapshot Set

### Prerequisites

- `db-analyser` on `$PATH` — known working version: `11.0.1` (ships with [cardano-node releases](https://github.com/IntersectMBO/cardano-node/releases))
- Internet access for Koios (epoch/block metadata) and Mithril (cardano-db download)

### Running the command

Generate a bootstrap set by passing the target starting epoch for Amaru to `amaru snapshot create`. For example, to start in epoch 166:

```shell
cargo run --bin amaru -- snapshot create --network preprod --epoch 166
```

This creates the snapshots for epochs `163`, `164`, and `165` on `preprod`:

The command is fully resumable: Mithril downloads are skipped when the local cardano-db already covers all target slots, and db-analyser work is reused when a matching snapshot directory already exists on disk.

### Steps performed for each target epoch

1. **Fetch block metadata** — queries Koios for the last block of the epoch (slot, hash, parent point).
2. **Download or resume cardano-db** — synchronises immutable files from Mithril up to the required slot; skipped entirely when local data already covers all target slots.
3. **Run db-analyser** — invokes `db-analyser --store-ledger <slot>` to produce a raw ledger state snapshot.
4. **Materialize snapshot** — assembles the snapshot directory at `snapshots/<network>/<slot>.<hash>/` (see [Snapshot format](#snapshot-format) below).
5. **Archive** — compresses the directory into `snapshots/<network>/<slot>.<hash>.tar.zst`.

### Snapshot format

Each materialized snapshot directory contains:

```
<slot>.<hash>/
├── bootstrap.headers.json   # JSON array of exactly two hex-encoded CBOR block headers
│                            # that immediately follow the snapshot point. Extracted
│                            # directly from the Mithril immutable .chunk files.
├── meta                     # Metadata directory produced by db-analyser
├── state                    # Ledger state directory produced by db-analyser
├── tables/
│   └── tvar                 # Binary ledger-state tables file produced by db-analyser
│                            # (db-analyser writes this as a flat 'tables' file;
│                            # amaru snapshot create relocates it to tables/tvar on materialization)
```
