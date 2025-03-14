use amaru_kernel::{
    protocol_parameters::ProtocolParameters, to_cbor, HasLovelace, MintedTransactionBody,
    TransactionOutput,
};

use crate::rules::TransactionRuleViolation;

pub struct OutputTooSmall {
    pub outputs_too_small: Vec<TransactionOutput>,
}

impl Into<TransactionRuleViolation> for OutputTooSmall {
    fn into(self) -> TransactionRuleViolation {
        TransactionRuleViolation::OutputTooSmall(self)
    }
}

pub fn validate_output_size(
    transaction: &MintedTransactionBody<'_>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), OutputTooSmall> {
    let coins_per_utxo_byte = protocol_parameters.coins_per_utxo_byte;
    let outputs_too_small = transaction
        .outputs
        .iter()
        .filter(|output| {
            let bytes = to_cbor(output);
            output.lovelace() <= bytes.len() as u64 * coins_per_utxo_byte
        })
        .collect::<Vec<_>>();

    if outputs_too_small.is_empty() {
        Ok(())
    } else {
        Err(OutputTooSmall {
            outputs_too_small: outputs_too_small
                .into_iter()
                .cloned()
                .map(TransactionOutput::from)
                .collect(),
        })
    }
}
