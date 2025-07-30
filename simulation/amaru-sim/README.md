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

## Overview

The main components of the simulator are:

* Test case generation, found in
  [`src/simulator/generate.rs`](src/simulator/generate.rs), which uses the
  pre-generated block tree which is saved in
  [`tests/data/chain.json`](tests/data/chain.json);
* The (discrete-event) simulator itself, lives in
  [`src/simulator/simulate.rs`](src/simulator/simulate.rs);
* The property-based test and property that uses the simulator, defined in
  [`src/simulator/mod.rs`](src/simulator/mod.rs);
  and property that uses the simulator is defined;
* The actual Rust `#test` which gets picked up by `cargo test`, found in
  [`tests/simulation.rs`](tests/simulation.rs). 

## Usage

The `simulator` test is a pared-down version of Amaru where network
communications are abstracted away.

The test can be run as follows (environment variables can be used to override
options, we show the default values here):

```
AMARU_NUMBER_OF_TESTS=50             # Set the number of test cases to generate. \
AMARU_NUMBER_OF_NODES=1              # Set the number of nodes in a simulation. \
AMARU_NUMBER_OF_UPSTREAM_PEERS=2     # Set the number of upstream peers.
AMARU_DISABLE_SHRINKING=0            # Set to 1 to disable shrinking. \
AMARU_TEST_SEED=                     # Seed to use to reproduce a test case. \
AMARU_PERSIST_ON_SUCCESS=0           # Set to 1 to persist pure-stage schedule on success. \
AMARU_SIMULATION_LOG="error"         # Only show error-level logging. \
\
cargo test run_simulator
```

## Debugging failures

When the test fails, the output looks something like this:

```
 Minimised input (0 shrinks):

  Envelope { src: "c1", dest: "n1", body: Fwd { msg_id: 0, slot: Slot(31), hash: "2487bd", header: "828a0118" } }
  Envelope { src: "c1", dest: "n1", body: Fwd { msg_id: 1, slot: Slot(38), hash: "4fcd1d", header: "828a0218" } }
  Envelope { src: "c1", dest: "n1", body: Fwd { msg_id: 2, slot: Slot(41), hash: "739307", header: "828a0318" } }
  Envelope { src: "c1", dest: "n1", body: Fwd { msg_id: 3, slot: Slot(55), hash: "726ef3", header: "828a0418" } }
  Envelope { src: "c1", dest: "n1", body: Fwd { msg_id: 4, slot: Slot(93), hash: "597ea6", header: "828a0518" } }
  [...]

History:

    0.  "c1" ==> "n1"   Fwd { msg_id: 0, slot: Slot(31), hash: "2487bd", header: "828a0118" }
    1.  "n1" ==> "c1"   Fwd { msg_id: 0, slot: Slot(31), hash: "2487bd", header: "828a0118" }
    2.  "c2" ==> "n1"   Fwd { msg_id: 0, slot: Slot(31), hash: "2487bd", header: "828a0118" }
    3.  "c2" ==> "n1"   Fwd { msg_id: 1, slot: Slot(38), hash: "4fcd1d", header: "828a0218" }
    4.  "n1" ==> "c2"   Fwd { msg_id: 1, slot: Slot(38), hash: "4fcd1d", header: "828a0218" }
    [...]

Error message:

  tip of chains don't match, expected:
    (Bytes { bytes: "fcb4a51..." }, Slot(990))
  got:
    (Bytes { bytes: "gcb4a51..." }, Slot(990))

Saved schedule: "./failure-1752489042.schedule"

Seed: 42
```

Let's break the components down:

* The minimised failing test case is printed so that it can be copy-pasted into a
  `#test` to create a regression test;
* The history is the same as test case, but the `src` and `dest` of each
  message has been pretty printed to be easier to read and it also contains the
  responses that we got back from the system under test. The history is what the
  property is checked against;
* The error message is how the property failed;
* Saved schedule is the execution schedule from `pure-stage`, which can provide
  low-level details about how the stage processing happened. See the following
  [note](https://github.com/pragma-org/amaru/wiki/log-::-2025%E2%80%9006#debugging-simulation-tests-failure)
  for more details of how to use this information;
* The seed is what was used to produce the test case, it can be used to replay
  the test (see `AMARU_TEST_SEED` above).

## References

* [Cardano Consensus and Storage
  Layer](https://ouroboros-consensus.cardano.intersectmbo.org/assets/files/report-b72e7d765cfee85b26dc035c52c6de84.pdf)
* [Ouroboros Network
  Specification](https://ouroboros-network.cardano.intersectmbo.org/pdfs/network-spec/network-spec.pdf)
