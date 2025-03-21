use std::{array::TryFromSliceError, ops::Deref};

use amaru_kernel::{
    alonzo::BootstrapWitness, AddrType, Address, HasAddress, Hasher, KeepRaw,
    MintedTransactionBody, NonEmptySet, OriginalHash, PublicKey, Signature,
};
use sha3::{Digest, Sha3_256};

use crate::rules::{context::UtxoSlice, TransactionRuleViolation};

pub fn validate_bootstrap_witnesses(
    transaction_body: &KeepRaw<'_, MintedTransactionBody<'_>>,
    bootstrap_witnesses: &Option<NonEmptySet<BootstrapWitness>>,
    utxo_slice: &UtxoSlice,
) -> Result<(), TransactionRuleViolation> {
    let empty_vec = vec![];
    let collateral = transaction_body.collateral.as_deref().unwrap_or(&empty_vec);

    let required_bootstrap_roots = {
        let inputs_with_collateral =
            [transaction_body.inputs.as_slice(), collateral.as_slice()].concat();

        let mut roots = Vec::with_capacity(inputs_with_collateral.len());
        for input in inputs_with_collateral.iter() {
            // We are assuming the utxo_slice has already been checked for valid inputs
            let output = utxo_slice.get(input);
            if let Some(output) = output {
                let address = output.address().map_err(|e| {
                    TransactionRuleViolation::UncategorizedError(format!(
                        "Invalid output address. (error {:?}) output: {:?}",
                        e, output,
                    ))
                })?;

                if let Address::Byron(byron_address) = address {
                    let payload = byron_address.decode().map_err(|e| {
                        TransactionRuleViolation::UncategorizedError(format!(
                            "Invalid byron address payload. (error {:?}) address: {:?}",
                            e, byron_address
                        ))
                    })?;
                    if let AddrType::PubKey = payload.addrtype {
                        roots.push(payload.root);
                    };
                };
            };
        }

        roots
    };

    let empty_vec = vec![];
    let bootstrap_witnesses = bootstrap_witnesses.as_deref().unwrap_or(&empty_vec);
    let bootstrap_roots = bootstrap_witnesses
        .iter()
        .map(|bootstrap_witness| {
            // CBOR header for data that will be encoded
            let prefix: &[u8] = &[131, 0, 130, 0, 88, 64];

            let mut sha_hasher = Sha3_256::new();
            sha_hasher.update(prefix);
            sha_hasher.update(bootstrap_witness.public_key.deref());
            sha_hasher.update(bootstrap_witness.chain_code.deref());
            sha_hasher.update(bootstrap_witness.attributes.deref());

            let sha_digest = sha_hasher.finalize();
            Hasher::<224>::hash(&sha_digest)
        })
        .collect::<Vec<_>>();

    let missing_bootstrap_roots: Vec<_> = required_bootstrap_roots
        .iter()
        .filter(|root| !bootstrap_roots.contains(root))
        .cloned()
        .collect();

    if !missing_bootstrap_roots.is_empty() {
        return Err(
            TransactionRuleViolation::MissingRequiredBootstrapWitnesses {
                missing_bootstrap_roots,
            },
        );
    }

    let invalid_witnesses = bootstrap_witnesses
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
        return Err(TransactionRuleViolation::InvalidBootstrapWitnesses { invalid_witnesses });
    }

    Ok(())
}

fn validate_witness(witness: &BootstrapWitness, message: &[u8]) -> Result<bool, TryFromSliceError> {
    let vkey_bytes: [u8; 32] = witness.public_key.deref().as_slice().try_into()?;
    let signature_bytes: [u8; 64] = witness.signature.deref().as_slice().try_into()?;

    let public_key: PublicKey = vkey_bytes.into();
    let signature: Signature = signature_bytes.into();

    Ok(public_key.verify(message, &signature))
}
