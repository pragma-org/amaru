use amaru_kernel::{
    cbor, output_lovelace, protocol_parameters::ProtocolParameters, TransactionBody,
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

// clean up clones, replace with lifetimes
#[allow(clippy::panic)]
pub fn validate_output_size(
    transaction: &TransactionBody,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), OutputTooSmall> {
    let coins_per_utxo_byte = protocol_parameters.coins_per_utxo_byte;
    let outputs_too_small = transaction
        .outputs
        .clone()
        .into_iter()
        .filter(|output| {
            let mut bytes = Vec::new();
            cbor::encode(output, &mut bytes)
                .unwrap_or_else(|_| panic!("Failed to serialize output"));

            let lovelace = output_lovelace(output);

            lovelace <= bytes.len() as u64 * coins_per_utxo_byte
        })
        .collect::<Vec<_>>();

    if outputs_too_small.is_empty() {
        Ok(())
    } else {
        Err(OutputTooSmall { outputs_too_small })
    }
}
