// Copyright 2025 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{
    context::WitnessSlice,
    rules::{
        format_vec, verify_ed25519_signature, InvalidEd25519Signature, TransactionField,
        WithPosition,
    },
};
use amaru_kernel::{to_root, BootstrapWitness, Hash, Hasher, TransactionId, VKeyWitness};
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidVKeyWitness {
    #[error(
        "missing required signatures for keys or roots: [{}]",
        format_vec(missing_keys_or_roots)
    )]
    MissingRequiredKeysOrRoots {
        missing_keys_or_roots: Vec<Hash<28>>,
    },

    #[error(
        "invalid verification key witnesses: [{}]",
        format_vec(invalid_witnesses)
    )]
    InvalidSignatures {
        invalid_witnesses: Vec<WithPosition<InvalidEd25519Signature>>,
    },

    #[error("unexpected bytes instead of reward account in {context:?} at position {position}")]
    MalformedRewardAccount {
        bytes: Vec<u8>,
        context: TransactionField,
        position: usize,
    },
}

pub fn execute(
    context: &mut impl WitnessSlice,
    transaction_id: TransactionId,
    bootstrap_witnesses: Option<&Vec<BootstrapWitness>>,
    vkey_witnesses: Option<&Vec<VKeyWitness>>,
) -> Result<(), InvalidVKeyWitness> {
    let empty_vec = vec![];
    let vkey_witnesses = vkey_witnesses.unwrap_or(&empty_vec);

    let empty_vec = vec![];
    let bootstrap_witnesses = bootstrap_witnesses.unwrap_or(&empty_vec);

    let mut provided_keys_or_roots = BTreeSet::new();
    vkey_witnesses.iter().for_each(|witness| {
        provided_keys_or_roots.insert(Hasher::<224>::hash(&witness.vkey));
    });
    bootstrap_witnesses.iter().for_each(|witness| {
        provided_keys_or_roots.insert(to_root(witness));
    });

    let mut required_keys_or_roots = context.required_signers();
    required_keys_or_roots.append(&mut context.required_bootstrap_roots());

    let missing_keys_or_roots = required_keys_or_roots
        .difference(&provided_keys_or_roots)
        .copied()
        .collect::<Vec<_>>();

    if !missing_keys_or_roots.is_empty() {
        // TODO: (Maybe?) return distinct errors for missing keys and for missing roots.
        return Err(InvalidVKeyWitness::MissingRequiredKeysOrRoots {
            missing_keys_or_roots,
        });
    }

    let mut invalid_witnesses = vec![];
    vkey_witnesses
        .iter()
        .enumerate()
        .for_each(|(position, witness)| {
            verify_ed25519_signature(&witness.vkey, &witness.signature, transaction_id.as_slice())
                .unwrap_or_else(|element| {
                    invalid_witnesses.push(WithPosition { position, element })
                })
        });

    if !invalid_witnesses.is_empty() {
        return Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses });
    }

    let mut invalid_witnesses = vec![];
    bootstrap_witnesses
        .iter()
        .enumerate()
        .for_each(|(position, witness)| {
            verify_ed25519_signature(
                &witness.public_key,
                &witness.signature,
                transaction_id.as_slice(),
            )
            .unwrap_or_else(|element| invalid_witnesses.push(WithPosition { position, element }))
        });

    if !invalid_witnesses.is_empty() {
        return Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::assert::AssertValidationContext,
        rules::{tests::fixture_context, InvalidEd25519Signature, WithPosition},
    };
    use amaru_kernel::{
        hash, include_cbor, include_json, json, KeepRaw, MintedTransactionBody, MintedWitnessSet,
        OriginalHash, WitnessSet,
    };
    use test_case::test_case;
    use tracing_json::assert_trace;

    macro_rules! fixture {
        ($hash:literal) => {
            (
                fixture_context!($hash),
                hash!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/witness.cbor")),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                fixture_context!($hash, $variant),
                hash!($hash),
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/witness.cbor"
                )),
            )
        };
    }

    // TODO: add tests for voting procedures
    // TODO: include more certificate variants
    #[test_case(
        fixture!("90412100dcf9229b187c9064f0f05375268e96ccb25524d762e67e3cb0c0259c");
        "valid"
    )]
    #[test_case(
        fixture!("44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca", "invalid-signature") =>
        matches Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses })
            if invalid_witnesses.len() == 1 && invalid_witnesses[0].position == 0 && matches!(
                invalid_witnesses[0].element,
                InvalidEd25519Signature::InvalidSignature
            );
        "invalid signature"
    )]
    #[test_case(
        fixture!("44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca", "invalid-signature-length") =>
        matches Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses })
            if invalid_witnesses.len() == 1 && invalid_witnesses[0].position == 0 && matches!(
                invalid_witnesses[0].element,
                InvalidEd25519Signature::InvalidSignatureSize { expected: 64, .. }
            );
        "invalid signature size"
    )]
    #[test_case(
        fixture!("44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca", "invalid-key-length") =>
        matches Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses })
            if invalid_witnesses.len() == 1 && invalid_witnesses[0].position == 1 && matches!(
                invalid_witnesses[0].element,
                InvalidEd25519Signature::InvalidKeySize { expected: 32, .. }
            );
        "invalid key size"
    )]
    #[test_case(
        fixture!("44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca", "missing-spending-vkey") =>
        matches Err(InvalidVKeyWitness::MissingRequiredKeysOrRoots { missing_keys_or_roots })
            if missing_keys_or_roots[..] == [hash!("00000000000000000000000000000000000000000000000000000000")];
        "missing required witness"
    )]
    #[test_case(
        fixture!("806aef9b20b9fcf2b3ee49b4aa20ebdfae6e0a32a2d8ce877aba8769e96c26bb") =>
        matches Err(InvalidVKeyWitness::MissingRequiredKeysOrRoots { missing_keys_or_roots })
            if missing_keys_or_roots[..] == [hash!("00000000000000000000000000000000000000000000000000000000")];
        "missing required signer"
    )]
    #[test_case(
        fixture!("bd7aee1f39142e1064dd0f504e2b2d57268c3ea9521aca514592e0d831bd5aca") =>
        matches Err(InvalidVKeyWitness::MissingRequiredKeysOrRoots { missing_keys_or_roots })
            if missing_keys_or_roots[..] == [hash!("61c083ba69ca5e6946e8ddfe34034ce84817c1b6a806b112706109da")];
        "missing withdraw vkey"
    )]
    #[test_case(
        fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2") =>
        matches Err(InvalidVKeyWitness::MissingRequiredKeysOrRoots { missing_keys_or_roots })
            if missing_keys_or_roots[..] == [hash!("112909208360fb65678272a1d6ff45cf5cccbcbb52bcb0c59bb74862")];
        "missing certificate vkey"
    )]
    fn test_vkey_witness(
        (mut ctx, transaction_id, witness_set): (
            AssertValidationContext,
            TransactionId,
            WitnessSet,
        ),
    ) -> Result<(), InvalidVKeyWitness> {
        super::execute(
            &mut ctx,
            transaction_id,
            witness_set.bootstrap_witness.as_deref(),
            witness_set.vkeywitness.as_deref(),
        )
    }

    macro_rules! fixture_bootstrap {
        ($hash:literal) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_cbor!(concat!("transactions/preprod/", $hash, "/witness.cbor")),
                include_json!(concat!("transactions/preprod/", $hash, "/expected.traces")),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                fixture_context!($hash, $variant),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/witness.cbor"
                )),
                include_json!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/expected.traces"
                )),
            )
        };
    }

    #[test_case(fixture_bootstrap!("49e6100c24938acb075f3415ddd989c7e91a5c52b8eb848364c660577e11594a"); "happy path byron")]
    #[test_case(fixture_bootstrap!("0c22edee0ffd7c8f32d2fe4da1f144e9ef78dfb51e1678d5198493a83d6cf8ec"); "happy path icarus")]
    #[test_case(fixture_bootstrap!("49e6100c24938acb075f3415ddd989c7e91a5c52b8eb848364c660577e11594a", "missing-required-witness") =>
        matches Err(InvalidVKeyWitness::MissingRequiredKeysOrRoots { missing_keys_or_roots })
        if missing_keys_or_roots.len() == 1 && hex::encode(missing_keys_or_roots[0]) == "65b1fe57f0ed455254aacf1486c448d7f34038c4c445fa905de33d8f";
        "Missing Required Witness")]
    #[test_case(fixture_bootstrap!("49e6100c24938acb075f3415ddd989c7e91a5c52b8eb848364c660577e11594a", "invalid-signature") =>
        matches Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses })
        if invalid_witnesses.len() == 1 && matches!(invalid_witnesses[0], WithPosition {
            position: 0,
            element: InvalidEd25519Signature::InvalidSignature});
        "Invalid Signatures: Invalid Signature")]
    #[test_case(fixture_bootstrap!("49e6100c24938acb075f3415ddd989c7e91a5c52b8eb848364c660577e11594a", "invalid-signature-size") =>
        matches Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses })
        if invalid_witnesses.len() == 1 && matches!(invalid_witnesses[0], WithPosition {
            position: 0,
            element: InvalidEd25519Signature::InvalidSignatureSize { ..}});
        "Invalid Signatures: Invalid Signature Size")]
    // InvalidKeySize is enforced by the validation context type (Hash<28>).
    // If the key in the signature is the wrong length, the execute function will fail with MissingRequiredWitness
    fn bootstrap_witness(
        (mut ctx, tx, witness_set, expected_traces): (
            AssertValidationContext,
            KeepRaw<'_, MintedTransactionBody<'_>>,
            KeepRaw<'_, MintedWitnessSet<'_>>,
            Vec<json::Value>,
        ),
    ) -> Result<(), InvalidVKeyWitness> {
        assert_trace(
            || {
                let transaction_id = tx.original_hash();
                super::execute(
                    &mut ctx,
                    transaction_id,
                    witness_set.bootstrap_witness.as_deref(),
                    witness_set.vkeywitness.as_deref(),
                )
            },
            expected_traces,
        )
    }
}
