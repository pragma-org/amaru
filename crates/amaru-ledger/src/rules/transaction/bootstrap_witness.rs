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
    rules::{format_vec, verify_ed25519_signature, InvalidEd25519Signature, WithPosition},
};
use amaru_kernel::{to_root, BootstrapWitness, Hash, TransactionId};
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidBootstrapWitnesses {
    #[error(
        "missing required signatures: bootstrap roots [{}]",
        format_vec(missing_bootstrap_roots)
    )]
    MissingRequiredBootstrapWitnesses {
        missing_bootstrap_roots: Vec<Hash<28>>,
    },

    #[error(
        "invalid bootstrap witnesses: indices [{}]",
        format_vec(invalid_witnesses)
    )]
    InvalidSignatures {
        invalid_witnesses: Vec<WithPosition<InvalidEd25519Signature>>,
    },
}

pub fn execute(
    context: &impl WitnessSlice,
    transaction_id: TransactionId,
    bootstrap_witnesses: Option<&Vec<BootstrapWitness>>,
) -> Result<(), InvalidBootstrapWitnesses> {
    let empty_vec = vec![];
    let bootstrap_witnesses = bootstrap_witnesses.unwrap_or(&empty_vec);

    let mut provided_bootstrap_roots = BTreeSet::new();
    bootstrap_witnesses.iter().for_each(|witness| {
        provided_bootstrap_roots.insert(to_root(witness));
    });

    let missing_bootstrap_roots = context
        .required_bootstrap_signers()
        .difference(&provided_bootstrap_roots)
        .copied()
        .collect::<Vec<_>>();

    if !missing_bootstrap_roots.is_empty() {
        return Err(
            InvalidBootstrapWitnesses::MissingRequiredBootstrapWitnesses {
                missing_bootstrap_roots,
            },
        );
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
        return Err(InvalidBootstrapWitnesses::InvalidSignatures { invalid_witnesses });
    }

    Ok(())
}
