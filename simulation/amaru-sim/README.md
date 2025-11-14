# Amaru Simulator

This component aims at implementing a _Simulator_ for the Ouroboros Consensus,
in Rust, using Amaru components. The main goal of this work is to be able to
test the consensus part as deeply as possible, using different strategies, in
increasing order of fidelity:

1. ✅ In-process deterministic testing, completely simulating the environment,
   allowing arbitrary fault injections and full control over concurrency and
   other side-effects
2. 🚧 [Maelstrom](https://github.com/jepsen-io/maelstrom/)-like testing through
   stdin/stdout interface ignoring network interactions
3. 🚧 [Jepsen](https://github.com/jepsen-io/jepsen)-like testing through
   full-blown deployment of a cluster and actual networking stack
4. ✅ [Antithesis](https://antithesis.com) support

# In-process deterministic testing

This testing strategy uses a discrete-event simulator to model a network of nodes, receiving and sending messages to each other, simulating network delays,
and allowing for controlled fault injections.

A simulator run consists in n tests.
For each test, the simulator performs three main steps:

 1. The generation of test data (see [`generate.rs`](src/simulator/data_generation/generate.rs)), which includes:
    * A random tree of headers representing the blockchain;
    * A sequence of messages (roll forward and rollback) that peers will send to the node under test, based on that tree.
    * Some random delays reflected in messages arrival times.

 2. The execution of the simulator (see [`simulate.rs`](src/simulator/simulate.rs)), which will:
    * Send input messages to nodes, one by one,
    * Randomly activate the nodes
      * Run idle node stages to make progress on the processing of messages.
      * 🚧Wake-up sleeping stages that are waiting for the passing of time.
      * 🚧Triggering time-outs
      * 🚧Inject faults.
    * Collect output messages.
    * Recurse until all input messages have been processed and no more progress can be made.

 3. The verification of properties for each node (see [`simulator/mod.rs`](src/simulator/mod.rs)):
    * Given a set of input messages arriving on a node we compute what should be the best chain, and compare it to
      the chain retrieved from output messages from that node.

## Run a simulation test

[A Rust `#test`](tests/simulation.rs) can be executed with the following parameters (environment variables can be used to override
options, we show the default values here):

```bash
AMARU_NUMBER_OF_TESTS=50  \
  AMARU_NUMBER_OF_NODES=1 \
  AMARU_NUMBER_OF_UPSTREAM_PEERS=2 \
  AMARU_NUMBER_OF_DOWNSTREAM_PEERS=1 \
  AMARU_DISABLE_SHRINKING=true \
  AMARU_TEST_SEED= \
  AMARU_PERSIST_ON_SUCCESS=false \
  AMARU_SIMULATION_LOG=error \
  AMARU_SIMULATION_LOG_AS_JSON=false \
  cargo test run_simulator
```

> [!TIP]
> To run those tests from the toplevel directory of the Amaru repository, one needs to explicitly sets the crate name:
>
> ```
> cargo test -p amaru-sim run_simulator
> ```

## Read test failures

When the test fails, the output looks something like this:

```text
Failed after 2 tests

 Minimised input (11 shrinks):

 History:

    0.  "c1" ==> "n1"   Fwd { msg_id: 0, slot: Slot(1), hash: "6eae61", parent_hash: "n/a", header: "828a0101" }
    1.  "c2" ==> "n1"   Fwd { msg_id: 0, slot: Slot(1), hash: "6eae61", parent_hash: "n/a", header: "828a0101" }
    2.  "c1" ==> "n1"   Fwd { msg_id: 1, slot: Slot(2), hash: "f0ffd0", parent_hash: "6eae61", header: "828a0202" }
    3.  "c2" ==> "n1"   Fwd { msg_id: 1, slot: Slot(2), hash: "f0ffd0", parent_hash: "6eae61", header: "828a0202" }
    4.  "c1" ==> "n1"   Fwd { msg_id: 2, slot: Slot(3), hash: "060280", parent_hash: "f0ffd0", header: "828a0303" }
    5.  "c2" ==> "n1"   Fwd { msg_id: 2, slot: Slot(3), hash: "060280", parent_hash: "f0ffd0", header: "828a0303" }
    6.  "c1" ==> "n1"   Fwd { msg_id: 3, slot: Slot(4), hash: "40c7eb", parent_hash: "060280", header: "828a0404" }
    7.  "c2" ==> "n1"   Fwd { msg_id: 3, slot: Slot(4), hash: "40c7eb", parent_hash: "060280", header: "828a0404" }
    8.  "c2" ==> "n1"   Fwd { msg_id: 4, slot: Slot(5), hash: "8704c4", parent_hash: "40c7eb", header: "828a0505" }
    9.  "c2" ==> "n1"   Fwd { msg_id: 5, slot: Slot(6), hash: "cfd69e", parent_hash: "8704c4", header: "828a0606" }
   10.  "c2" ==> "n1"   Fwd { msg_id: 6, slot: Slot(7), hash: "2b44ec", parent_hash: "cfd69e", header: "828a0707" }
   [...]

 Error message:

The actual chain

1. 6eae61ead131208ad52eb7c891648476a0dd78a6dc766f420eb22c57b986ca82,
  2. f0ffd050ed2ef488363c71fdfcc0e9a3c32826ec5c255f1c807ea42b00ffa965 (6eae61ead131208ad52eb7c891648476a0dd78a6dc766f420eb22c57b986ca82),
  3. 06028066fd6b00ac4cac12f1591f1829906815dcbc32c527e57a7798336e3f46 (f0ffd050ed2ef488363c71fdfcc0e9a3c32826ec5c255f1c807ea42b00ffa965),
  4. 40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350 (06028066fd6b00ac4cac12f1591f1829906815dcbc32c527e57a7798336e3f46),
  5. 8704c4cd06842a0b33c054f693a7801cdd94c4b76c99ab8e7f1454de188851b3 (40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350),
  6. cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433 (8704c4cd06842a0b33c054f693a7801cdd94c4b76c99ab8e7f1454de188851b3),
  7. 2b44ecdf60cd9ddecc90c0eae3f7aa6ae6e93a79ea96fae42d85379d8b0a6785 (cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433),
  10. e33dbe3482257359b37b30e06bb0a4541bd6fb5c21355a57e2e7c42fce3da6c4 (f0f7981a30f5d7e320e46685c37e528beed327ad1f72481660b8aa070e86ec53)

is not in the best chains

[1. 6eae61ead131208ad52eb7c891648476a0dd78a6dc766f420eb22c57b986ca82,
  2. f0ffd050ed2ef488363c71fdfcc0e9a3c32826ec5c255f1c807ea42b00ffa965 (6eae61ead131208ad52eb7c891648476a0dd78a6dc766f420eb22c57b986ca82),
  3. 06028066fd6b00ac4cac12f1591f1829906815dcbc32c527e57a7798336e3f46 (f0ffd050ed2ef488363c71fdfcc0e9a3c32826ec5c255f1c807ea42b00ffa965),
  4. 40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350 (06028066fd6b00ac4cac12f1591f1829906815dcbc32c527e57a7798336e3f46),
  5. 8704c4cd06842a0b33c054f693a7801cdd94c4b76c99ab8e7f1454de188851b3 (40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350),
  6. cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433 (8704c4cd06842a0b33c054f693a7801cdd94c4b76c99ab8e7f1454de188851b3),
  7. 2b44ecdf60cd9ddecc90c0eae3f7aa6ae6e93a79ea96fae42d85379d8b0a6785 (cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433)],
  [...]

The headers tree is
1. 6eae61ead131208ad52eb7c891648476a0dd78a6dc766f420eb22c57b986ca82
    └── 2. f0ffd050ed2ef488363c71fdfcc0e9a3c32826ec5c255f1c807ea42b00ffa965 (6eae61ead131208ad52eb7c891648476a0dd78a6dc766f420eb22c57b986ca82)
        └── 3. 06028066fd6b00ac4cac12f1591f1829906815dcbc32c527e57a7798336e3f46 (f0ffd050ed2ef488363c71fdfcc0e9a3c32826ec5c255f1c807ea42b00ffa965)
            └── 4. 40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350 (06028066fd6b00ac4cac12f1591f1829906815dcbc32c527e57a7798336e3f46)
                ├── 5. 8704c4cd06842a0b33c054f693a7801cdd94c4b76c99ab8e7f1454de188851b3 (40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350)
                │   └── 6. cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433 (8704c4cd06842a0b33c054f693a7801cdd94c4b76c99ab8e7f1454de188851b3)
                │       └── 7. 2b44ecdf60cd9ddecc90c0eae3f7aa6ae6e93a79ea96fae42d85379d8b0a6785 (cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433)
                └── 5. f8aad20c35ec41dbd8fddac5be632f8080134a59bf8f9ce3bf5c59bd45a6f4e9 (40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350)
                    └── 6. d6e6844e8a5ed7a91e0ccd764b7ae9a1e2460041e8b1a673f446aa1353acc4bd (f8aad20c35ec41dbd8fddac5be632f8080134a59bf8f9ce3bf5c59bd45a6f4e9)
                        └── 7. 881a39be93ca4b16ab716f42494e4777089c1c570c9c669eb88f8f6523bd20a5 (d6e6844e8a5ed7a91e0ccd764b7ae9a1e2460041e8b1a673f446aa1353acc4bd)
                            ├── 8. 9859841981376654de9f490383eee4591ef7eff2c6b77d9f8be7b5a7d60c3836 (881a39be93ca4b16ab716f42494e4777089c1c570c9c669eb88f8f6523bd20a5)
                            └── 8. a4e6aee20490b2f9c4207234af0cfc4053fdc86faa4f2ae15d783302b7bfa7c3 (881a39be93ca4b16ab716f42494e4777089c1c570c9c669eb88f8f6523bd20a5)
                                └── 9. f0f7981a30f5d7e320e46685c37e528beed327ad1f72481660b8aa070e86ec53 (a4e6aee20490b2f9c4207234af0cfc4053fdc86faa4f2ae15d783302b7bfa7c3)
                                    └── 10. e33dbe3482257359b37b30e06bb0a4541bd6fb5c21355a57e2e7c42fce3da6c4 (f0f7981a30f5d7e320e46685c37e528beed327ad1f72481660b8aa070e86ec53)


The actions are

r#"{"RollForward":{"peer":"1","header":{"hash":"6eae61ead131208ad52eb7c891648476a0dd78a6dc766f420eb22c57b986ca82","block":1,"slot":1,"parent":null}}}"#,
r#"{"RollForward":{"peer":"2","header":{"hash":"6eae61ead131208ad52eb7c891648476a0dd78a6dc766f420eb22c57b986ca82","block":1,"slot":1,"parent":null}}}"#,
r#"{"RollForward":{"peer":"1","header":{"hash":"f0ffd050ed2ef488363c71fdfcc0e9a3c32826ec5c255f1c807ea42b00ffa965","block":2,"slot":2,"parent":"6eae61ead131208ad52eb7c891648476a0dd78a6dc766f420eb22c57b986ca82"}}}"#,
r#"{"RollForward":{"peer":"2","header":{"hash":"f0ffd050ed2ef488363c71fdfcc0e9a3c32826ec5c255f1c807ea42b00ffa965","block":2,"slot":2,"parent":"6eae61ead131208ad52eb7c891648476a0dd78a6dc766f420eb22c57b986ca82"}}}"#,
r#"{"RollForward":{"peer":"1","header":{"hash":"06028066fd6b00ac4cac12f1591f1829906815dcbc32c527e57a7798336e3f46","block":3,"slot":3,"parent":"f0ffd050ed2ef488363c71fdfcc0e9a3c32826ec5c255f1c807ea42b00ffa965"}}}"#,
r#"{"RollForward":{"peer":"2","header":{"hash":"06028066fd6b00ac4cac12f1591f1829906815dcbc32c527e57a7798336e3f46","block":3,"slot":3,"parent":"f0ffd050ed2ef488363c71fdfcc0e9a3c32826ec5c255f1c807ea42b00ffa965"}}}"#,
r#"{"RollForward":{"peer":"1","header":{"hash":"40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350","block":4,"slot":4,"parent":"06028066fd6b00ac4cac12f1591f1829906815dcbc32c527e57a7798336e3f46"}}}"#,
r#"{"RollForward":{"peer":"2","header":{"hash":"40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350","block":4,"slot":4,"parent":"06028066fd6b00ac4cac12f1591f1829906815dcbc32c527e57a7798336e3f46"}}}"#,
r#"{"RollForward":{"peer":"1","header":{"hash":"8704c4cd06842a0b33c054f693a7801cdd94c4b76c99ab8e7f1454de188851b3","block":5,"slot":5,"parent":"40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350"}}}"#,
r#"{"RollForward":{"peer":"2","header":{"hash":"8704c4cd06842a0b33c054f693a7801cdd94c4b76c99ab8e7f1454de188851b3","block":5,"slot":5,"parent":"40c7eb08b47b4c9dd90f4d714d1d91fbc8263adba4171f2c935dfbe643565350"}}}"#,
r#"{"RollForward":{"peer":"1","header":{"hash":"cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433","block":6,"slot":6,"parent":"8704c4cd06842a0b33c054f693a7801cdd94c4b76c99ab8e7f1454de188851b3"}}}"#,
r#"{"RollForward":{"peer":"2","header":{"hash":"cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433","block":6,"slot":6,"parent":"8704c4cd06842a0b33c054f693a7801cdd94c4b76c99ab8e7f1454de188851b3"}}}"#,
r#"{"RollForward":{"peer":"1","header":{"hash":"2b44ecdf60cd9ddecc90c0eae3f7aa6ae6e93a79ea96fae42d85379d8b0a6785","block":7,"slot":7,"parent":"cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433"}}}"#,
r#"{"RollForward":{"peer":"2","header":{"hash":"2b44ecdf60cd9ddecc90c0eae3f7aa6ae6e93a79ea96fae42d85379d8b0a6785","block":7,"slot":7,"parent":"cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433"}}}"#,
r#"{"RollBack":{"peer":"1","rollback_point":"7.cfd69e7bcd110854225189bf4d59854f6479de660ea1d368053ecc880d602433"}}"#,
[...]

 Seed: 9860506592519546157
```

This is a lot of information. Let's break it down:

* The `History` presents the list of messages sent from the peers to the node under test but also the message emitted by the simulated nodes.
* The `Error message` display the chain selection property failure;
* The `Headers tree` shows the full tree of headers that was generated to produce roll forwards and rollback messages.
* The `Actions` parts displays a list of actions that can be pasted to a unit test in the [`headers_tree.rs`](../amaru-consensus/src/consensus/headers_tree.rs) file to reproduce the failure if the issue is
  specifically related to the chain selection algorithm.
* The seed is what was used to produce the test case, it can be used to replay the full test (see `AMARU_TEST_SEED` above).

## Debug a test failure

Re-running the chain selection algorithm in `headers_tree.rs` might not be enough to understand what went wrong.
To help with debugging, you can:

 1. Visualize the generated input data with [`entries.html`](tests/data/entries.html)
 2. Visualize the execution of each stage with [`trace.html`](tests/data/trace.html)

### Debugging data

When a simulation is run, if it fails, or if it succeeds and `AMARU_PERSIST_ON_SUCCESS` is set to true,
the generated data and the pure-stage traces are persisted in the top-level `target` directory:
```text
  tests
    ├── latest↗
    ├── 1762347647
    └── 1762347656
        ├── latest↗
        ├── args.json
        ├── traces.json
        ├── traces.cbor
        └── test-1
            ├── actions.json
            └── entries.json
        ├── test-2
        └── test-3
```

* Data is organized by run (e.g. `1762347656/`), then by test (e.g. `test-1/`).
* Each run directory contains:
  * `latest↗` symlink to the latest run.
  * `args.json` this file contains the test seed (can be set on `AMARU_TEST_SEED` to reproduce the run) and other run arguments.
  * `traces.json` pure-stage traces, in JSON format.
  * `traces.cbor` pure-stage traces, in CBOR format. (in the future, only the CBOR format will be needed).
  * One directory per test (e.g. `test-1/`, `test-2/`, etc.).

* Each test directory contains:
  * `latest↗` symlink to the latest test in a run.
  * `entries.json` generated headers tree and the list of messages sent to the nodes under test.
  * `actions.json` the list of actions that can be used to reproduce a chain selection failure in `headers_tree.rs`.

### Visualize generated input data

In order to visualize the generated input data for a given test, open the `entries.html` file located in `tests/animations`.
Then load the `entries.json` file for the test you want to visualize.

This will display:

 * The generated tree of headers used to produce peer messages.
 * Labels of peers moving along that tree, showing at which time a given peer sent a roll forward or rollback message to the node under test.
 * If a label moves towards a leaf of the the tree, it indicates a roll forward message.
 * Otherwise if a label moves back up the tree, it indicates a rollback message.
 * Note that the peer label also provides the arrival time of the message.

### Visualize the node execution

In order to visualize the node execution during the simulation, open the `traces.html` file located in `tests/animations`.
Then load the `traces.json` file for the run you want to visualize.

This will display:

* All the `pure-stage` stages for the node (🚧this animation does not support several nodes yet).
* Successive traces of various types
  * `Input` input message is received by a given stage.
  * `Resume` resume a stage execution. That trace shows what other stages are still suspended.
  * `Suspend` stop the stage to execute an effect, like sending a message to another stage or use storage.
* Below each stage, a label showing when the last header hash was received as an input.
* Above each stage, if applicable, a label showing the external effect currently being executed.

# References

* [Cardano Consensus and Storage Layer](https://ouroboros-consensus.cardano.intersectmbo.org/assets/files/report-b72e7d765cfee85b26dc035c52c6de84.pdf)
* [Ouroboros Network Specification](https://ouroboros-network.cardano.intersectmbo.org/pdfs/network-spec/network-spec.pdf)
