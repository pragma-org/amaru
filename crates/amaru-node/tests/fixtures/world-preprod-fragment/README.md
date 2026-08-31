# Preprod fragment fixture for WorldLoop

The dissemination test primes one world node from a real preprod chain/ledger
store and starts the other node from bootstrap state only. The fragment HEAD
must appear on every honest node after `WorldLoop`.

The RocksDB stores are too large to commit. This directory keeps the discovery
inputs (`index.json`, `meta.json`). Missing `bootstrap/` and `primed/` are
produced by the recorded-data tests themselves (CDN bootstrap, then live
`run_until` of `meta.json`'s `target_epoch` from embedded big-ledger peers).
Progress prints to stderr.

## Discover the target epoch (do not guess)

Amaru bootstrap lists snapshots from `<public_url>/preprod/index.json`
(`amaru-bootstrap::AnonymousS3Client::list_snapshots`, same URL as
`DEFAULT_PUBLIC_URL`). Each entry is a `<slot>.<hash>` point. Epochs come from
`EraHistory::slot_to_epoch_unchecked_horizon` on preprod. The latest snapshot
state is the maximum of those epochs. Snapshots sit at the end of that epoch, so
`run_until` stops at the first block of `latest + 2` and the fragment is the
whole following epoch (~432000 slots, on the order of 10⁴ blocks).

Refresh the committed index, then recompute:

```sh
curl -sS https://pub-b844360df4774bb092a2bb2043b888e5.r2.dev/preprod/index.json \
  -o crates/amaru-node/tests/fixtures/world-preprod-fragment/index.json

cargo test -p amaru-node --lib --features test-utils \
  test_target_epoch_is_discovered_from_bootstrap_index
```

Update `meta.json` if the latest published snapshot moved. The test fails when
`meta.json` disagrees with the index mapped through era history.

## Run the dissemination test

```sh
cargo test -p amaru-node --lib --features test-utils -- --ignored --nocapture \
  test_world_disseminates_preprod_fragment
```

The first run imports the published snapshot window from the workspace
`./snapshots/<network>/` cache (the same directory `amaru node bootstrap` uses)
and syncs one epoch. Missing archives are downloaded there, not under the crate.
Later runs reuse `bootstrap/` and `primed/`. Do not commit those directories.

`meta.json`'s `fragment_head` is the last header after the snapshot that has a
stored body (the linear HEAD). It is not the first epoch-boundary header.
