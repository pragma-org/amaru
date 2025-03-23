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
    context::UtxoSlice,
    rules::{
        format_vec, traits::requires_vkey_witness::RequiresVkeyWitness, verify_ed25519_signature,
        InvalidEd25519Signature, TransactionField, WithPosition,
    },
};
use amaru_kernel::{
    Address, HasAddress, HasKeyHash, Hash, Hasher, KeepRaw, MintedTransactionBody, NonEmptySet,
    OriginalHash, VKeyWitness,
};
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

    // TODO: This error shouldn't exist, it's a placeholder for better error handling in less straight forward cases
    #[error("uncategorized error: {0}")]
    UncategorizedError(String),
}

pub fn execute(
    context: &impl UtxoSlice,
    transaction_body: &KeepRaw<'_, MintedTransactionBody<'_>>,
    vkey_witnesses: &Option<NonEmptySet<VKeyWitness>>,
) -> Result<(), InvalidVKeyWitness> {
    let mut required_vkey_hashes: BTreeSet<Hash<28>> = BTreeSet::new();

    let empty_vec = vec![];
    let collateral = transaction_body.collateral.as_deref().unwrap_or(&empty_vec);
    for input in [transaction_body.inputs.as_slice(), collateral.as_slice()]
        .concat()
        .iter()
    {
        match context.lookup(input) {
            Some(output) => {
                let address = output.address().map_err(|e| {
                    InvalidVKeyWitness::UncategorizedError(format!(
                        "Invalid output address. (error {:?}) output: {:?}",
                        e, output,
                    ))
                })?;

                if let Some(key_hash) = address.key_hash() {
                    required_vkey_hashes.insert(key_hash);
                };
            }
            None => unimplemented!("failed to lookup input: {input:?}"),
        }
    }

    if let Some(required_signers) = &transaction_body.required_signers {
        required_signers.iter().for_each(|signer| {
            required_vkey_hashes.insert(*signer);
        });
    }

    if let Some(withdrawals) = &transaction_body.withdrawals {
        withdrawals
            .iter()
            .enumerate()
            .try_for_each(|(position, (raw_account, _))| {
                match Address::from_bytes(raw_account) {
                    // TODO: This parsing should happen when we first deserialise the block, and
                    // not in the middle of rules validations.
                    Ok(Address::Stake(account)) => {
                        if let Some(kh) = account.requires_vkey_witness() {
                            required_vkey_hashes.insert(kh);
                        };
                        Ok(())
                    }
                    _ => Err(InvalidVKeyWitness::MalformedRewardAccount {
                        bytes: raw_account.to_vec(),
                        context: TransactionField::Withdrawals,
                        position,
                    }),
                }
            })?;
    }

    if let Some(voting_procedures) = &transaction_body.voting_procedures {
        voting_procedures.iter().for_each(|(voter, _)| {
            if let Some(kh) = voter.requires_vkey_witness() {
                required_vkey_hashes.insert(kh);
            }
        });
    }

    if let Some(certificates) = &transaction_body.certificates {
        certificates.iter().for_each(|certificate| {
            if let Some(kh) = certificate.requires_vkey_witness() {
                required_vkey_hashes.insert(kh);
            }
        })
    }

    let empty_vec = vec![];
    let vkey_witnesses = vkey_witnesses.as_deref().unwrap_or(&empty_vec);
    let mut provided_vkey_hashes = BTreeSet::new();
    vkey_witnesses.iter().for_each(|witness| {
        provided_vkey_hashes.insert(Hasher::<224>::hash(&witness.vkey));
    });

    let missing_key_hashes = required_vkey_hashes
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
            verify_ed25519_signature(
                &witness.vkey,
                &witness.signature,
                transaction_body.original_hash().as_slice(),
            )
            .unwrap_or_else(|element| invalid_witnesses.push(WithPosition { position, element }))
        });

    if !invalid_witnesses.is_empty() {
        return Err(InvalidVKeyWitness::InvalidSignatures { invalid_witnesses });
    }

    Ok(())
}
