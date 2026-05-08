# Phase-one validation fixtures

JSON test vectors for Cardano phase-one transaction validation. Each fixture
declares an initial ledger state, a transaction, and an expected outcome. The
format is implementation-generic; any Cardano implementation can consume these
to increase confidence their phase-one implementation conforms.

## Layout

```
pass/<name>.json
fail/<Predicate>/<n>.json
schema.json
```

`<Predicate>` matches the Haskell ledger's predicate-failure name (e.g.
`InvalidWitnessesUTXOW`). `<n>` distinguishes multiple cases for the same
predicate.

## Schema

The authoritative schema is [`schema.json`](./schema.json). A summary follows; consult the schema for exact types, ranges, and
required-field rules.

| Field                | Notes                                                  |
| -------------------- | ------------------------------------------------------ |
| `title`              | Optional. Informational; ignored by the harness.       |
| `description`        | Optional. Informational; ignored by the harness.       |
| `network`            | `mainnet`, `preprod`, `preview`, or `testnet_<magic>`. |
| `eraHistory`         | `{ stabilityWindow, eras: [EraSummary] }`.             |
| `protocolParameters` | See `schema.json` for the full field list.             |
| `initialState`       | `{ utxo: [{input, output}], votingState }`.            |
| `ledgerEnv`          | `{ slot, txIx }`.                                      |
| `transaction`        | Hex-encoded CBOR.                                      |
| `expected`           | `"Pass"` or `{ "predicate": "<Name>", ... }`.          |

UTxO entries are pairs of hex-encoded CBOR: `input` is `TransactionInput`,
`output` is the transaction output.

`protocolParameters` is loosely inspired by [Ogmios](https://github.com/CardanoSolutions/ogmios)
but intentionally diverges:

- ratios are `{ "numerator": N, "denominator": M }`, not `"n/m"` strings
- lovelace amounts are bare integers, not `{ "ada": { "lovelace": N } }`
- byte sizes are bare integers, not `{ "bytes": N }`
- Plutus cost-model keys are camelCase (`plutusV1`, `plutusV2`, `plutusV3`)
- `minFeeReferenceScripts.base` and `multiplier` are ratio objects

`maxRefScriptSizePerBlock` is currently hardcoded in the Haskell ledger and is
not part of the schema.

## Test harness

A harness should:

1. Parse the fixture as JSON.
2. Build an initial ledger state from `network`, `eraHistory`,
   `protocolParameters`, and `initialState`. Hex-decode then CBOR-decode each
   UTxO entry's `input` and `output`.
3. Hex-decode and CBOR-decode the `transaction`.
4. Run phase-one validation against that state and `ledgerEnv`.
5. Compare the result to `expected`:
   - If `expected` is `"Pass"`, validation must succeed.
   - If `expected` is an object, validation must fail with an error that
     corresponds to `expected.predicate` (and to any other fields present).
     Implementations are responsible for mapping their internal error types
     to the canonical predicate names.

The Amaru implementation lives in
`crates/amaru-ledger/src/rules/transaction/phase_one/mod.rs`.

## Adding a fixture

1. Place the JSON in `pass/<name>.json` for a passing fixture, or
   `fail/<Predicate>/<n>.json` for a failing one. A failing fixture must
   exercise exactly one predicate failure; the transaction should be valid
   in every other respect.
2. Register it in the `#[test_case(...)]` list in `mod.rs`.
3. If the predicate is new, add a variant to `Predicate` and a match arm in
   `From<PhaseOneError> for Predicate` in `fixture.rs`.
4. Open a PR, proposing the new fixture(s).
