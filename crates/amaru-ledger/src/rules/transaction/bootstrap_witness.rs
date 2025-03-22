use crate::rules::{
    context::UtxoSlice, vkey_witness::verify_ed25519_signature, TransactionRuleViolation,
    WithPosition,
};
use amaru_kernel::{
    alonzo::BootstrapWitness, to_root, AddrType, Address, HasAddress, KeepRaw,
    MintedTransactionBody, NonEmptySet, OriginalHash,
};
use std::collections::BTreeSet;

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

    let mut invalid_witnesses = vec![];
    bootstrap_witnesses
        .iter()
        .enumerate()
        .for_each(|(position, witness)| {
            verify_ed25519_signature(
                &witness.public_key,
                &witness.signature,
                transaction_body.original_hash().as_slice(),
            )
            .unwrap_or_else(|element| invalid_witnesses.push(WithPosition { position, element }))
        });

    if !invalid_witnesses.is_empty() {
        return Err(TransactionRuleViolation::InvalidBootstrapWitnesses { invalid_witnesses });
    }

    Ok(())
}
