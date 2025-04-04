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
use core::mem;
use std::ops::Deref;
use thiserror::Error;

pub mod bootstrap_witness;
pub use bootstrap_witness::InvalidBootstrapWitnesses;

pub mod certificates;
pub use certificates::InvalidCertificates;

pub mod fees;
pub use fees::InvalidFees;

pub mod inputs;
pub use inputs::InvalidInputs;

pub mod metadata;
pub use metadata::InvalidTransactionMetadata;

pub mod outputs;
pub use outputs::InvalidOutputs;

pub mod proposals;

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

    #[error("invalid fees: {0}")]
    Fees(#[from] InvalidFees),

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
    if let Some(withdrawals) = &transaction_body.withdrawals {
        println!(
            "Withdrawal Count: {:?}\ntx: {}\n{}",
            withdrawals.len(),
            transaction_body.original_hash(),
            hex::encode(transaction_body.raw_cbor())
        )
    }
    let transaction_id = transaction_body.original_hash();

    let mut transaction_body = transaction_body.unwrap();

    metadata::execute(&transaction_body, transaction_auxiliary_data)?;

    certificates::execute(
        context,
        pointer,
        mem::take(&mut transaction_body.certificates),
    )?;

    fees::execute(
        context,
        is_valid,
        transaction_body.fee,
        transaction_body.collateral.as_deref(),
        transaction_body.collateral_return.as_ref(),
    )?;

    inputs::execute(
        context,
        transaction_body.inputs.deref(),
        transaction_body.reference_inputs.as_deref(),
        transaction_body.collateral.as_deref(),
    )?;

    outputs::execute(
        protocol_params,
        mem::take(&mut transaction_body.collateral_return)
            .map(|x| vec![x])
            .unwrap_or_default(),
        &mut |_index, output| {
            if !is_valid {
                // NOTE(1): Collateral outputs are indexed based off the number of normal outputs.
                //
                // NOTE(2): We must process collateral before processing normal outputs, or, store
                // the output length elsewhere since after having consumed the outputs, the .len()
                // will always return zero.
                let offset = transaction_body.outputs.len() as u64;
                context.produce(
                    TransactionInput {
                        transaction_id,
                        index: offset,
                    },
                    output,
                );
            }
        },
    )?;

    outputs::execute(
        protocol_params,
        mem::take(&mut transaction_body.outputs),
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

    withdrawals::execute(context, transaction_body.withdrawals.as_deref())?;

    proposals::execute(
        context,
        transaction_id,
        mem::take(&mut transaction_body.proposal_procedures).map(|xs| xs.to_vec()),
    );

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

    // At last, consume inputs
    if is_valid {
        transaction_body.inputs.to_vec()
    } else {
        transaction_body
            .collateral
            .map(|x| x.to_vec())
            .unwrap_or_default()
    }
    .into_iter()
    .for_each(|input| context.consume(input));

    Ok(())
}
