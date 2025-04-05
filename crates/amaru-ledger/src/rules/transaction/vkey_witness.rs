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
use amaru_kernel::{Hash, Hasher, TransactionId, VKeyWitness};
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidVKeyWitness {
    #[error(
        "missing required signatures: pkhs [{}]",
        format_vec(missing_key_hashes)
    )]
    MissingRequiredVkeyWitnesses { missing_key_hashes: Vec<Hash<28>> },

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
    vkey_witnesses: Option<&Vec<VKeyWitness>>,
) -> Result<(), InvalidVKeyWitness> {
    let empty_vec = vec![];
    let vkey_witnesses = vkey_witnesses.unwrap_or(&empty_vec);
    let mut provided_vkey_hashes = BTreeSet::new();
    vkey_witnesses.iter().for_each(|witness| {
        provided_vkey_hashes.insert(Hasher::<224>::hash(&witness.vkey));
    });

    let missing_key_hashes = context
        .required_signers()
        .difference(&provided_vkey_hashes)
        .copied()
        .collect::<Vec<_>>();

    if !missing_key_hashes.is_empty() {
        return Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes });
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{context::assert::AssertValidationContext, rules::tests::fixture_context};
    use amaru_kernel::{hash, include_cbor, include_json, WitnessSet};
    use test_case::test_case;

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
        matches Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes })
            if missing_key_hashes[..] == [hash!("00000000000000000000000000000000000000000000000000000000")];
        "missing required witness"
    )]
    #[test_case(
        fixture!("806aef9b20b9fcf2b3ee49b4aa20ebdfae6e0a32a2d8ce877aba8769e96c26bb") =>
        matches Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes })
            if missing_key_hashes[..] == [hash!("00000000000000000000000000000000000000000000000000000000")];
        "missing required signer"
    )]
    #[test_case(
        fixture!("bd7aee1f39142e1064dd0f504e2b2d57268c3ea9521aca514592e0d831bd5aca") =>
        matches Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes })
            if missing_key_hashes[..] == [hash!("61c083ba69ca5e6946e8ddfe34034ce84817c1b6a806b112706109da")];
        "missing withdraw vkey"
    )]
    #[test_case(
        fixture!("4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2") =>
        matches Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes })
            if missing_key_hashes[..] == [hash!("112909208360fb65678272a1d6ff45cf5cccbcbb52bcb0c59bb74862")];
        "missing certificate vkey"
    )]
    fn test_vkey_witness(
        (mut ctx, transaction_id, witness_set): (
            AssertValidationContext,
            TransactionId,
            WitnessSet,
        ),
    ) -> Result<(), InvalidVKeyWitness> {
        super::execute(&mut ctx, transaction_id, witness_set.vkeywitness.as_deref())
    }
}
