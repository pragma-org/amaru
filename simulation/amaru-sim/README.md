# Amaru Simulator

This component aims at implementing a _Simulator_ for the Ouroboros Consensus, in Rust, using Amaru components. The main goal of this work is to be able to test the consensus part as deeply as possible, using different strategies, in increasing order of fidelity:

1. ✅ In-process deterministic testing, completely simulating the environment, allowing arbitrary fault injections and full control over concurrency and other side-effects
2. ✅ [Maelstrom](https://github.com/jepsen-io/maelstrom/)-like testing through stdin/stdout interface ignoring network interactions
3. 🔴 [Jepsen](https://github.com/jepsen-io/jepsen)-like testing through full-blown deployment of a cluster and actual networking stack
4. 🔴 [Antithesis](https://antithesis.com) support

## Usage

The `simulator` executable is a pared-down version of Amaru where network communications are abstracted away. It's packaged as a test so running it amounts to:

```
cargo test -p amaru-sim
```

By default, it only logs `error` level and above log entries, but one filter logs by setting the `AMARU_SIMULATION_LOG` environment variable to an appropriate value.

## References

* [Cardano Consensus and Storage Layer](https://ouroboros-consensus.cardano.intersectmbo.org/assets/files/report-b72e7d765cfee85b26dc035c52c6de84.pdf)
* [Ouroboros Network Specification](https://ouroboros-network.cardano.intersectmbo.org/pdfs/network-spec/network-spec.pdf)
