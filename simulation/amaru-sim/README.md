# Amaru Simulator

This component aims at implementing a _Simulator_ for the Ouroboros Consensus, in Rust, using Amaru components. The main goal of this work is to be able to test the consensus part as deeply as possible, using different strategies, in increasing order of fidelity:

1. 🔴 In-process deterministic testing, completely simulating the environment, allowing arbitrary fault injections and full control over concurrency and other side-effects
2. ✅ [Maelstrom](https://github.com/jepsen-io/maelstrom/)-like testing through stdin/stdout interface ignoring network interactions
3. 🔴 [Jepsen](https://github.com/jepsen-io/jepsen)-like testing through full-blown deployment of a cluster and actual networking stack

## Usage

The `simulator` executable is a pared-down version of Amaru where network communications are abstracted away and fully controlled by [simulation-testing](https://github.com/pragma-org/simulation-testing) executable(s).
To run such tests, one needs to:

1. compile amaru simulator
2. compile and run `moskstraumen` components relevant for testing Amaru

### Compile simulator

Build the simulator in debug mode (from toplevel Amaru workspace):

```
cargo build -p amaru-sim
```

### Compile & Run moskstraumen

**NOTE**: This requires a fully functional Haskell toolchain which one can obtain from [GHCUp](http://ghcup.haskell.org)

Checkout simulation testing project

```
git clone https://github.com/pragma-org/simulation-testing
```

Change directory to moskstraumen:

```
cd simulation-testing/moskstraumen
```

Run test against simulator:

```
cabal run blackbox-test -- ../../amaru/target/debug/simulator amaru 1 1 \
   --disable-shrinking \
   --stake-distribution-file data/stake.json \
   --consensus-context-file data/context.json
```

The `stake.json` and `context.json` are files extracted from chain generation which one can find in `chain.json`. They are needed to provide enough context to validate "fake" headers.

If all goes well, one should see something like:

```
% cabal run blackbox-test -- ../../amaru/target/debug/simulator amaru 1 1 --disable-shrinking --stake-distribution-file data/stake.json --consensus-context-file data/context.json
{"timestamp":"2025-03-24T17:24:41.641906Z","level":"INFO","fields":{"message":"using upstream peer addresses: [\"c1\"]"},"target":"amaru_sim::simulator"}
("TRACEPREDICATE",47,47,2)
Success!
```

## References

* [Cardano Consensus and Storage Layer](https://ouroboros-consensus.cardano.intersectmbo.org/assets/files/report-b72e7d765cfee85b26dc035c52c6de84.pdf)
* [Ouroboros Network Specification](https://ouroboros-network.cardano.intersectmbo.org/pdfs/network-spec/network-spec.pdf)
