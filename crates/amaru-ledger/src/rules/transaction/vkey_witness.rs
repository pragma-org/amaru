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

    use amaru_kernel::{
        cbor, from_cbor, Hash, KeepRaw, MintedTransactionBody, OriginalHash, WitnessSet,
    };

    use crate::{
        context::assert::AssertValidationContext,
        tests::{include_transaction_body, include_witness_set},
    };

    fn load_validation_context_from_file(
        path: &str,
    ) -> Result<AssertValidationContext, Box<dyn std::error::Error>> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let context = serde_json::from_reader(reader)?;
        Ok(context)
    }

    #[test]
    fn valid_vkey_witnesses() {
        let mut ctx: AssertValidationContext = load_validation_context_from_file("tests/data/transactions/preprod/90412100dcf9229b187c9064f0f05375268e96ccb25524d762e67e3cb0c0259c/context.json").expect("Failed to load context");

        let transaction_body = include_transaction_body!(
            "../../../tests",
            "90412100dcf9229b187c9064f0f05375268e96ccb25524d762e67e3cb0c0259c"
        );
        let witness_set = include_witness_set!(
            "../../../tests",
            "90412100dcf9229b187c9064f0f05375268e96ccb25524d762e67e3cb0c0259c"
        );

        assert!(matches!(
            super::execute(
                &mut ctx,
                transaction_body.original_hash(),
                witness_set.vkeywitness.as_deref(),
            ),
            Ok(())
        ));
    }
    #[test]
    fn invalid_signature() {
        // The following test relies on a real tranasction found on Preprod (44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca)
        // The witness set is modified to include an invalid signature
        let mut ctx: AssertValidationContext = load_validation_context_from_file("tests/data/transactions/preprod/44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca/invalid-signature/context.json").expect("failed to laod validation context");
        let transaction_body = include_transaction_body!(
            "../../../tests",
            "44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca"
        );
        let witness_set = include_witness_set!(
            "../../../tests",
            "44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca",
            "invalid-signature"
        );

        match super::execute(
            &mut ctx,
            transaction_body.original_hash(),
            witness_set.vkeywitness.as_deref(),
        ) {
            Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses }) => {
                assert!(invalid_witnesses.len() == 1);
                assert!(
                    invalid_witnesses[0].position == 0
                        && matches!(
                            invalid_witnesses[0].element,
                            InvalidEd25519Signature::InvalidSignature
                        )
                );
            }
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(e) => panic!("Unexpected error variant: {:?}", e),
        }
    }

    #[test]
    fn invalid_signature_length() {
        // The following test relies on a real tranasction found on Preprod (44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca)
        // The witness set is modified to include an invalid signature (due to length)
        let mut ctx: AssertValidationContext = load_validation_context_from_file("tests/data/transactions/preprod/44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca/invalid-signature-length/context.json").expect("failed to laod validation context");
        let transaction_body = include_transaction_body!(
            "../../../tests",
            "44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca"
        );
        let witness_set = include_witness_set!(
            "../../../tests",
            "44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca",
            "invalid-signature-length"
        );

        match super::execute(
            &mut ctx,
            transaction_body.original_hash(),
            witness_set.vkeywitness.as_deref(),
        ) {
            Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses }) => {
                assert!(invalid_witnesses.len() == 1);
                assert!(
                    invalid_witnesses[0].position == 0
                        && matches!(
                            invalid_witnesses[0].element,
                            InvalidEd25519Signature::InvalidSignatureSize {
                                error: _,
                                expected: 64
                            }
                        )
                );
            }
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(e) => panic!("Unexpected error variant: {:?}", e),
        }
    }

    #[test]
    fn invalid_key_length() {
        // The following test relies on a real tranasction found on Preprod (44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca)
        // The witness set is modified to include an invalid signature (key length)
        let mut ctx: AssertValidationContext = load_validation_context_from_file("tests/data/transactions/preprod/44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca/invalid-key-length/context.json").expect("failed to laod validation context");
        let transaction_body = include_transaction_body!(
            "../../../tests",
            "44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca"
        );
        let witness_set = include_witness_set!(
            "../../../tests",
            "44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca",
            "invalid-key-length"
        );

        match super::execute(
            &mut ctx,
            transaction_body.original_hash(),
            witness_set.vkeywitness.as_deref(),
        ) {
            Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses }) => {
                assert!(invalid_witnesses.len() == 1);
                assert!(
                    invalid_witnesses[0].position == 1
                        && matches!(
                            invalid_witnesses[0].element,
                            InvalidEd25519Signature::InvalidKeySize {
                                error: _,
                                expected: 32
                            }
                        )
                );
            }
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(e) => panic!("Unexpected error variant: {:?}", e),
        }
    }

    #[test]
    fn missing_spending_vkey() {
        // The following test relies on a real tranasction found on Preprod (44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca)
        // The context is modified to require a different signer than what is in the witness (and what is actually required on Preprod)
        let mut ctx: AssertValidationContext = load_validation_context_from_file("tests/data/transactions/preprod/44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca/missing-spending-vkey/context.json").expect("failed to laod validation context");
        let transaction_body = include_transaction_body!(
            "../../../tests",
            "44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca"
        );
        let witness_set = include_witness_set!(
            "../../../tests",
            "44762542f8e2f66da2fa0d4fdf2eb82cc1d24ae689c1d19ffd7e57d038f50bca",
            "missing-spending-vkey"
        );

        match super::execute(
            &mut ctx,
            transaction_body.original_hash(),
            witness_set.vkeywitness.as_deref(),
        ) {
            Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes }) => {
                assert_eq!(
                    missing_key_hashes,
                    vec![Hash::from(
                        hex::decode("00000000000000000000000000000000000000000000000000000000")
                            .expect("failed to decode hex key")
                            .as_slice(),
                    )]
                )
            }
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(e) => panic!("Unexpected error variant: {:?}", e),
        }
    }

    #[test]
    fn missing_required_signer_vkey() {
        // The following test relies on a handrolled transaction based off of a Preprod transaction (806aef9b20b9fcf2b3ee49b4aa20ebdfae6e0a32a2d8ce877aba8769e96c26bb).
        // It's been modified to only include the bare minimum to meet this test requirements
        let mut ctx: AssertValidationContext = load_validation_context_from_file("tests/data/transactions/preprod/806aef9b20b9fcf2b3ee49b4aa20ebdfae6e0a32a2d8ce877aba8769e96c26bb/context.json").expect("failed to laod validation context");
        let transaction_body = include_transaction_body!(
            "../../../tests",
            "806aef9b20b9fcf2b3ee49b4aa20ebdfae6e0a32a2d8ce877aba8769e96c26bb"
        );
        let witness_set = include_witness_set!(
            "../../../tests",
            "806aef9b20b9fcf2b3ee49b4aa20ebdfae6e0a32a2d8ce877aba8769e96c26bb"
        );

        match super::execute(
            &mut ctx,
            transaction_body.original_hash(),
            witness_set.vkeywitness.as_deref(),
        ) {
            Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes }) => {
                assert_eq!(
                    missing_key_hashes,
                    vec![Hash::from(
                        hex::decode("00000000000000000000000000000000000000000000000000000000")
                            .expect("failed to decode missing key hash")
                            .as_slice(),
                    )]
                )
            }
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(e) => panic!("Unexpected error variant: {:?}", e),
        }
    }

    // TODO: add tests for voting procedures

    #[test]
    fn missing_withdraw_vkey() {
        // The following test relies on a handrolled transaction based off of a Preprod transaction (bd7aee1f39142e1064dd0f504e2b2d57268c3ea9521aca514592e0d831bd5aca).
        // It's been modified to only include the bare minimum to meet this test requirements
        let mut ctx: AssertValidationContext = load_validation_context_from_file("tests/data/transactions/preprod/bd7aee1f39142e1064dd0f504e2b2d57268c3ea9521aca514592e0d831bd5aca/context.json").expect("failed to laod validation context");
        let transaction_body = include_transaction_body!(
            "../../../tests",
            "bd7aee1f39142e1064dd0f504e2b2d57268c3ea9521aca514592e0d831bd5aca"
        );
        let witness_set = include_witness_set!(
            "../../../tests",
            "bd7aee1f39142e1064dd0f504e2b2d57268c3ea9521aca514592e0d831bd5aca"
        );
        match super::execute(
            &mut ctx,
            transaction_body.original_hash(),
            witness_set.vkeywitness.as_deref(),
        ) {
            Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes }) => {
                assert_eq!(
                    missing_key_hashes,
                    vec![Hash::from(
                        hex::decode("61C083BA69CA5E6946E8DDFE34034CE84817C1B6A806B112706109DA")
                            .expect("failed to decode")
                            .as_slice(),
                    )]
                )
            }
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(e) => panic!("Unexpected error variant: {:?}", e),
        }
    }

    // TODO: include more certificate variants
    #[test]
    fn missing_certificate_vkey() {
        // The following test relies on a handrolled transaction based off of a Preprod transaction (4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2)
        // The witness set has been modified to exclude the witness assosciated with the certificate
        let mut ctx: AssertValidationContext = load_validation_context_from_file("tests/data/transactions/preprod/4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2/context.json").expect("failed to laod validation context");
        let transaction_body = include_transaction_body!(
            "../../../tests",
            "4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2"
        );
        let witness_set = include_witness_set!(
            "../../../tests",
            "4d8e6416f1566dc2ab8557cb291b522f46abbd9411746289b82dfa96872ee4e2"
        );

        match super::execute(
            &mut ctx,
            transaction_body.original_hash(),
            witness_set.vkeywitness.as_deref(),
        ) {
            Err(InvalidVKeyWitness::MissingRequiredVkeyWitnesses { missing_key_hashes }) => {
                assert_eq!(
                    missing_key_hashes,
                    vec![Hash::from(
                        hex::decode("112909208360fb65678272a1d6ff45cf5cccbcbb52bcb0c59bb74862")
                            .expect("failed to decode")
                            .as_slice(),
                    )]
                )
            }
            Ok(_) => panic!("Expected Err, got Ok"),
            Err(e) => panic!("Unexpected error variant: {:?}", e),
        }
    }
}
