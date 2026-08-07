# cbor-fixture-generator

Generate **positive** and **negative** CBOR fixtures for amaru's Rust decoder
tests by using the [`cuddle`][cuddle] CLI against a CDDL schema.

Fixtures are created under `crates/amaru-kernel/tests/data/cbor.decode/<kind>/<era>/<hash>/`
and are picked up by the Rust test at `crates/amaru-kernel/tests/test_cbor_serialization.rs`.

## How it works

- **`cuddle gen`** parses [`conway.cddl`](./conway.cddl) (the Conway-era CDDL schema, bundled here from
  `cardano-ledger-conway-1.23.0.0`),
  then emits a random CBOR term that matches a named rule (either `block` or `transaction_body`).
- **`cuddle gen --negative`** uses [`antigen`][antigen]'s `zapAntiGen` to perturb the same generator so the result is
  intentionally malformed.
- A `--seed N` flag makes every run deterministic (the same seed always produces the same CBOR).
- `--no-twiddle` is passed for negatives fixtures. Otherwise we might just switch from a definite to an 
  indefinite-length encoding and that would not produce a truly negative test case.
- The rules cuddle sees are `conway.cddl` with [`conway.overrides.cddl`](./conway.overrides.cddl) applied; see
  `build_effective_cddl` in the script.

The companion script [`scripts/regenerate-cbor-fixtures`](../../scripts/regenerate-cbor-fixtures) loops over kind ×
mode × seed and writes the fixtures + a `meta.json` per sample.

## Prerequisites

The pipeline needs `cuddle` >= 1.8.0.0 (for `--seed`, `--negative`), GHC + cabal to build it on first use, and a
handful of C libraries it links against. Two ways to get them:

### Option 1: Flox (recommended)

The repository ships a project-local [Flox](https://flox.dev/) environment at [
`.flox/env/manifest.toml`](../../.flox/env/manifest.toml)
that pins exactly what the script needs:

| Pinned                                                        | Purpose                                                      |
|---------------------------------------------------------------|--------------------------------------------------------------|
| `ghc`, `cabal-install`                                        | build the `cuddle` CLI (fetched lazily into `target/bin/`)   |
| `pkg-config`, `autoconf`, `libtool`                           | autotools + pkg-config lookup for cuddle's crypto stack      |
| `libsodium`, `secp256k1`, `blst` (all with `outputs = "all"`) | crypto libs cuddle's `cardano-crypto-*` deps link against    |
| `python3`                                                     | blake2b hashing + `meta.json` manipulation inside the script |

```bash
# Install Flox once: https://flox.dev/docs/install-flox/
flox activate
```

The activation exposes `ghc`, `cabal`, `pkg-config` and friends on `PATH` and points them at the pinned C libraries.
After that, `make regenerate-cbor-fixtures` runs end-to-end. The first cold run takes ~15–25 minutes
(Nix store download + the one-off `cabal install cuddle` compile). Subsequent runs should take seconds.

The Rust toolchain itself is **not** included in this Flox environemnt. It is pinned by [
`rust-toolchain.toml`](../../rust-toolchain.toml)
and provided by `rustup` (or your usual setup).

### Option 2: install manually

If you'd rather not use Flox, you can install the same set yourself (`brew install` on macOS, distro packages on Linux):

- GHC + cabal (e.g. via [ghcup](https://www.haskell.org/ghcup/))
- `pkg-config`, `autoconf`, `libtool`
- `libsodium`, `secp256k1`, `blst` (with their pkg-config / dev files)
- `python3`

The regenerate script auto-installs `cuddle` on demand:

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

When a generated (or on-chain) fixture exposes a behavior amaru doesn't share with the canonical CDDL interpretation,
the regenerate script *keeps* the fixture and stamps `"known_amaru_divergence": true` into its `meta.json`.
The Rust test (`test_cbor_serialization`) honors that flag and accepts any of the three failure modes it acknowledges:

- `amaru` rejects a CDDL-valid positive (`well_formed: true` + decode fails),
- `amaru` accepts a CDDL-invalid negative (`well_formed: false` + decode succeeds),
- `amaru`'s encoder doesn't reproduce the input bytes on a round-trip.

The script reports the annotation count on each run. With the current Conway CDDL + amaru decoders a
large fraction of cuddle-generated *positives* get flagged. The CDDL is permissive about things amaru validates
strictly (address header bytes, integer widths on transaction indices, etc.).
Keeping the fixtures makes the gap a visible, trackable signal: the count should *drop* over time as
amaru's decoders/encoders are fixed. Once a divergence is resolved upstream, drop the flag from the
fixture's `meta.json` so the test starts enforcing the canonical behavior.

If positives feel thin after annotation, bump `CBOR_FIXTURE_COUNT` so enough unflagged samples survive each round.

## Refreshing conway.cddl

`conway.cddl` is checked in so generation is offline. It can be updated with:

```bash
curl -sL https://raw.githubusercontent.com/IntersectMBO/cardano-ledger/cardano-ledger-conway-<VERSION>/eras/conway/impl/cddl/data/conway.cddl \
    -o crates/amaru/tests/conformance/serialization/cbor-fixture-generator/conway.cddl
```

Then re-run `make regenerate-cbor-fixtures`: the samples are generated from the grammar, so a
refreshed grammar invalidates the ones already committed. 

Check `conway.overrides.cddl` at the same time and update it if necessary. 
An override becomes dead weight once upstream fixes the rule it works around.

[cuddle]: https://github.com/input-output-hk/cuddle

[antigen]: https://github.com/input-output-hk/antigen
