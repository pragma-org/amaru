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
    #[error("Unknown input: {}#{}", .0.transaction_id, .0.index)]
    UnknownInput(TransactionInput),
    #[error(
        "inputs included in both reference inputs and spent inputs: intersection [{}]",
        intersection
            .iter()
            .map(|input|
                format!("{}#{}", input.transaction_id, input.index)
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
                // In theory, we could receive a stake address as an output destination here and it would be valid...
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
            None => Err(InvalidInputs::UnknownInput(input.clone()))?,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        context::assert::{AssertPreparationContext, AssertValidationContext},
        rules::tests::fixture_context,
    };
    use amaru_kernel::{include_cbor, include_json, json, KeepRaw, MintedTransactionBody};
    use test_case::test_case;
    use tracing_json::assert_trace;

    use super::InvalidInputs;

    macro_rules! fixture {
        ($hash:literal) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_json!(concat!("transactions/preprod/", $hash, "/expected.traces")),
            )
        };
        ($hash:literal, $variant:literal) => {
            (
                fixture_context!($hash, $variant),
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/tx.cbor"
                )),
                include_json!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/expected.traces"
                )),
            )
        };
    }

    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b"))]
    #[test_case(fixture!("537de728655d2da5fc21e57953a8650b25db8fc84e7dc51d85ebf6b8ea165596"))]
    #[test_case(fixture!("578feaed155aa44eb6e0e7780b47f6ce01043d79edabfae60fdb1cb6a3bfefb6"))]
    #[test_case(fixture!("d731b9832921c0cf9294eea0da2de215d0e9afd36126dc6af9af7e8d6310282a"))]
    #[test_case(fixture!("6961d536a1f4d09204d5cfe3cc42949a0e803245fead9a36fad328bf4de9d2f4"))]
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "non-disjoint-reference-inputs") =>
        matches Err(InvalidInputs::NonDisjointRefInputs { intersection })
            if  intersection.len() == 1
                && InvalidInputs::NonDisjointRefInputs { intersection: intersection.clone() }.to_string() == "inputs included in both reference inputs and spent inputs: intersection [47a890217e4577ec3e6d5db161a4aa524a5cce3302e389ccb22b5662146f52ab#0]";
        "Non-Disjoint Reference Inputs")]
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "invalid-address-header") =>
        matches Err(InvalidInputs::UncategorizedError(e))
            if e == "Invalid output address. (error InvalidHeader(160)) output: PostAlonzo(PseudoPostAlonzoTransactionOutput { address: Bytes(ByteVec([160, 83, 9, 250, 120, 104, 86, 193, 38, 45, 9, 91, 137, 173, 246, 79, 232, 165, 37, 90, 209, 145, 66, 201, 197, 55, 53, 158, 65])), value: Coin(0), datum_option: None, script_ref: None })";
            "invalid address header")]
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "unknown-input") =>
        matches Err(InvalidInputs::UnknownInput(input))
        if  hex::encode(input.transaction_id) == "47a890217e4577ec3e6d5db161a4aa524a5cce3302e389ccb22b5662146f52ab" && input.index == 2;
        "unknown input")]
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "invalid-byron-address") =>
        matches Err(InvalidInputs::UncategorizedError(e))
        if e == "Invalid byron address payload. (error InvalidByronCbor(Error { err: TypeMismatch(U8), pos: Some(31), msg: \"expected map\" })) address: ByronAddress { payload: TagWrap(ByteVec([130, 88, 28, 133, 24, 18, 154, 60, 13, 248, 227, 60, 64, 224, 75, 141, 38, 173, 59, 4, 34, 208, 250, 156, 169, 37, 88, 6, 163, 243, 139, 0])), crc: 3884043611 }";
        "invalid byron payload")]
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "valid-byron-address");
        "valid byron address")]
    fn inputs(
        (ctx, tx, expected_traces): (
            AssertPreparationContext,
            KeepRaw<'_, MintedTransactionBody<'_>>,
            Vec<json::Value>,
        ),
    ) -> Result<(), InvalidInputs> {
        assert_trace(
            || {
                let mut validation_context = AssertValidationContext::from(ctx.clone());
                super::execute(
                    &mut validation_context,
                    &tx.inputs,
                    tx.reference_inputs.as_deref(),
                    tx.collateral.as_deref(),
                )
            },
            expected_traces,
        )
    }
}
