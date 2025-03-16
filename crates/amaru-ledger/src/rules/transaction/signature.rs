use std::ops::Deref;

use amaru_kernel::{
    get_payment_key_hash, HasAddress, Hash, Hasher, KeepRaw, MintedTransactionBody, NonEmptySet,
    PublicKey, Signature, VKeyWitness, Voter,
};

use crate::rules::{context::UtxoSlice, TransactionRuleViolation};

// TODO: handle withdrawals and certificates here too?
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

    let spend_pkhs = [transaction_body.inputs.as_slice(), collateral.as_slice()]
        .concat()
        .iter()
        .filter_map(|input| {
            // Here we are assuming the inputs have already been validated (they exist in the utxo slice)
            utxo_slice.get(input).and_then(|output| {
                let address = output.address();
                get_payment_key_hash(address)
            })
        })
        .collect::<Vec<_>>();

    let empty_vec = vec![];
    let withdrawal_pkhs = transaction_body
        .withdrawals
        .as_deref()
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
        .unwrap_or(empty_vec);

    let empty_vec = vec![];
    let vote_pkhs = transaction_body
        .voting_procedures
        .as_deref()
        .map(|voting_procedures| {
            voting_procedures
                .iter()
                .filter_map(|(voter, _)| match voter {
                    Voter::ConstitutionalCommitteeKey(hash) => Some(hash),
                    Voter::DRepKey(hash) => Some(hash),
                    Voter::StakePoolKey(hash) => Some(hash),
                    Voter::ConstitutionalCommitteeScript(_) => None,
                    Voter::DRepScript(_) => None,
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or(empty_vec);

    // TODO: this is not an entire set of required witnesses, also need to check certs
    let required_vkey_hashes = [
        spend_pkhs.as_slice(),
        required_signers.as_slice(),
        withdrawal_pkhs.as_slice(),
        vote_pkhs.as_slice(),
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
        .into_iter()
        .filter(|witness| validate_witness(witness, transaction_body.raw_cbor()))
        .cloned()
        .collect::<Vec<_>>();

    if !invalid_witnesses.is_empty() {
        return Err(TransactionRuleViolation::InvalidWitnesses { invalid_witnesses });
    }

    Ok(())
}

fn validate_witness(witness: &VKeyWitness, message: &[u8]) -> bool {
    let vkey_bytes: [u8; 32] = witness
        .vkey
        .deref()
        .as_slice()
        .try_into()
        .unwrap_or_else(|_| panic!("Invalid vkey length (expected 32 bytes)"));

    let signature_bytes: [u8; 64] = witness
        .signature
        .deref()
        .as_slice()
        .try_into()
        .unwrap_or_else(|_| panic!("Invalid vkey length (expected 64 bytes)"));

    let public_key: PublicKey = vkey_bytes.into();
    let signature: Signature = signature_bytes.into();

    public_key.verify(message, &signature)
}
