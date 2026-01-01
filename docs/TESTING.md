# How amaru is tested?

This page is inspired by the famous [How SQLite Is Tested](https://www.sqlite.org/testing.html) page.

## Unit testing

Each individual crate can contain a number of unit tests, either inline in some source file, or in the `test` folder.
They are executed via `cargo test`.

### Property testing

[Property testing](https://github.com/proptest-rs/proptest) is used to improve unit testing.

### CLI

[cli-tests](./cli-tests) guarantee that no CLI regression are introduced.

### Traces testing

TODO
Here we want to make sure OpenTelemetry traces are considered as part of the API.

## Real chan tests

`make test-e2e` is run per PR. Runs preprod
On main branch: run upto the latest block

## Conformance tests

The `amaru-ledger` module runs conformance tests based on vectors from the [cardano-blueprint](https://github.com/cardano-scaling/cardano-blueprint/tree/main/src/ledger/conformance-test-vectors) repository. These are run as unit tests, executed via `cargo test`.

Not all conformance tests pass; we use a snapshot approach to track which tests are currently failing and why. To update that snapshot, run
`make update-ledger-conformance-test-snapshot`.

`make generate-test-snapshots`
TODO add details

## Simulation

The [amaru-sim](https://github.com/pragma-org/amaru/tree/main/simulation/amaru-sim) standalone application provides a thorough deterministic simulation testing framework specifically targeted at testing the _consensus_ and _network_ components of Amaru. Deterministic simulation testing is based on a simple idea, to simulate the environment the system is interacting with, inject inputs and verifies its output conforms to expected properties.

DST requires full control of the _effects_ enacted by the system, ie. everything that's not pure computation like sending or receiving data from the network, running concurrent computations, reading and writing to disk ; and we want to ensure we test the _exact same code_ that runs in production. Therefore we built [pure-stage](https://github.com/pragma-org/amaru/tree/main/crates/pure-stage) which is the machinery that allows us to isolate _effects_ and run concurrent code deterministically.

We currently run "small" simulation tests on every CI build, and "large" ones [nightly](https://github.com/pragma-org/amaru/actions/workflows/nightly-simulation.yml) on our main branch.

More details about this approach can be found in [this repository](https://github.com/pragma-org/simulation-testing).

## Antithesis

[Antithesis](https://antithesis.com) is an _autonomous testing_ service that provides end-to-end deterministic simulation testing environment with faults injection. It allows testing a complete Cardano network, comprised of standard OCI container images, at a 50x speed execution, injecting various kind of faults and checking the overall behaviour of the system.

These tests are run nightly on the `main` branch.
