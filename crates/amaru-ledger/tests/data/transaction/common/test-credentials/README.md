# Test credentials

The Ed25519 keys the transaction fixtures sign with. They are committed so that any
implementation can regenerate a fixture's witness set, or author a new fixture that stays
consistent with the rest of the corpus.

> These keys are **public test material**. They exist only to make fixtures reproducible.
> Never use them to hold funds, on any network.

Each file is a single line of lowercase hex:

- `<name>.skey`: the 32-byte Ed25519 private key
- `<name>.vkey`: the 32-byte Ed25519 public key derived from it

## Signing keys

| Name | Seed | Key hash |
| --- | --- | --- |
| `dev-42` | `0x42` × 32 | `93c191b1094746961f6f00fba27f3d8eff6a66490baf806d4e179fd8` |
| `dev-aa` | `0xAA` × 32 | `e4ff642ed686b644c5cb39c230c24b0e8a5d850701971bdde3253cec` |

**`dev-42` is the default: use it for everything.** 

`dev-aa` exists for the one situation `dev-42` cannot express: a fixture that needs two
*distinct* credentials, such as a certificate whose required signer must differ from the
input's owner. If a case ever needs a third, commit another seed
here rather than inventing one locally.

The key hash is `blake2b-224` (28 bytes) of the verification key.

Because a credential is just the hash, the payment credential of an input and the stake
credential of a certificate or withdrawal can be the *same* key, so a single vkey witness
satisfies both. Most fixtures rely on this to keep their witness set to one entry.

## `preprod-replay.vkey`

`pass/script-integrity-hash/0.json` replays a real preprod transaction at its actual slot, so
its witness belongs to a real key that is not ours and never will be. The verification key is
recorded here so that the witness can be identified rather than mistaken for a dev key.

That fixture is the one place a body genuinely cannot be re-signed: editing it voids the
witness with no way to rebuild it. Treat it as read-only and add new coverage elsewhere.

## Signing

The transaction id is `blake2b-256` over the transaction body's **original bytes**, and the
witness signs that id.

Some fixtures deliberately carry malformed witnesses, and are the exception to "a witness must verify."
All three are still `dev-42` material, so they can be rebuilt.
