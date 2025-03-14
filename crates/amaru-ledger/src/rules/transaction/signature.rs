use amaru_kernel::{
    get_payment_key_hash, HasAddress, Hasher, MintedTransactionBody, NonEmptySet, VKeyWitness,
};

use crate::rules::{context::UtxoSlice, TransactionRuleViolation};

// TODO: handle withdrawals and certificates here too?
pub fn validate_sigantures(
    transaction_body: &MintedTransactionBody<'_>,
    vkey_witnesses: &Option<NonEmptySet<VKeyWitness>>,
    utxo_slice: UtxoSlice,
) -> Result<(), TransactionRuleViolation> {
    let empty_vec = vec![];
    let collateral = transaction_body.collateral.as_deref().unwrap_or(&empty_vec);
    let empty_vec = vec![];
    let additional_required_signers = transaction_body
        .required_signers
        .as_deref()
        .unwrap_or(&empty_vec);

    let required_signers = [transaction_body.inputs.as_slice(), collateral.as_slice()]
        .concat()
        .iter()
        .filter_map(|input| {
            utxo_slice.get(input).and_then(|output| {
                let address = output.address();
                get_payment_key_hash(address)
            })
        })
        .collect::<Vec<_>>();

    // TODO: this is not an entire set of required witnesses, also need to check mints, certs, withdrawals, votes(?)
    let required_vkey_hashes = [
        additional_required_signers.as_slice(),
        required_signers.as_slice(),
    ]
    .concat();

    let empty_vec = vec![];
    let vkey_hashes = vkey_witnesses
        .as_deref()
        .unwrap_or(&empty_vec)
        .iter()
        .map(|witness| Hasher::<224>::hash(&witness.vkey))
        .collect::<Vec<_>>();

    // Are we worried about efficiency here? this is quadratic time
    let missing_key_hashes: Vec<_> = required_vkey_hashes
        .into_iter()
        .filter(|hash| vkey_hashes.contains(hash))
        .collect();

    if !missing_key_hashes.is_empty() {
        return Err(TransactionRuleViolation::MissingRequiredWitnesses { missing_key_hashes });
    }

    todo!()
}
