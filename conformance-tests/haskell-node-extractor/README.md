# Haskell node extractor

This tool reads Conway ledger snapshots from disk and extracts JSON fixtures for conformance testing.

## Prerequisites

To check for any missing prerequisites, just run:

```console
make pre
```

Install any missing system or vendor dependency.

## Build

```console
make build
```

> [!TIP]
>
> The Makefile defaults `PREFIX` to `brew --prefix` when [Homebrew](brew.sh) is available, and otherwise falls back to `/usr/local`.
> If your local prefix is different, override it explicitly. For example on Apple Silicon Homebrew:
>
>
> ```console
> make secp256k1 PREFIX=/opt/homebrew
> ```

## Run

Simply run:

```console
cabal run exe:haskell-node-extractor -- --help
```

### Example

Given a snapshot directory produced by `db-analyser` (or `cargo run create-snapshots`), extract all conformance fixtures:

```console
cabal run exe:haskell-node-extractor -- extract --preprod \
  --snapshot ../../snapshots/preprod/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912 \
  --output ../../conformance-tests/data
```

## Make targets

To see the available make targets, just run:

```console
make help
```
