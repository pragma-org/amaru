# Stake Distribution Fixtures

This directory contains the cardano-node stake distribution fixtures used by Amaru's summary conformance tests.

## Generating cardano-node ledger snapshots

The expected JSON fixtures are derived from cardano-node ledger snapshots produced with `db-analyser`.
In practice, the easiest way to obtain those snapshots is to let Amaru orchestrate the process:

```console
cargo run --bin amaru -- snapshot create --network preview --epoch 1099
```

That command downloads the required immutable chunks from Mithril and then runs `db-analyser` to materialize the corresponding cardano-node ledger snapshots on disk.

If you already have a local cardano-node database, you can also run `db-analyser` directly. The extractor in [haskell-node-extractor](haskell-node-extractor) expects snapshot directories of the form `<slot>_db-analyser`.

## Extracting stake distributions

The reference JSON payloads are produced by the Haskell extractor in [haskell-node-extractor](haskell-node-extractor). It writes one file per epoch as `epoch_<N>.json`, which can then be compressed to `epoch_<N>.json.zst`.

The extractor needs two consecutive cardano-node snapshots:

- the snapshot for the target epoch
- the snapshot for the following epoch

This is intentional. Some values exposed by the Haskell ledger API, most notably the DRep voting stake used for ratification, lag by one epoch. To reconstruct the canonical stake distribution for epoch `N`, the extractor combines the snapshot taken for epoch `N` with the snapshot from epoch `N + 1`, and rejects non-consecutive pairs.

The extractor README documents the exact command-line usage and validation flow.

## Generated Rust tests

[build.rs](../../../build/build.rs) scans `tests/conformance/stake-distributions/<network>/` for `epoch_*.json` and `epoch_*.json.zst` files and generates the matching `test_case` entries automatically. There is no manually-maintained test list anymore. The fixture directory is watched by cargo so tests regenerate when snapshots are added, removed, or replaced.

Availability of the matching epoch in the local `ledger.<network>.db` is **not** decided at build time (watching that live RocksDB would rebuild `amaru` on every node write). Each generated test checks for the snapshot at runtime: if it is missing, the test succeeds after printing a warning (visible with `--nocapture`).

To list or run the generated comparison tests for a given network:

```console
cargo test -p amaru --test summary -- --list
cargo test -p amaru --test summary --nocapture
```

Those tests compare the extracted JSON fixtures against stake distributions computed by Amaru from the local `ledger.<network>.db` store at the repository root, for example `ledger.preview.db`.

## Network-specific Makefiles

Each network directory such as [preview](preview), [preprod](preprod), and [mainnet](mainnet) contains a tiny `Makefile` that includes the shared one in this directory.

Use `make help` from one of those network directories to see the available utilities. 
They cover local fixture compression/decompression, listing, downloading, and uploading the corresponding S3 bucket contents. 
The S3 helpers work on individual `epoch_*.json.zst` objects and only transfer files that are missing locally or remotely.

`make download` needs no credentials: each network directory commits a `MANIFEST` file listing its `epoch_*.json.zst` 
fixtures, and the target downloads the missing ones over plain HTTPS from the bucket's public URL. The public R2 domain 
does not support bucket listing, which is why the manifest exists. Checking it into the repository also pins the fixture
 set per commit, so the generated conformance tests are reproducible across machines.

When adding fixtures, `make upload` pushes the missing `epoch_*.json.zst` objects and then refreshes `MANIFEST` from 
the local files. Commit the updated manifest in the same pull request. 

`make manifest` does the refresh on its own and only ever adds entries (removals are manual edits). It only records 
compressed `epoch_*.json.zst` files, so run `make zst` first when starting from plain `epoch_*.json` fixtures. 
`make upload` and `make list` talk to the S3 API and require:
 
 - `ENDPOINT` (the `https://<account-id>.r2.cloudflarestorage.com` endpoint, not the public URL).
 - `AWS_ACCESS_KEY_ID`.
 - `AWS_SECRET_ACCESS_KEY`.
