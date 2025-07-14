# Amaru Simulator

This component aims at implementing a _Simulator_ for the Ouroboros Consensus,
in Rust, using Amaru components. The main goal of this work is to be able to
test the consensus part as deeply as possible, using different strategies, in
increasing order of fidelity:

1. ✅ In-process deterministic testing, completely simulating the environment,
   allowing arbitrary fault injections and full control over concurrency and
   other side-effects
2. ✅ [Maelstrom](https://github.com/jepsen-io/maelstrom/)-like testing through
   stdin/stdout interface ignoring network interactions
3. 🔴 [Jepsen](https://github.com/jepsen-io/jepsen)-like testing through
   full-blown deployment of a cluster and actual networking stack
4. 🔴 [Antithesis](https://antithesis.com) support

## Usage

The `simulator` test uses is a pared-down version of Amaru where network
communications are abstracted away.

The test can be run as with default options follows:

```
PROPTEST_CASES=256                   # Set the number of test cases to generate. \
PROPTEST_MAX_SHRINK_ITERS=4294967295 # How many times to shrink, use 0 to disable. \
AMARU_TEST_SEED=                     # Seed to use to reproduce a test case. \
AMARU_PERSIST_ON_SUCCESS=            # Don't persist pure-stage schedule on success. \
AMARU_SIMULATION_LOG="error"         # Only show error-level logging. \
cargo test run_simulator
```

## Debugging failures

When the test fails, the output looks something like this:

```
Found minimal failing case:

  Envelope { src: "c1", dest: "n1", body: Fwd { msg_id: 0, slot: Slot(31), hash: "2487bd",
 header: "828a0118" } }
  Envelope { src: "c1", dest: "n1", body: Fwd { msg_id: 1, slot: Slot(38), hash: "4fcd1d",
 header: "828a0218" } }
  Envelope { src: "c1", dest: "n1", body: Fwd { msg_id: 2, slot: Slot(41), hash: "739307",
 header: "828a0318" } }
 [...]

History:
  XXX: to be implemented

Error message:

  assertion `left == right`
  left: "c1"
 right: "n1"

Saved schedule: "./failure-1752489042.schedule"

Seed: 42
```

Let's break the components down:

* The minimal failing test case is printed so that it can be copy-pasted into a
  `#test` to create a regression test;
* The history is the same as test case, but also contains the responses that we
  got back from the system under test. The history is what the property is
  checked against;
* The error message is how the property property failed;
* Saved schedule is the execution schedule from `pure-stage`, which can provide
  low-level details about how the stage processing happened. See the following
  [note](https://github.com/pragma-org/amaru/wiki/log-::-2025%E2%80%9006#debugging-simulation-tests-failure)
  for more details of how to use this information;
* The seed is what was used to produce the test case, it can be used to replay
  the test (see `AMARU_TEST_SEED` above).

## Module structure

The main modules of the simulator are:

* [`src/simulator/generate.rs`](src/simulator/generate.rs): which defines the
  test case generator, which uses the pre-generated block tree which is saved in
  [`tests/data/chain.json`](tests/data/chain.json);
* [`src/simulator/simulate.rs`](src/simulator/simulate.rs): in which the
  actually discrete-event simulator lives;
* [`src/simulator/mod.rs`](src/simulator/mod.rs): where the property-based test
  and property that uses the simulator is defined;
* [`tests/simulation.rs`](tests/simulation.rs): where the actual Rust `#test`
  is defined.

## References

* [Cardano Consensus and Storage
  Layer](https://ouroboros-consensus.cardano.intersectmbo.org/assets/files/report-b72e7d765cfee85b26dc035c52c6de84.pdf)
* [Ouroboros Network
  Specification](https://ouroboros-network.cardano.intersectmbo.org/pdfs/network-spec/network-spec.pdf)
