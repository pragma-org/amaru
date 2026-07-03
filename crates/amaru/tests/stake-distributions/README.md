# Stake Distribution Fixtures

This directory contains the cardano-node stake distribution fixtures used by Amaru's summary conformance tests.

## Generating cardano-node ledger snapshots

The expected JSON fixtures are derived from cardano-node ledger snapshots produced with `db-analyser`.
In practice, the easiest way to obtain those snapshots is to let Amaru orchestrate the process:

```console
cargo run --bin amaru -- snapshot create --network preview --epoch 1099
```

That command downloads the required immutable chunks from Mithril and then runs `db-analyser` to materialize the corresponding cardano-node ledger snapshots on disk.

If you already have a local cardano-node database, you can also run `db-analyser` directly. The extractor in [haskell-node-extractor](/Users/ktorz/Documents/Projects/PRAGMA/amaru/crates/amaru/tests/stake-distributions/haskell-node-extractor) expects snapshot directories of the form `<slot>_db-analyser`.

## Extracting stake distributions

The reference JSON payloads are produced by the Haskell extractor in [haskell-node-extractor](/Users/ktorz/Documents/Projects/PRAGMA/amaru/crates/amaru/tests/stake-distributions/haskell-node-extractor). It writes one file per epoch as `epoch_<N>.json`, which can then be compressed to `epoch_<N>.json.xz`.

The extractor needs two consecutive cardano-node snapshots:

- the snapshot for the target epoch
- the snapshot for the following epoch

This is intentional. Some values exposed by the Haskell ledger API, most notably the DRep voting stake used for ratification, lag by one epoch. To reconstruct the canonical stake distribution for epoch `N`, the extractor combines the snapshot taken for epoch `N` with the snapshot from epoch `N + 1`, and rejects non-consecutive pairs.

The extractor README documents the exact command-line usage and validation flow.

## Generated Rust tests

[build.rs](/Users/ktorz/Documents/Projects/PRAGMA/amaru/crates/amaru/build.rs) scans `tests/stake-distributions/$AMARU_NETWORK/` for `epoch_*.json` and `epoch_*.json.xz` files and generates the matching `test_case` entries automatically. There is no manually-maintained test list anymore.

To list or run the generated comparison tests for a given network:

```console
AMARU_NETWORK=preview cargo test -p amaru --test summary -- --list
AMARU_NETWORK=preview cargo test -p amaru --test summary
```

Those tests compare the extracted JSON fixtures against stake distributions computed by Amaru from the local `ledger.<network>.db` store at the repository root, for example `ledger.preview.db`.

## Network-specific Makefiles

Each network directory such as [preview](/Users/ktorz/Documents/Projects/PRAGMA/amaru/crates/amaru/tests/stake-distributions/preview), [preprod](/Users/ktorz/Documents/Projects/PRAGMA/amaru/crates/amaru/tests/stake-distributions/preprod), and [mainnet](/Users/ktorz/Documents/Projects/PRAGMA/amaru/crates/amaru/tests/stake-distributions/mainnet) contains a tiny `Makefile` that includes the shared one in this directory.

Use `make help` from one of those network directories to see the available utilities. They cover local fixture compression and archive management, plus listing, downloading, and uploading the corresponding S3 bucket contents.
