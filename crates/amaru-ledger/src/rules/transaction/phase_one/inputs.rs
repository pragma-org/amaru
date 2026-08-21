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

use amaru_kernel::{
    Address, AsHash, HasScriptHash, MemoizedDatum, ProtocolParameters, RedeemerTag, RequiredScript, Set,
    TransactionInput, address::byron::AddressType, utils::string::display_collection,
};
use thiserror::Error;

use crate::context::{BalanceSlice, UtxoSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidInputs {
    #[error("Unknown input: {0}")]
    UnknownInput(TransactionInput),

    #[error(
        "inputs included in both reference inputs and spent inputs: intersection [{}]",
        display_collection(.intersection),
    )]
    NonDisjointRefInputs { intersection: Vec<TransactionInput> },

    #[error("input set empty")]
    EmptyInputSet,

    #[error("reference scripts total bytes exceeds per-tx limit: (provided {provided}, allowed {allowed})")]
    RefScriptSizeTooBig { provided: u64, allowed: u64 },
}

pub fn execute<C>(
    context: &mut C,
    inputs: &Set<TransactionInput>,
    reference_inputs: Option<&[TransactionInput]>,
    protocol_parameters: &ProtocolParameters,
) -> Result<u64, InvalidInputs>
where
    C: UtxoSlice + WitnessSlice + BalanceSlice,
{
    if inputs.is_empty() {
        return Err(InvalidInputs::EmptyInputSet);
    }

    let mut intersection = Vec::new();
    let mut ref_scripts_size: u64 = 0;

    if let Some(reference_inputs) = reference_inputs {
        for reference_input in reference_inputs {
            // Non-disjoint reference inputs
            if inputs.contains(reference_input) {
                intersection.push(*reference_input);
                continue;
            }

            let output = context.lookup(reference_input).ok_or(InvalidInputs::UnknownInput(*reference_input))?;

            let script_ref = output.script.as_ref().map(|s| (s.script_hash(), s.len()));

            match &output.datum {
                MemoizedDatum::Inline(data) => context.acknowledge_datum(data.hash(), *reference_input),
                MemoizedDatum::Hash(hash) => {
                    context.allow_supplemental_datum(*hash.as_ref());
                }
                MemoizedDatum::None => (),
            };

            if let Some((script_hash, script_size)) = script_ref {
                ref_scripts_size += script_size;
                context.acknowledge_script(script_hash, *reference_input);
            }
        }
    }

    if !intersection.is_empty() {
        return Err(InvalidInputs::NonDisjointRefInputs { intersection });
    }

    let allowed = protocol_parameters.max_ref_script_size_per_tx as u64;

    for (input_index, input) in inputs.iter().enumerate() {
        let output = context.lookup(input).ok_or(InvalidInputs::UnknownInput(*input))?;

        let script_ref = output.script.as_ref().map(|s| (s.script_hash(), s.len()));

        // TODO: Avoid cloning here. Could probably be achieved by having 'RequiredScript'
        // always take a datum hash, and lookup its value when needed.
        let datum = output.datum.clone();

        // Clone the value off the borrowed output so the immutable borrow of `context` can be
        // released before we make any mutable calls below.
        let consumed_value = output.value.clone();

        match &output.address {
            Address::Byron(byron_address) => {
                if let AddressType::VerificationKey = byron_address.address_type {
                    context.require_bootstrap_witness(byron_address.root);
                };
            }
            Address::Shelley(shelley_address) => {
                if shelley_address.payment().is_script() {
                    context.require_script_witness(RequiredScript {
                        hash: shelley_address.payment().as_hash(),
                        index: input_index as u32,
                        purpose: RedeemerTag::Spend,
                        datum,
                    });
                } else {
                    context.require_verification_key_witness(shelley_address.payment().as_hash());
                }
            }
            Address::Stake(_) => unreachable!("found a stake address in a TransactionOutput"),
        }

        if let Some((script_hash, script_size)) = script_ref {
            ref_scripts_size += script_size;
            context.acknowledge_script(script_hash, *input);
        }

        context.consume_value(&consumed_value);
    }

    if ref_scripts_size > allowed {
        return Err(InvalidInputs::RefScriptSizeTooBig { provided: ref_scripts_size, allowed });
    }

    Ok(ref_scripts_size)
}
