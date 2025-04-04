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
    #[error("Unknown input")]
    UnknownInput(TransactionInput),
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
