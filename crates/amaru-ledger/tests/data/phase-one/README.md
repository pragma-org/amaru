# Phase-one validation fixtures

JSON test vectors for Cardano phase-one transaction validation. Each fixture
declares an initial ledger state, a transaction, and an expected outcome. The
format is implementation-generic; any Cardano implementation can consume these
to increase confidence their phase-one implementation conforms.

## Layout

```text
pass/<name>.json
fail/<Predicate>/<n>.json
common/
  protocolParameters/<preset>.json
  eraHistory/<preset>.json
schema.json
```

`<Predicate>` matches the Haskell ledger's predicate-failure name (e.g.
`InvalidWitnessesUTXOW`). `<n>` distinguishes multiple cases for the same
predicate.

`common/` holds shared canonical documents that fixtures reference instead of
inlining. See [Shared documents](#shared-documents) below.

## Schema

The authoritative schema is [`schema.json`](./schema.json). A summary follows; consult the schema for exact types, ranges, and
required-field rules.

| Field                | Notes                                                  |
| -------------------- | ------------------------------------------------------ |
| `title`              | Optional. Informational; ignored by the harness.       |
| `description`        | Optional. Informational; ignored by the harness.       |
| `network`            | `mainnet`, `preprod`, `preview`, or `testnet_<magic>`. |
| `eraHistory`         | Inline `{ stabilityWindow, eras: [EraSummary] }` or a `$ref` to a shared file. |
| `protocolParameters` | Inline (see `schema.json`) or a `$ref` to a shared file, optionally with `$override`. |
| `initialState`       | See [Initial state](#initial-state).                   |
| `point`              | `{ slot, transactionIndex }`.                          |
| `transaction`        | Hex-encoded CBOR.                                      |
| `expected`           | `"Pass"` or `{ "predicate": "<Name>", ... }`.          |

UTxO entries are pairs of hex-encoded CBOR: `input` is `TransactionInput`,
`output` is the transaction output.

### Initial state

Initial state represents the state on which the transaction is validated. Currently,
it is made up of five fields:


- `pools`: array of hex-encoded pool key hashes, does not contain pool's parameters.
- `accounts`: `[{ credential, deposit, rewards?, pool?, drep? }]`. `credential`
  is hex-encoded CBOR of a `StakeCredential`; `deposit`/`rewards` are lovelace
  (`rewards` defaults to `0`); `pool`/`drep` are optional delegations of the form
  `{ id, delegatedAt }`, where `id` is a hex pool hash or, for the drep delegation, a hex-encoded CBOR `DRep`
  and `delegatedAt` is a `{ transaction, certificateIndex }` pointer.
- `dreps`: `[{ credential, deposit, registeredAt, validUntil }]`, with the same
  `credential` encoding, a `registeredAt` certificate pointer, and a `validUntil`
  epoch.

`protocolParameters` is loosely inspired by [Ogmios](https://github.com/CardanoSolutions/ogmios)
but intentionally diverges:

- ratios are `{ "numerator": N, "denominator": M }`, not `"n/m"` strings
- lovelace amounts are bare integers, not `{ "ada": { "lovelace": N } }`
- byte sizes are bare integers, not `{ "bytes": N }`
- Plutus cost-model keys are camelCase (`plutusV1`, `plutusV2`, `plutusV3`)
- `minFeeReferenceScripts.base` and `multiplier` are ratio objects

`maxRefScriptSizePerBlock` is currently hardcoded in the Haskell ledger and is
not part of the schema.

## Shared documents

`protocolParameters` and `eraHistory` are identical across many
fixtures. To avoid duplicating ~800 lines per fixture, both fields accept a
reference to a shared canonical document under `common/` instead of the inline
form.

Reference form:

```json
"protocolParameters": { "$ref": "common/protocolParameters/preprod-conway-v9.json" }
```

The path is relative to the fixture data root. The harness reads the file and
deserializes it as if it had been inlined.

For one-off variations on a shared preset, an optional `$override` object is
shallow-merged over the referenced document before deserialization:

```json
"protocolParameters": {
  "$ref": "common/protocolParameters/preprod-conway-v9.json",
  "$override": { "maxTransactionSize": 100 }
}
```

Top-level keys in `$override` replace the corresponding keys in the referenced
document. The merge is shallow; nested objects are replaced
entirely instead of merged.

The inline form is also accepted and equivalent.

## Test harness

A harness should:

1. Parse the fixture as JSON (and resolve/shallow-merge any `$ref` or `$override` values)
2. Build an initial ledger state from `network`, `eraHistory`,
   `protocolParameters`, and `initialState`. Hex-decode then CBOR-decode each
   UTxO entry's `input` and `output`.
3. Hex-decode and CBOR-decode the `transaction`.
4. Run phase-one validation against that state and `point`.
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
