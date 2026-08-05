# Haskell node extractor

This tool reads Conway ledger snapshots from disk and extracts JSON fixtures for conformance testing.

## Prerequisites

To check for any missing prerequisites, just run:

```console
make prerequisites
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

## Utilities

### Continuous Analyser

To run db-analyser continuously and automatically on every epoch boundary, one can use `scripts/run-db-analyser.mjs`. This script requires a running instance of the (Haskell) cardano-node and Ogmios to resolve the last point of every epoch.

```console
./scripts/run-db-analyser.mjs <NETWORK>
```

### Schema Validator

The JSON Schema validator is managed with `uv` from this directory.

To materialize the local environment:

```console
uv sync
```

To validate a JSON value from stdin against one of the bundled schemas:

```console
uv run scripts/validate-json-schema --schema stake-distribution < /path/to/stake-distribution.json
```

`uv run` is enough for one-off use; `uv sync` is only needed if you want the local `.venv` eagerly created up front.

### Example

Given a snapshot directory produced by `db-analyser` (or `cargo run --bin amaru -- snapshot create`), validate the resulting stake distribution payload:

```console
cabal run -v0 exe:haskell-node-extractor -- stake-distribution \
  --preprod \
  --snapshot ../../snapshots/preprod/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912 \
  --next-snapshot ../../snapshots/preprod/69321198. \
  --output ./data
```

This writes the resulting file under `data/stake-distributions/<network>/<epoch>.json`, relative to the current working directory.

You can then validate the generated file with:

```console
uv run scripts/validate-json-schema --schema stake-distribution < ./data/stake-distributions/preprod/<epoch>.json
```

> [!TIP]
>
> To produce new cardano-node ledger snapshots easily, you can use ouroboros-consensus' db-analyser as follows:
>
> ```console
> db-analyser \
>   --in-mem \
>   --db path/to/cardano-node.db \
>   --config path/to/cardano-node-config.json \
>   --analyse-from START_SLOT \
>   --store-ledger TARGET_SLOT
> ```
>
> The first time, `--analyse-from` has no effect and the command will perform a
> full reconstruction from genesis. After that, it can pick up from the last
> snapshot.

## Make targets

To see the available make targets, just run:

```console
make help
```

## Flox environment

Some of the tools in this directory are managed with [Flox](https://flox.dev).
Once you have Flox installed, you can materialize the local environment with:

```console
flox activate
```

This will make a number of executables available in your shell:
```
❯ flox list
autoconf: autoconf (2.73)
automake: automake (1.18.1)
blst: blst (0.3.16)
cabal-install: cabal-install (3.16.1.0)
ghc: haskell.compiler.ghc96 (9.6.7)
gmp: gmp (6.3.0)
gnumake: gnumake (4.4.1)
libffi: libffi (40)
libsodium: libsodium (2026-04-09)
libtool: libtool (2.5.4)
llvm: llvm (21.1.8)
lmdb: lmdb (0.9.35)
openssl: openssl (3.6.2)
pcre: pcre (8.45)
pkg-config: pkg-config (0.29.2)
secp256k1: secp256k1 (0.7.1)
zlib: zlib (1.3.2)
```

You will also be able to run the `haskell-node-extractor` binary directly from the shell, without needing to prefix it with `cabal run`.
