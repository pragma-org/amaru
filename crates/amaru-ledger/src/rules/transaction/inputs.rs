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
use amaru_kernel::{
    AddrType, Address, HasScriptHash, MemoizedDatum, RequiredScript, ScriptPurpose,
    TransactionInput, TransactionInputAdapter,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidInputs {
    #[error("Unknown input: {0}")]
    UnknownInput(TransactionInputAdapter),
    #[error(
        "inputs included in both reference inputs and spent inputs: intersection [{}]",
        intersection
            .iter()
            .map(|input|
                input.to_string()
            )
            .collect::<Vec<_>>()
            .join(", ")
    )]
    NonDisjointRefInputs {
        intersection: Vec<TransactionInputAdapter>,
    },
    #[error("input set empty")]
    EmptyInputSet,
    // TODO: This error shouldn't exist, it's a placeholder for better error handling in less straight forward cases
    #[error("uncategorized error: {0}")]
    UncategorizedError(String),
}

pub fn execute<C>(
    context: &mut C,
    inputs: &[TransactionInput],
    reference_inputs: Option<&[TransactionInput]>,
) -> Result<(), InvalidInputs>
where
    C: UtxoSlice + WitnessSlice,
{
    if inputs.is_empty() {
        return Err(InvalidInputs::EmptyInputSet);
    }

    let mut intersection = Vec::new();

    if let Some(reference_inputs) = reference_inputs {
        for reference_input in reference_inputs {
            // Non-disjoint reference inputs
            if inputs.contains(reference_input) {
                intersection.push(reference_input.clone().into());
                continue;
            }

            let output = context
                .lookup(reference_input)
                .ok_or_else(|| InvalidInputs::UnknownInput(reference_input.clone().into()))?;

            let script_ref = output.script.as_ref().map(|s| s.script_hash());

            match &output.datum {
                MemoizedDatum::Inline(data) => {
                    context.acknowledge_datum(data.hash(), reference_input.clone())
                }
                MemoizedDatum::Hash(hash) => {
                    context.allow_supplemental_datum(*hash);
                }
                MemoizedDatum::None => (),
            };

            if let Some(script_hash) = script_ref {
                context.acknowledge_script(script_hash, reference_input.clone())
            }
        }
    }

    if !intersection.is_empty() {
        return Err(InvalidInputs::NonDisjointRefInputs { intersection });
    }

    /*
    The Haskell node sorts inputs lexicographically when deserializing.
    Pallas does not do this, and just provides a representation of exactly the bytes on the wire.

    As a result, we have to access the inputs in the correct lexicographical order, so that required scripts are indexed correctly
    */
    let mut indices: Vec<usize> = (0..inputs.len()).collect();
    indices.sort_by(|&a, &b| inputs[a].cmp(&inputs[b]));

    for (input_index, original_index) in indices.iter().enumerate() {
        let input = &inputs[*original_index];

        let output = context
            .lookup(input)
            .ok_or_else(|| InvalidInputs::UnknownInput(input.clone().into()))?;

        let script = output.script.as_ref().map(|script| script.script_hash());

        // TODO: Avoid cloning here. Could probably be achieved by having 'RequiredScript'
        // always take a datum hash, and lookup its value when needed.
        let datum = output.datum.clone();

        match &output.address {
            Address::Byron(byron_address) => {
                let payload = byron_address.decode().map_err(|e| {
                    InvalidInputs::UncategorizedError(format!(
                        "Invalid byron address payload. (error {:?}) address: {:?}",
                        e, byron_address
                    ))
                })?;

                if let AddrType::PubKey = payload.addrtype {
                    context.require_bootstrap_witness(payload.root);
                };
            }
            Address::Shelley(shelley_address) => {
                if shelley_address.payment().is_script() {
                    context.require_script_witness(RequiredScript {
                        hash: *shelley_address.payment().as_hash(),
                        index: input_index as u32,
                        purpose: ScriptPurpose::Spend,
                        datum,
                    });
                } else {
                    context.require_vkey_witness(*shelley_address.payment().as_hash());
                }
            }
            Address::Stake(_) => unreachable!("found a stake address in a TransactionOutput"),
        }

        if let Some(script_hash) = script {
            context.acknowledge_script(script_hash, input.clone());
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
    use amaru_kernel::{KeepRaw, MintedTransactionBody, include_cbor, include_json, json};
    use amaru_tracing_json::assert_trace;
    use test_case::test_case;

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
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "valid-byron-address");
        "valid byron address"
    )]
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "non-disjoint-reference-inputs") =>
        matches Err(InvalidInputs::NonDisjointRefInputs { intersection })
            if  intersection.len() == 1
                && InvalidInputs::NonDisjointRefInputs { intersection: intersection.clone() }.to_string() == "inputs included in both reference inputs and spent inputs: intersection [47a890217e4577ec3e6d5db161a4aa524a5cce3302e389ccb22b5662146f52ab#0]";
        "Non-Disjoint Reference Inputs"
    )]
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "unknown-input") =>
        matches Err(InvalidInputs::UnknownInput(input))
        if hex::encode(input.transaction_id) == "47a890217e4577ec3e6d5db161a4aa524a5cce3302e389ccb22b5662146f52ab" && input.index == 2;
        "unknown input"
    )]
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "invalid-byron-address") =>
        matches Err(InvalidInputs::UncategorizedError(e))
        if e.contains("InvalidByronCbor");
        "invalid byron payload"
    )]
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "empty-input-set") =>
        matches Err(InvalidInputs::EmptyInputSet);
        "empty input set"
    )]
    #[test_case(fixture!("7a098c13f3fb0119bc1ea6a418af3b9b8fef18bb65147872bf5037d28dda7b7b", "unknown-reference-input") =>
        matches Err(InvalidInputs::UnknownInput(..));
        "unknown reference input"
    )]
    fn inputs(
        (ctx, tx, expected_traces): (
            AssertPreparationContext,
            KeepRaw<'_, MintedTransactionBody<'_>>,
            Vec<json::Value>,
        ),
    ) -> Result<(), InvalidInputs> {
        assert_trace(
            move || {
                let mut validation_context = AssertValidationContext::from(ctx);
                super::execute(
                    &mut validation_context,
                    &tx.inputs,
                    tx.reference_inputs.as_deref().map(|vec| vec.as_slice()),
                )
            },
            expected_traces,
        )
    }
}
