# cbor-fixture-generator

Generate **positive** and **negative** CBOR fixtures for amaru's Rust decoder
tests by using the [`cuddle`][cuddle] CLI against a CDDL schema.

Fixtures are created under `crates/amaru-kernel/tests/data/cbor.decode/<kind>/<era>/<hash>/`
and are picked up by the Rust test at `crates/amaru-kernel/tests/test_cbor_serialization.rs`.

## How it works

- **`cuddle gen`** parses [`conway.cddl`](./conway.cddl) (the Conway-era CDDL schema, bundled here from
  `cardano-ledger-conway-1.22.1.0`),
  then emits a random CBOR term that matches a named rule (either `block` or `transaction_body`).
- **`cuddle gen --negative`** uses [`antigen`][antigen]'s `zapAntiGen` to perturb the same generator so the result is
  intentionally malformed.
- A `--seed N` flag makes every run deterministic (the same seed always produces the same CBOR).

The companion script [`scripts/regenerate-cbor-fixtures`](../../scripts/regenerate-cbor-fixtures) loops over kind ×
mode × seed and writes the fixtures + a `meta.json` per sample.

## Prerequisites

- The `cuddle` CLI on `PATH` (we need >= 1.8.0.0 for `--seed`, `--negative`).
- GHC + cabal (used once to build cuddle if it isn't already on `PATH`).
- `xxd` (ships with macOS / vim packages on Linux) for hex to binary conversion.

The regenerate script auto-installs cuddle on demand:

```bash
cabal install cuddle --installdir=target/bin
```

## Run

```bash
make regenerate-cbor-fixtures
```

or directly:

```bash
./scripts/regenerate-cbor-fixtures
```

The seed and number of fixtures can be set via environment variables:

| Variable             | Default | Meaning                                  |
|----------------------|---------|------------------------------------------|
| `CBOR_FIXTURE_COUNT` | `64`    | Fixtures per (kind × mode)               |
| `CBOR_FIXTURE_SEED`  | `42`    | Base seed; per-fixture seed = base + idx |

## Output layout

```
<repo>/crates/amaru-kernel/tests/data/cbor.decode/
  block/conway/<hash>/sample.cbor + meta.json
  transaction_body/conway/<hash>/sample.cbor + meta.json
```

`<hash>` is the blake2b-256 hex digest of `sample.cbor`.

## Verifying

```bash
cargo test -p amaru-kernel --test test_cbor_serialization -- --no-capture
```

That test collects all fixtures and run them to check if they are accepted or rejected by the decoder as expected.

## A note on the CDDL <-> amaru divergence

Currently the regenerate script discards any generated fixture the amaru disagrees with:

- Positive fixtures that amaru rejects.
- Negatives fixtures that amaru accepts.

With the current Conway CDDL + amaru decoders, a large fraction of cuddle-generated *positives*
are discarded. The CDDL is permissive about things amaru validates strictly (address header bytes, integer widths on
transaction indices, etc...).
The script reports the discard count on each run; if positives are unusually thin, you can increase the
`CBOR_FIXTURE_COUNT`
to make sure that enough fixtures will be kept.

This gap surfaces real CDDL/amaru semantic differences that need to be addressed in `amaru`.

## Refreshing conway.cddl

`conway.cddl` is checked in so generation is offline. It can be updated with:

```bash
curl -sL https://raw.githubusercontent.com/IntersectMBO/cardano-ledger/cardano-ledger-conway-<VERSION>/eras/conway/impl/cddl/data/conway.cddl \
    -o tooling/cbor-fixture-generator/conway.cddl
```

[cuddle]: https://github.com/input-output-hk/cuddle

[antigen]: https://github.com/input-output-hk/antigen
