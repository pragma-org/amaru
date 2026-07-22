# Conformance tests

This tool validates phase-one conformance fixtures against the Haskell ledger implementation, so that Amaru's ledger rules can be compared against the reference node.

The fixtures live in [amaru-ledger/tests/data/phase-one](../../../../amaru-ledger/tests/data/phase-one), split into `pass` and `fail` cases.

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
cabal run exe:conformance -- --help
```

### Example

To validate a single phase-one fixture:

```console
cabal run -v0 exe:conformance -- validate-phase-one \
  --test-case ../../../../amaru-ledger/tests/data/phase-one/fail/bad-inputs-utxo.json
```

The command prints the test case label and either the expected outcome (`PASS` or the expected predicate failure), or a validation mismatch error.

To run the whole suite of fixtures:

```console
make test
```

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

You will also be able to run the `conformance` binary directly from the shell, without needing to prefix it with `cabal run`.

### Tricorder

You can start a [`tricoder`](https://github.com/atelier-hub/tricorder) daemon in the background to observe the compilation
of the executable and execution of the test suite in real time:

```console
> tricorder start
> tricorder ui
```
