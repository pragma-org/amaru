use amaru_kernel::{
    cbor, protocol_parameters::ProtocolParameters, TransactionBody, TransactionOutput,
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
    transaction: &TransactionBody,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), OutputTooSmall> {
    let coins_per_utxo_byte = protocol_parameters.coins_per_utxo_byte;
    let outputs_too_small = transaction
        .clone()
        .outputs
        .into_iter()
        .filter(|output| {
            let mut bytes = Vec::new();
            cbor::encode(output, &mut bytes)
                .unwrap_or_else(|_| panic!("Failed to serialize output"));

            let lovelace = match output {
                amaru_kernel::PseudoTransactionOutput::Legacy(legacy) => match legacy.amount {
                    amaru_kernel::alonzo::Value::Coin(lovelace) => lovelace,
                    amaru_kernel::alonzo::Value::Multiasset(lovelace, _) => lovelace,
                },
                amaru_kernel::PseudoTransactionOutput::PostAlonzo(post_alonzo) => {
                    match post_alonzo.value {
                        amaru_kernel::Value::Coin(lovelace) => lovelace,
                        amaru_kernel::Value::Multiasset(lovelace, _) => lovelace,
                    }
                }
            };

            lovelace <= bytes.len() as u64 * coins_per_utxo_byte
        })
        .collect::<Vec<_>>();

    if outputs_too_small.is_empty() {
        Ok(())
    } else {
        Err(OutputTooSmall { outputs_too_small })
    }
}
