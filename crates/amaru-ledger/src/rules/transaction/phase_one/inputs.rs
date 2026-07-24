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
    AddrType, Address, AddressError, HasScriptHash, MemoizedDatum, ProtocolParameters, RedeemerTag, RequiredScript,
    TransactionInput, cardano::memoized::script_size, cbor, transaction_input_to_string,
};
use thiserror::Error;

use crate::context::{BalanceSlice, UtxoSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidInputs {
    #[error("Unknown input: {}", transaction_input_to_string(.0))]
    UnknownInput(TransactionInput),
    #[error(
        "inputs included in both reference inputs and spent inputs: intersection [{}]",
        intersection
            .iter()
            .map(transaction_input_to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )]
    NonDisjointRefInputs { intersection: Vec<TransactionInput> },
    #[error("input set empty")]
    EmptyInputSet,
    #[error("invalid Byron address payload at input {}: {error}", transaction_input_to_string(input))]
    InvalidByronAddressPayload { input: TransactionInput, error: Box<cbor::decode::Error> },
    #[error("reference scripts total bytes exceeds per-tx limit: (provided {provided}, allowed {allowed})")]
    RefScriptSizeTooBig { provided: u64, allowed: u64 },
}

pub fn execute<C>(
    context: &mut C,
    inputs: &[TransactionInput],
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
                intersection.push(reference_input.clone());
                continue;
            }

            let output =
                context.lookup(reference_input).ok_or_else(|| InvalidInputs::UnknownInput(reference_input.clone()))?;

            let script_ref = output.script.as_ref().map(|s| (s.script_hash(), script_size(s)));

            match &output.datum {
                MemoizedDatum::Inline(data) => context.acknowledge_datum(data.hash(), reference_input.clone()),
                MemoizedDatum::Hash(hash) => {
                    context.allow_supplemental_datum(*hash);
                }
                MemoizedDatum::None => (),
            };

            if let Some((script_hash, script_size)) = script_ref {
                ref_scripts_size += script_size;
                context.acknowledge_script(script_hash, reference_input.clone());
            }
        }
    }

    if !intersection.is_empty() {
        return Err(InvalidInputs::NonDisjointRefInputs { intersection });
    }

    let allowed = protocol_parameters.max_ref_script_size_per_tx as u64;

    /*
    The Haskell node sorts inputs lexicographically when deserializing.
    Pallas does not do this, and just provides a representation of exactly the bytes on the wire.

    As a result, we have to access the inputs in the correct lexicographical order, so that required scripts are indexed correctly
    */
    let mut indices: Vec<usize> = (0..inputs.len()).collect();
    indices.sort_by(|&a, &b| inputs[a].cmp(&inputs[b]));

    for (input_index, original_index) in indices.iter().enumerate() {
        let input = &inputs[*original_index];

        let output = context.lookup(input).ok_or_else(|| InvalidInputs::UnknownInput(input.clone()))?;

        let script_ref = output.script.as_ref().map(|s| (s.script_hash(), script_size(s)));

        // TODO: Avoid cloning here. Could probably be achieved by having 'RequiredScript'
        // always take a datum hash, and lookup its value when needed.
        let datum = output.datum.clone();

        // Clone the value off the borrowed output so the immutable borrow of `context` can be
        // released before we make any mutable calls below.
        let consumed_value = output.value.as_ref().clone();

        match &output.address {
            Address::Byron(byron_address) => {
                let payload = byron_address.decode().map_err(|e| {
                    #[allow(clippy::wildcard_enum_match_arm)]
                    match e {
                        AddressError::InvalidByronCbor(error) => {
                            InvalidInputs::InvalidByronAddressPayload { input: input.clone(), error: Box::new(error) }
                        }
                        _ => unreachable!("byron_address.decode() only returns InvalidByronCbor"),
                    }
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
                        purpose: RedeemerTag::Spend,
                        datum,
                    });
                } else {
                    context.require_vkey_witness(*shelley_address.payment().as_hash());
                }
            }
            Address::Stake(_) => unreachable!("found a stake address in a TransactionOutput"),
        }

        if let Some((script_hash, script_size)) = script_ref {
            ref_scripts_size += script_size;
            context.acknowledge_script(script_hash, input.clone());
        }

        context.consume_value(&consumed_value);
    }

    if ref_scripts_size > allowed {
        return Err(InvalidInputs::RefScriptSizeTooBig { provided: ref_scripts_size, allowed });
    }

    Ok(ref_scripts_size)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS;

    use super::InvalidInputs;
    use crate::{
        context::DefaultValidationContext,
        tests::{fake_input, fake_output},
    };

    /// A Byron address with a well-formed envelope (so it resolves as an address) but whose inner
    /// payload is a two-element array instead of the `(root, attributes, addrtype)` triple, so
    /// `decode()` fails. Haskell cannot represent this state, hence no conformance predicate for it.
    const UNDECODABLE_BYRON_ADDRESS: &str =
        "82D818582082581C8518129A3C0DF8E33C40E04B8D26AD3B0422D0FA9CA9255806A3F38B001AE781CD5B";

    #[test]
    fn rejects_an_input_whose_byron_address_payload_does_not_decode() {
        let input = fake_input("47a890217e4577ec3e6d5db161a4aa524a5cce3302e389ccb22b5662146f52ab", 2);

        let mut context = DefaultValidationContext::new(
            BTreeMap::from([(input.clone(), fake_output(UNDECODABLE_BYRON_ADDRESS))]),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        );

        assert!(matches!(
            super::execute(&mut context, &[input], None, &PREPROD_DEFAULT_PROTOCOL_PARAMETERS),
            Err(InvalidInputs::InvalidByronAddressPayload { .. })
        ));
    }
}
