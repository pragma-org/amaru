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

## Make targets

To see the available make targets, just run:

```console
make help
```
