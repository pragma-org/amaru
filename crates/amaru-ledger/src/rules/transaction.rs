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

use crate::context::ValidationContext;
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, AuxiliaryData, KeepRaw, MintedTransactionBody,
    MintedWitnessSet, OriginalHash, TransactionInput, TransactionPointer,
};
use std::ops::Deref;
use thiserror::Error;

pub mod bootstrap_witness;
pub use bootstrap_witness::InvalidBootstrapWitnesses;

pub mod certificates;
pub use certificates::InvalidCertificates;

pub mod inputs;
pub use inputs::InvalidInputs;

pub mod metadata;
pub use metadata::InvalidTransactionMetadata;

pub mod outputs;
pub use outputs::InvalidOutputs;

pub mod vkey_witness;
pub use vkey_witness::InvalidVKeyWitness;

pub mod voting_procedures;

pub mod withdrawals;
pub use withdrawals::InvalidWithdrawals;

#[derive(Debug, Error)]
pub enum InvalidTransaction {
    #[error("invalid inputs: {0}")]
    Inputs(#[from] InvalidInputs),

    #[error("invalid outputs: {0}")]
    Outputs(#[from] InvalidOutputs),

    #[error("invalid certificates: {0}")]
    Certificates(#[from] InvalidCertificates),

    #[error("invalid withdrawals: {0}")]
    Withdrawals(#[from] InvalidWithdrawals),

    #[error("invalid transaction verification key witness: {0}")]
    VKeyWitness(#[from] InvalidVKeyWitness),

    #[error("invalid transaction bootstrap witness: {0}")]
    BootstrapWitnesses(#[from] InvalidBootstrapWitnesses),

    #[error("invalid transaction metadata: {0}")]
    Metadata(#[from] InvalidTransactionMetadata),
}

pub fn execute(
    context: &mut impl ValidationContext,
    protocol_params: &ProtocolParameters,
    pointer: TransactionPointer,
    is_valid: bool,
    transaction_body: KeepRaw<'_, MintedTransactionBody<'_>>,
    transaction_witness_set: &MintedWitnessSet<'_>,
    transaction_auxiliary_data: Option<&AuxiliaryData>,
) -> Result<(), InvalidTransaction> {
    let transaction_id = transaction_body.original_hash();

    let mut transaction_body = transaction_body.unwrap();

    metadata::execute(&transaction_body, transaction_auxiliary_data)?;

    certificates::execute(
        context,
        pointer,
        core::mem::take(&mut transaction_body.certificates),
    )?;

    inputs::execute(
        context,
        transaction_body.inputs.deref(),
        transaction_body.reference_inputs.as_deref(),
        transaction_body.collateral.as_deref(),
    )?;

    outputs::execute(
        protocol_params,
        core::mem::take(&mut transaction_body.outputs),
        &mut |index, output| {
            if is_valid {
                context.produce(
                    TransactionInput {
                        transaction_id,
                        index,
                    },
                    output,
                );
            }
        },
    )?;

    outputs::execute(
        protocol_params,
        core::mem::take(&mut transaction_body.collateral_return)
            .map(|x| vec![x])
            .unwrap_or_default(),
        &mut |index, output| {
            if !is_valid {
                // NOTE: Collateral outputs are indexed as if starting at the end of standard
                // outputs.
                let offset = transaction_body.outputs.len() as u64;
                context.produce(
                    TransactionInput {
                        transaction_id,
                        index: index + offset,
                    },
                    output,
                );
            }
        },
    )?;

    withdrawals::execute(context, transaction_body.withdrawals.as_deref())?;

    voting_procedures::execute(context, transaction_body.voting_procedures.as_deref());

    vkey_witness::execute(
        context,
        transaction_id,
        transaction_witness_set.vkeywitness.as_deref(),
    )?;

    bootstrap_witness::execute(
        context,
        transaction_id,
        transaction_witness_set.bootstrap_witness.as_deref(),
    )?;

    Ok(())
}
