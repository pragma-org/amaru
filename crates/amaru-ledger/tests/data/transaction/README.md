# Transaction validation fixtures

JSON test vectors for Cardano transaction validation. Each fixture declares an
initial ledger state, a transaction, and an expected outcome. The format is
implementation-generic; any Cardano implementation can consume these to increase
confidence their ledger rules conform.

Both validation phases are in scope: the phase-one rules, and phase-two Plutus
script evaluation. A fixture whose transaction carries a redeemer therefore has
its scripts executed, and the outcome of that execution is part of what the
fixture asserts.

## Layout

```text
common/
  protocolParameters/<preset>.json
  eraHistory/<preset>.json
  test-credentials/<name>.skey, <name>.vkey
scenarios/
  <id>-<pass|fail>-<name>.json
schema.json
```

`pass` refer to scenarios that must validate, `fail` those that must be
rejected.

`<id>` is a 5-digit number, unique across *both* directories, that identifies the
fixture on its own; `00065` is one fixture, whichever directory it is in. It is
a permanent handle: fixtures are never renumbered, and an id is not reused when a
fixture is deleted, so an external reference to a fixture stays meaningful.

`<name>` is a kebab-case description of the **scenario**'s title
(`00065-pass-stake-delegation-to-unregistered-pool`), not of the expected failure. The
predicate a `fail` fixture must trip is stated once, in `expected.predicate`, so
that a fixture's location can never contradict its expectation. To list the
fixtures covering one predicate, grep for it:

```console
$ grep -l '"predicate": "DelegateeStakePoolNotRegistered"' fail/*.json
```

`common/protocolParameters/` and `common/eraHistory/` hold shared canonical documents
that fixtures reference instead of inlining. See [Shared documents](#shared-documents)
below.

`common/test-credentials/` holds the Ed25519 test keys the fixtures sign with, as
hex, so that any implementation can rebuild a witness set or author a new fixture
with the same credentials. See its
[README](./common/test-credentials/README.md); `dev-42` is the default signer.

## Schema

The authoritative schema is [`schema.json`](./schema.json). A summary follows; consult the schema for exact types, ranges, and
required-field rules.

| Field                | Notes                                                  |
| -------------------- | ------------------------------------------------------ |
| `title`              | Optional, but names the generated test case, so it must be unique across the corpus. Falls back to the fixture's path when absent. |
| `description`        | Optional. Informational; ignored by the harness.       |
| `network`            | `mainnet`, `preprod`, `preview`, or `testnet_<magic>`. |
| `eraHistory`         | Inline `{ stabilityWindow, eras: [EraSummary] }` or a `$ref` to a shared file. |
| `protocolParameters` | Inline (see `schema.json`) or a `$ref` to a shared file, optionally with `$override`. |
| `initialState`       | See [Initial state](#initial-state).                   |
| `point`              | `{ slot, transactionIndex }`.                          |
| `transaction`        | Hex-encoded CBOR.                                      |
| `expected`           | `"Pass"`, `{ "predicate": "<Name>", ... }`, or `{ "decodingFailure": true }`. Some predicates carry extra fields; `ValidationTagMismatch` requires `description`, either `"PassedUnexpectedly"` or `"FailedUnexpectedly"`. |

UTxO entries are pairs of hex-encoded CBOR: `input` is `TransactionInput`,
`output` is the transaction output.

### Initial state

Initial state represents the state on which the transaction is validated. Currently,
it is made up of the following fields:

- `utxo`: array of `{ input, output }` pairs of hex-encoded CBOR, as described above.
- `pools`: array of hex-encoded pool key hashes, does not contain pool's parameters.
- `accounts`: `[{ credential, deposit, rewards?, pool?, drep? }]`. `credential`
  is hex-encoded CBOR of a `StakeCredential`; `deposit`/`rewards` are lovelace
  (`rewards` defaults to `0`); `pool`/`drep` are optional delegations of the form
  `{ id, delegatedAt }`, where `id` is a hex pool hash or, for the drep delegation, a hex-encoded CBOR `DRep`
  and `delegatedAt` is a `{ transaction, certificateIndex }` pointer.
- `dreps`: `[{ credential, deposit, registeredAt, validUntil }]`, with the same
  `credential` encoding, a `registeredAt` certificate pointer, and a `validUntil`
  epoch.
- `committee`: `[{ coldCredential, hotCredential?, validUntil? }]`, the constitutional
  committee keyed by cold credential, both credentials hex-encoded CBOR of a
  `StakeCredential`. `hotCredential` is absent for a member that has never authorized
  one or has resigned; `validUntil` is absent for a member holding no term, which is
  still a state a member can authorize a hot credential from. Note that a vote
  identifies its committee member by *hot* credential.
- `proposals`: `[[id, kind]]`, the in-flight governance proposals seeding the
  block-start proposal set. `id` is a governance action id (see below); `kind` is
  the lineage the proposal belongs to, one of `ProtocolParameters`, `HardFork`,
  `ConstitutionalCommittee`, `Constitution` or `Orphan` — the last for the actions
  that chain to nothing, `Information` and `TreasuryWithdrawals`. Only the lineage
  matters here, so the action itself is not spelled out. Use `[]` when none are
  seeded.
- `proposalsRoots`: `{ protocolParameters?, hardFork?, constitutionalCommittee?, constitution? }`,
  the latest enacted governance action id per purpose, each a governance action id
  (see below). Use `{}` when none are enacted; absent purposes default to none.
- `governanceActivity`: `{ consecutiveDormantEpochs }`.
- `pots`: `{ treasury, reserves }`, the protocol pots as of the initial state.
- `guardrailScript`: hex-encoded script hash of the enacted constitution's
  guardrails script. A proposal carrying a policy must name exactly this script;
  absent or `null` requires proposals to carry no policy at all.

A governance action id is `{ transactionId, proposalIndex }`, mirroring
`CertificatePointer`: `transactionId` is the hex-encoded 32-byte hash of the
transaction that submitted the action, and `proposalIndex` is the action's index
within that transaction.

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
"protocolParameters": { "$ref": "common/protocolParameters/preprod-conway-v10.json" }
```

The path is relative to the fixture data root. The harness reads the file and
deserializes it as if it had been inlined.

For one-off variations on a shared preset, an optional `$override` object is
shallow-merged over the referenced document before deserialization:

```json
"protocolParameters": {
  "$ref": "common/protocolParameters/preprod-conway-v10.json",
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
3. Hex-decode and CBOR-decode the `transaction`. If `expected` is
   `{ "decodingFailure": true }`, that decode must itself fail and no validation
   is run.
4. Run phase-one validation against that state and `point`, then, if phase one
   succeeded, phase-two script evaluation. Both must succeed for the transaction
   to be accepted.
5. Compare the result to `expected`:
   - If `expected` is `"Pass"`, validation must succeed.
   - If `expected` carries a `predicate`, validation must fail with an error that
     corresponds to it (and to any other fields present). Implementations are
     responsible for mapping their internal error types to the canonical
     predicate names.

The Amaru implementation lives in
`crates/amaru-ledger/src/rules/transaction/`, with the two phases under
`phase_one/mod.rs` and `phase_two/mod.rs` and the harness in `mod.rs`.

## Adding a fixture

1. Place the JSON in `scenario` named `<id>-pass-<name>.json` or
   `<id>-fail-<name>.json`, taking the next unused id; one above the highest in
   either directory. A failing fixture must exercise exactly one predicate
   failure; the transaction should be valid in every other respect.
2. The `<name>` part of a scenario should correspond to a kebab-case version of
   the scenario's title, to ease the identification of scenarios upon failures.
3. If the predicate is new, add a variant to `Predicate` and a match arm in the
   relevant `From<..> for Predicate` impl in `fixture.rs`.
4. Open a PR, proposing the new fixture(s).
