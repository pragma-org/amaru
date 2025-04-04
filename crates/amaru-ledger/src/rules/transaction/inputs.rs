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
    #[error(
        "inputs included in both reference inputs and spent inputs: intersection [{}]",
        intersection
            .iter()
            .map(|input|
                format!("{}#{}", input.transaction_id, input.transaction_id)
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
                }

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
            None => unimplemented!("failed to lookup input: {input:?}"),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {

    use std::sync::LazyLock;
    use test_case::test_case;

    use crate::{
        context::assert::{AssertPreparationContext, AssertValidationContext},
        tests::{include_expected_traces, include_transaction_body, verify_traces, with_tracing},
    };
    use amaru_kernel::{cbor, KeepRaw, MintedTransactionBody};

    static TEST_CASES: LazyLock<
        [(
            KeepRaw<'_, MintedTransactionBody<'_>>,
            AssertPreparationContext,
            &str,
        ); 5],
    > = LazyLock::new(|| {
        macro_rules! include_preperation_context {
            ($hash:literal) => {
                serde_json::from_reader(std::io::BufReader::new(
                    std::fs::File::open(concat!(
                        "tests/data/transactions/preprod/",
                        $hash,
                        "/context.json"
                    ))
                    .unwrap(),
                ))
                .unwrap()
            };
        }

        macro_rules! include_test_data {
            ($path:literal, $hash:literal) => {
                (
                    include_transaction_body!($path, $hash),
                    include_preperation_context!($hash),
                    include_expected_traces!($path, $hash),
                )
            };
        }

        [
            include_test_data!(
                "../../../tests",
                "7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b"
            ),
            include_test_data!(
                "../../../tests",
                "537de728655d2da5fc21e57953a8650b25db8fc84e7dc51d85ebf6b8ea165596"
            ),
            include_test_data!(
                "../../../tests",
                "578feaed155aa44eb6e0e7780b47f6ce01043d79edabfae60fdb1cb6a3bfefb6"
            ),
            include_test_data!(
                "../../../tests",
                "d731b9832921c0cf9294eea0da2de215d0e9afd36126dc6af9af7e8d6310282a"
            ),
            include_test_data!(
                "../../../tests",
                "6961d536a1f4d09204d5cfe3cc42949a0e803245fead9a36fad328bf4de9d2f4"
            ),
        ]
    });
    #[test_case(&TEST_CASES[0])]
    #[test_case(&TEST_CASES[1])]
    #[test_case(&TEST_CASES[2])]
    #[test_case(&TEST_CASES[3])]
    #[test_case(&TEST_CASES[4])]
    fn valid_reference_input_transactions(
        (tx, ctx, expected_traces): &(
            KeepRaw<'_, MintedTransactionBody<'_>>,
            AssertPreparationContext,
            &str,
        ),
    ) {
        with_tracing(|collector| {
            let mut validation_context = AssertValidationContext::from(ctx.clone());
            assert!(super::execute(
                &mut validation_context,
                &tx.inputs,
                tx.reference_inputs.as_deref(),
                tx.collateral.as_deref(),
            )
            .is_ok());

            let actual_traces = collector.lines.lock().unwrap();
            match verify_traces(actual_traces.clone(), expected_traces) {
                Ok(_) => {}
                Err(e) => panic!("{:?}", e),
            }
        })
    }
}
