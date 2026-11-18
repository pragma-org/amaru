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

use std::collections::BTreeSet;

use amaru_kernel::{
    BootstrapWitness, ByronAddress, Hash, Hasher, InvalidEd25519Signature, TransactionId, VerificationKeyWitnesses,
    size::KEY, utils::string::display_collection, verify_ed25519_signature,
};
use thiserror::Error;

use crate::{context::WitnessSlice, rules::WithPosition};

#[derive(Debug, Error)]
pub enum InvalidVerificationKeyWitness {
    #[error("missing required signatures for keys or roots: [{}]", display_collection(missing_keys_or_roots))]
    MissingRequiredKeysOrRoots { missing_keys_or_roots: Vec<Hash<KEY>> },

    #[error("invalid verification key witnesses: [{}]", display_collection(invalid_witnesses))]
    InvalidSignatures { invalid_witnesses: Vec<WithPosition<InvalidEd25519Signature>> },
}

pub fn execute(
    context: &mut impl WitnessSlice,
    transaction_id: TransactionId,
    bootstrap_witnesses: Option<&[BootstrapWitness]>,
    verification_key_witnesses: Option<&VerificationKeyWitnesses>,
) -> Result<(), InvalidVerificationKeyWitness> {
    let empty_vec = vec![];
    let bootstrap_witnesses = bootstrap_witnesses.unwrap_or(&empty_vec);

    let mut provided_keys_or_roots = BTreeSet::new();
    verification_key_witnesses.into_iter().flat_map(|entries| entries.iter()).for_each(|witness| {
        provided_keys_or_roots.insert(Hasher::<224>::hash(&witness.verification_key));
    });
    bootstrap_witnesses.iter().for_each(|witness| {
        provided_keys_or_roots.insert(ByronAddress::root(witness));
    });

    let mut required_keys_or_roots = context.required_signers();
    required_keys_or_roots.append(&mut context.required_bootstrap_roots());

    let missing_keys_or_roots = required_keys_or_roots.difference(&provided_keys_or_roots).copied().collect::<Vec<_>>();

    if !missing_keys_or_roots.is_empty() {
        // TODO: (Maybe?) return distinct errors for missing keys and for missing roots.
        return Err(InvalidVerificationKeyWitness::MissingRequiredKeysOrRoots { missing_keys_or_roots });
    }

    let mut invalid_witnesses = vec![];
    verification_key_witnesses.into_iter().flat_map(VerificationKeyWitnesses::iter_with_positions).for_each(
        |(position, witness)| {
            verify_ed25519_signature(&witness.verification_key, &witness.signature, transaction_id.as_slice())
                .unwrap_or_else(|element| invalid_witnesses.push(WithPosition { position, element }))
        },
    );

    if !invalid_witnesses.is_empty() {
        return Err(InvalidVerificationKeyWitness::InvalidSignatures { invalid_witnesses });
    }

    let mut invalid_witnesses = vec![];
    bootstrap_witnesses.iter().enumerate().for_each(|(position, witness)| {
        verify_ed25519_signature(&witness.public_key, &witness.signature, transaction_id.as_slice())
            .unwrap_or_else(|element| invalid_witnesses.push(WithPosition { position, element }))
    });

    if !invalid_witnesses.is_empty() {
        return Err(InvalidVerificationKeyWitness::InvalidSignatures { invalid_witnesses });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        VerificationKeyWitness,
        ed25519::{Signer, SigningKey},
    };

    use super::*;
    use crate::context::DefaultValidationContext;

    #[test]
    fn invalid_signature_reports_first_submitted_position() {
        let tx_id = TransactionId::new(Hash::new([3; 32]));
        let key = SigningKey::from_bytes(&[7; 32]);
        let valid = VerificationKeyWitness {
            verification_key: key.verifying_key().to_bytes().into(),
            signature: key.sign(tx_id.as_slice()).to_bytes().into(),
        };
        let invalid = VerificationKeyWitness { signature: [0; 64].into(), ..valid.clone() };
        let witnesses = vec![valid.clone(), valid, invalid.clone(), invalid].try_into().unwrap();
        let error = execute(&mut DefaultValidationContext::default(), tx_id, None, Some(&witnesses)).unwrap_err();
        let InvalidVerificationKeyWitness::InvalidSignatures { invalid_witnesses } = error else {
            panic!("expected a signature error");
        };
        assert_eq!(invalid_witnesses.len(), 1);
        assert_eq!(invalid_witnesses[0].position, 2);
    }
}
