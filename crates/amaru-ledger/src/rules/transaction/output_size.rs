use crate::rules::TransactionRuleViolation;
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, to_cbor, HasLovelace, MintedTransactionBody,
    TransactionOutput,
};

pub fn execute(
    transaction: &MintedTransactionBody<'_>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), TransactionRuleViolation> {
    let coins_per_utxo_byte = protocol_parameters.coins_per_utxo_byte;
    let outputs_too_small = transaction
        .outputs
        .iter()
        .filter(|output| {
            let bytes = to_cbor(output);
            output.lovelace() < bytes.len() as u64 * coins_per_utxo_byte
        })
        .collect::<Vec<_>>();

    if outputs_too_small.is_empty() {
        Ok(())
    } else {
        Err(TransactionRuleViolation::OutputTooSmall {
            outputs_too_small: outputs_too_small
                .into_iter()
                .cloned()
                .map(TransactionOutput::from)
                .collect(),
        })
    }
}
