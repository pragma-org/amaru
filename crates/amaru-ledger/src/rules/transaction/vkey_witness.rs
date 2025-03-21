use crate::rules::{context::UtxoSlice, TransactionRuleViolation};
use amaru_kernel::{
    HasAddress, HasKeyHash, Hash, Hasher, KeepRaw, MintedTransactionBody, NonEmptySet,
    OriginalHash, PublicKey, RequiresVkeyWitness, Signature, VKeyWitness,
};
use std::{array::TryFromSliceError, collections::BTreeSet, ops::Deref};

pub fn validate_vkey_witnesses(
    context: &impl UtxoSlice,
    transaction_body: &KeepRaw<'_, MintedTransactionBody<'_>>,
    vkey_witnesses: &Option<NonEmptySet<VKeyWitness>>,
) -> Result<(), TransactionRuleViolation> {
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
                    TransactionRuleViolation::UncategorizedError(format!(
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
        withdrawals.iter().for_each(|withdrawal| {
            if let Some(kh) = withdrawal.requires_vkey_witness() {
                required_vkey_hashes.insert(kh);
            };
        });
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
        return Err(TransactionRuleViolation::MissingRequiredVkeyWitnesses { missing_key_hashes });
    }

    let invalid_witnesses = vkey_witnesses
        .iter()
        .enumerate()
        .filter_map(|(index, witness)| {
            match validate_witness(witness, transaction_body.original_hash().as_slice()) {
                Ok(is_valid) => {
                    if !is_valid {
                        Some(index)
                    } else {
                        None
                    }
                }
                Err(_) => Some(index),
            }
        })
        .collect::<Vec<_>>();

    if !invalid_witnesses.is_empty() {
        return Err(TransactionRuleViolation::InvalidVkeyWitnesses { invalid_witnesses });
    }

    Ok(())
}

fn validate_witness(witness: &VKeyWitness, message: &[u8]) -> Result<bool, TryFromSliceError> {
    let vkey_bytes: [u8; 32] = witness.vkey.deref().as_slice().try_into()?;
    let signature_bytes: [u8; 64] = witness.signature.deref().as_slice().try_into()?;

    let public_key: PublicKey = vkey_bytes.into();
    let signature: Signature = signature_bytes.into();

    Ok(public_key.verify(message, &signature))
}
