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
Here we want to make sure OpenTelemtry traces are considered as part of the API.

## Real chan tests

`make test-e2e` is run per PR. Runs preprod
On main branch: run upto the latest block

## Conformance tests

`make generate-test-snapshots`
TODO add details

## Simulation

TODO add details

## Antithesis

TODO add details