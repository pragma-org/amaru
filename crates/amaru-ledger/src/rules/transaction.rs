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

pub mod bootstrap_witness;
pub mod certificates;
pub mod inputs;
pub mod metadata;
pub mod outputs;
pub mod vkey_witness;

use crate::context::ValidationContext;
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, AuxiliaryData, KeepRaw, MintedTransactionBody,
    MintedWitnessSet, OriginalHash, TransactionInput,
};
pub use bootstrap_witness::InvalidBootstrapWitnesses;
pub use inputs::InvalidInputs;
pub use metadata::InvalidTransactionMetadata;
pub use outputs::InvalidOutputs;
use thiserror::Error;
pub use vkey_witness::InvalidVKeyWitness;

#[derive(Debug, Error)]
pub enum InvalidTransaction {
    #[error("invalid inputs: {0}")]
    Inputs(#[from] InvalidInputs),

    #[error("invalid outputs: {0}")]
    Outputs(#[from] InvalidOutputs),

    #[error("invalid transaction verification key witness: {0}")]
    VKeyWitness(#[from] InvalidVKeyWitness),

    #[error("invalid transaction bootstrap witness: {0}")]
    BootstrapWitnesses(#[from] InvalidBootstrapWitnesses),

    #[error("invalid transaction metadata: {0}")]
    Metadata(#[from] InvalidTransactionMetadata),
}

pub fn execute(
    context: &mut impl ValidationContext,
    is_valid: bool,
    transaction_body: &KeepRaw<'_, MintedTransactionBody<'_>>,
    transaction_witness_set: &MintedWitnessSet<'_>,
    transaction_auxiliary_data: Option<&AuxiliaryData>,
    protocol_params: &ProtocolParameters,
) -> Result<(), InvalidTransaction> {
    let new_output_reference = |index: u64| -> TransactionInput {
        TransactionInput {
            transaction_id: transaction_body.original_hash(),
            index,
        }
    };

    metadata::execute(transaction_body, transaction_auxiliary_data)?;

    inputs::execute(transaction_body)?;

    outputs::execute(
        protocol_params,
        transaction_body.outputs.iter(),
        &mut |index, output| {
            if is_valid {
                context.produce(new_output_reference(index), output);
            }
        },
    )?;

    outputs::execute(
        protocol_params,
        transaction_body.collateral_return.iter(),
        &mut |index, output| {
            if !is_valid {
                // NOTE: Collateral outputs are indexed as if starting at the end of standard
                // outputs.
                let offset = transaction_body.outputs.len() as u64;
                context.produce(new_output_reference(index + offset), output);
            }
        },
    )?;

    vkey_witness::execute(
        context,
        transaction_body,
        &transaction_witness_set.vkeywitness,
    )?;

    bootstrap_witness::execute(
        context,
        transaction_body,
        &transaction_witness_set.bootstrap_witness,
    )?;

    Ok(())
}
