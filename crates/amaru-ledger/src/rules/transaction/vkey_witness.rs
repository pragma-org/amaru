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
