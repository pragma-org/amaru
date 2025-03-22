use crate::rules::{context::UtxoSlice, TransactionRuleViolation};
use amaru_kernel::{
    alonzo::BootstrapWitness, to_root, AddrType, Address, HasAddress, KeepRaw,
    MintedTransactionBody, NonEmptySet, OriginalHash, PublicKey, Signature,
};
use std::{array::TryFromSliceError, collections::BTreeSet, ops::Deref};

pub fn execute(
    context: &impl UtxoSlice,
    transaction_body: &KeepRaw<'_, MintedTransactionBody<'_>>,
    bootstrap_witnesses: &Option<NonEmptySet<BootstrapWitness>>,
) -> Result<(), TransactionRuleViolation> {
    let mut required_bootstrap_roots = BTreeSet::new();
    let empty_vec = vec![];
    let collateral = transaction_body.collateral.as_deref().unwrap_or(&empty_vec);

    let inputs_with_collateral =
        [transaction_body.inputs.as_slice(), collateral.as_slice()].concat();

    for input in inputs_with_collateral.iter() {
        match context.lookup(input) {
            Some(output) => {
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
                        required_bootstrap_roots.insert(payload.root);
                    };
                };
            }
            None => unimplemented!("failed to lookup input: {input:?}"),
        }
    }

    let empty_vec = vec![];
    let bootstrap_witnesses = bootstrap_witnesses.as_deref().unwrap_or(&empty_vec);
    let mut provided_bootstrap_roots = BTreeSet::new();
    bootstrap_witnesses.iter().for_each(|witness| {
        provided_bootstrap_roots.insert(to_root(witness));
    });

    let missing_bootstrap_roots = required_bootstrap_roots
        .difference(&provided_bootstrap_roots)
        .copied()
        .collect::<Vec<_>>();

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
