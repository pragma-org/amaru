use std::{array::TryFromSliceError, ops::Deref};

use amaru_kernel::{
    Certificate, HasAddress, HasKeyHash, Hash, Hasher, KeepRaw, MintedTransactionBody, NonEmptySet,
    OriginalHash, PublicKey, Signature, VKeyWitness, Voter,
};

use crate::rules::{context::UtxoSlice, TransactionRuleViolation};

pub fn validate_sigantures(
    transaction_body: &KeepRaw<'_, MintedTransactionBody<'_>>,
    vkey_witnesses: &Option<NonEmptySet<VKeyWitness>>,
    utxo_slice: &UtxoSlice,
) -> Result<(), TransactionRuleViolation> {
    let empty_vec = vec![];
    let collateral = transaction_body.collateral.as_deref().unwrap_or(&empty_vec);

    let empty_vec = vec![];
    let required_signers = transaction_body
        .required_signers
        .as_deref()
        .unwrap_or(&empty_vec);
    let spend_pkhs = {
        let inputs_with_collateral =
            [transaction_body.inputs.as_slice(), collateral.as_slice()].concat();

        let mut pkhs = Vec::with_capacity(inputs_with_collateral.len());

        for input in inputs_with_collateral.iter() {
            // We are assuming the utxo_slice has already been checked for valid inputs
            let output = utxo_slice.get(input);
            if let Some(output) = output {
                let address = output.address().map_err(|e| {
                    TransactionRuleViolation::Unnanmed(format!(
                        "Invalid output address. (error {:?}) output: {:?}",
                        e, output,
                    ))
                })?;

                if let Some(key_hash) = address.get_key_hash() {
                    pkhs.push(key_hash);
                };
            };
        }

        pkhs
    };

    let withdrawal_pkhs = transaction_body
        .withdrawals
        .as_ref()
        .map(|withdrawals| {
            withdrawals
                .iter()
                .filter_map::<Hash<28>, _>(|(reward_account, _)| {
                    if reward_account[0] & 0b00010000 == 0 {
                        Some(Hash::from(&reward_account[1..29]))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let vote_pkhs = transaction_body
        .voting_procedures
        .as_ref()
        .map(|voting_procedures| {
            voting_procedures
                .iter()
                .filter_map(|(voter, _)| match voter {
                    Voter::ConstitutionalCommitteeKey(hash) => Some(*hash),
                    Voter::DRepKey(hash) => Some(*hash),
                    Voter::StakePoolKey(hash) => Some(*hash),
                    Voter::ConstitutionalCommitteeScript(_) => None,
                    Voter::DRepScript(_) => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let certificate_pkhs = transaction_body
        .certificates
        .as_deref()
        .map(|certificates| {
            certificates
                .iter()
                .filter_map(|certificate| match certificate {
                    Certificate::StakeRegistration(stake_credential) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::StakeDeregistration(stake_credential) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::StakeDelegation(stake_credential, _) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::PoolRegistration {
                        operator,
                        vrf_keyhash: _,
                        pledge: _,
                        cost: _,
                        margin: _,
                        reward_account: _,
                        pool_owners: _,
                        relays: _,
                        pool_metadata: _,
                    } => Some(*operator),
                    Certificate::PoolRetirement(hash, _) => Some(*hash),
                    Certificate::Reg(stake_credential, coin) => {
                        if coin == &0 {
                            None
                        } else {
                            stake_credential.get_key_hash()
                        }
                    }
                    Certificate::UnReg(stake_credential, _) => stake_credential.get_key_hash(),
                    Certificate::VoteDeleg(stake_credential, _) => stake_credential.get_key_hash(),
                    Certificate::StakeVoteDeleg(stake_credential, _, _) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::StakeRegDeleg(stake_credential, _, _) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::VoteRegDeleg(stake_credential, _, _) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::StakeVoteRegDeleg(stake_credential, _, _, _) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::AuthCommitteeHot(stake_credential, _) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::ResignCommitteeCold(stake_credential, _) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::RegDRepCert(stake_credential, _, _) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::UnRegDRepCert(stake_credential, _) => {
                        stake_credential.get_key_hash()
                    }
                    Certificate::UpdateDRepCert(stake_credential, _) => {
                        stake_credential.get_key_hash()
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let required_vkey_hashes = [
        spend_pkhs.as_slice(),
        required_signers.as_slice(),
        withdrawal_pkhs.as_slice(),
        vote_pkhs.as_slice(),
        certificate_pkhs.as_slice(),
    ]
    .concat();

    let empty_vec = vec![];
    let vkey_witnesses = vkey_witnesses.as_deref().unwrap_or(&empty_vec);
    let vkey_hashes = vkey_witnesses
        .iter()
        .map(|witness| Hasher::<224>::hash(&witness.vkey))
        .collect::<Vec<_>>();

    // Are we worried about efficiency here? this is quadratic time
    let missing_key_hashes: Vec<_> = required_vkey_hashes
        .into_iter()
        .filter(|hash| !vkey_hashes.contains(hash))
        .collect();

    if !missing_key_hashes.is_empty() {
        return Err(TransactionRuleViolation::MissingRequiredWitnesses { missing_key_hashes });
    }

    let invalid_witnesses = vkey_witnesses
        .iter()
        .filter(|witness| {
            match validate_witness(witness, transaction_body.original_hash().as_slice()) {
                Ok(valid) => !valid,
                Err(e) => {
                    eprintln!("Failed to validate witness: {:?}", e);
                    true
                }
            }
        })
        .cloned()
        .collect::<Vec<_>>();

    if !invalid_witnesses.is_empty() {
        return Err(TransactionRuleViolation::InvalidWitnesses { invalid_witnesses });
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
