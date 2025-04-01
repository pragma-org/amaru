// Copyright 2025 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::context::{UtxoSlice, WitnessSlice};
use amaru_kernel::{AddrType, Address, HasAddress, HasOwnership, TransactionInput};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidInputs {
    #[error("Unknown input: {}#{}", .0.index, .0.transaction_id)]
    UnknownInput(TransactionInput),
    #[error(
        "inputs included in both reference inputs and spent inputs: intersection [{}]",
        intersection
            .iter()
            .map(|input|
                format!("{}#{}", input.index, input.transaction_id)
            )
            .collect::<Vec<_>>()
            .join(", ")
    )]
    NonDisjointRefInputs { intersection: Vec<TransactionInput> },

    // TODO: This error shouldn't exist, it's a placeholder for better error handling in less straight forward cases
    #[error("uncategorized error: {0}")]
    UncategorizedError(String),
}

pub fn execute<C>(
    context: &mut C,
    inputs: &Vec<TransactionInput>,
    reference_inputs: Option<&Vec<TransactionInput>>,
    collaterals: Option<&Vec<TransactionInput>>,
) -> Result<(), InvalidInputs>
where
    C: UtxoSlice + WitnessSlice,
{
    // Check for disjoint reference inputs.
    let intersection = match &reference_inputs {
        Some(ref_inputs) => ref_inputs
            .iter()
            .filter(|ref_input| inputs.contains(ref_input))
            .cloned()
            .collect(),
        None => Vec::new(),
    };

    if !intersection.is_empty() {
        return Err(InvalidInputs::NonDisjointRefInputs { intersection });
    }

    // Collect witnesses
    //
    // FIXME: Collaterals should probably only be acknowledged when the transaction is failing; and
    // vice-versa for inputs.
    let collaterals = collaterals.map(|x| x.as_slice()).unwrap_or(&[]);
    for input in [inputs.as_slice(), collaterals].concat().iter() {
        match context.lookup(input) {
            Some(output) => {
                let address = output.address().map_err(|e| {
                    InvalidInputs::UncategorizedError(format!(
                        "Invalid output address. (error {:?}) output: {:?}",
                        e, output,
                    ))
                })?;

                if let Some(credential) = address.credential() {
                    context.require_witness(credential);
                };

                if let Address::Byron(byron_address) = address {
                    let payload = byron_address.decode().map_err(|e| {
                        InvalidInputs::UncategorizedError(format!(
                            "Invalid byron address payload. (error {:?}) address: {:?}",
                            e, byron_address
                        ))
                    })?;

                    if let AddrType::PubKey = payload.addrtype {
                        context.require_bootstrap_witness(payload.root);
                    };
                };
            }
            None => Err(InvalidInputs::UnknownInput(input.clone()))?,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {

    use crate::context::assert::{AssertPreparationContext, AssertValidationContext};
    use amaru_kernel::{cbor, MintedTransactionBody};

    fn load_preperation_context_from_file(
        path: &str,
    ) -> Result<AssertPreparationContext, Box<dyn std::error::Error>> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let context = serde_json::from_reader(reader)?;
        Ok(context)
    }

    #[test]
    fn valid_reference_input_transactions() {
        // The following are transaction bodies with a variable number of inputs and reference inputs from preprod
        // Defining seperately for lifetime purposes. Certainly there's a nicer way to do this...
        let tx_7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b = include_bytes!("../../../tests/data/transactions/preprod/7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b/tx.cbor");
        let tx_537de728655d2da5fc21e57953a8650b25db8fc84e7dc51d85ebf6b8ea165596 = include_bytes!("../../../tests/data/transactions/preprod/537de728655d2da5fc21e57953a8650b25db8fc84e7dc51d85ebf6b8ea165596/tx.cbor");
        let tx_578feaed155aa44eb6e0e7780b47f6ce01043d79edabfae60fdb1cb6a3bfefb6 = include_bytes!("../../../tests/data/transactions/preprod/578feaed155aa44eb6e0e7780b47f6ce01043d79edabfae60fdb1cb6a3bfefb6/tx.cbor");
        let tx_d731b9832921c0cf9294eea0da2de215d0e9afd36126dc6af9af7e8d6310282a = include_bytes!("../../../tests/data/transactions/preprod/d731b9832921c0cf9294eea0da2de215d0e9afd36126dc6af9af7e8d6310282a/tx.cbor");
        let tx_6961d536a1f4d09204d5cfe3cc42949a0e803245fead9a36fad328bf4de9d2f4 = include_bytes!("../../../tests/data/transactions/preprod/6961d536a1f4d09204d5cfe3cc42949a0e803245fead9a36fad328bf4de9d2f4/tx.cbor");

        let cases: Vec<(MintedTransactionBody<'_>, AssertPreparationContext)> = vec![
            (
                cbor::decode(tx_7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b)
                    .expect("failed to decode tx body cbor"),
                load_preperation_context_from_file("tests/data/transactions/preprod/7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b/context.json").expect("failed to load prepreation context from file")
            ),
            (
                cbor::decode(tx_537de728655d2da5fc21e57953a8650b25db8fc84e7dc51d85ebf6b8ea165596)
                    .expect("failed to decode tx body cbor"),
                load_preperation_context_from_file("tests/data/transactions/preprod/537de728655d2da5fc21e57953a8650b25db8fc84e7dc51d85ebf6b8ea165596/context.json").expect("failed to load prepreation context from file")
            ),
            (
                cbor::decode(tx_578feaed155aa44eb6e0e7780b47f6ce01043d79edabfae60fdb1cb6a3bfefb6)
                    .expect("failed to decode tx body cbor"),
                load_preperation_context_from_file("tests/data/transactions/preprod/578feaed155aa44eb6e0e7780b47f6ce01043d79edabfae60fdb1cb6a3bfefb6/context.json").expect("failed to load prepreation context from file")

            ),
            (cbor::decode(tx_d731b9832921c0cf9294eea0da2de215d0e9afd36126dc6af9af7e8d6310282a)
                .expect("failed to decode tx body cbor"),
            load_preperation_context_from_file("tests/data/transactions/preprod/d731b9832921c0cf9294eea0da2de215d0e9afd36126dc6af9af7e8d6310282a/context.json").expect("failed to load prepreation context from file")
            ),
            (cbor::decode(tx_6961d536a1f4d09204d5cfe3cc42949a0e803245fead9a36fad328bf4de9d2f4)
                .expect("failed to decode tx body cbor"),
            load_preperation_context_from_file("tests/data/transactions/preprod/6961d536a1f4d09204d5cfe3cc42949a0e803245fead9a36fad328bf4de9d2f4/context.json").expect("failed to load prepreation context from file")

            )
        ];

        cases.into_iter().for_each(|(tx, ctx)| {
            let mut validation_context = AssertValidationContext::from(ctx.clone());
            assert!(super::execute(
                &mut validation_context,
                &tx.inputs,
                tx.reference_inputs.as_deref(),
                tx.collateral.as_deref(),
            )
            .is_ok());
        });
    }
}
