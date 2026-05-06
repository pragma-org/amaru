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

use std::{fmt, mem, ops::Deref};

use amaru_kernel::{
    AuxiliaryData, EraHistory, Network, NetworkId, NetworkName, ProtocolParameters, TransactionBody, TransactionInput,
    TransactionPointer, WitnessSet,
};
use thiserror::Error;

use crate::{
    context::ValidationContext, rules::transaction::phase_one::outputs::SupplementalDatumPolicy,
    store::GovernanceActivity,
};

pub mod certificates;
pub use certificates::InvalidCertificates;

pub mod fees;
pub use fees::InvalidFees;

pub mod inputs;
pub use inputs::InvalidInputs;

pub mod collateral;
pub use collateral::InvalidCollateral;

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

pub mod scripts;
pub use scripts::InvalidScripts;

pub mod native_scripts;

pub mod validity_interval;
pub use validity_interval::InvalidValidityInterval;

pub mod mint;

#[derive(Debug, Error)]
pub enum PhaseOneError {
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

    #[error("invalid transaction scripts: {0}")]
    Scripts(#[from] InvalidScripts),

    #[error("invalid collateral: {0}")]
    Collateral(#[from] InvalidCollateral),

    #[error("invalid proposals: {0}")]
    Proposals(#[from] proposals::InvalidProposals),

    #[error("invalid transaction metadata: {0}")]
    Metadata(#[from] InvalidTransactionMetadata),

    #[error("invalid network ID in transaction body: expected {expected:?} provided {provided:?}")]
    InvalidNetworkID { expected: Network, provided: Network },

    #[error("transaction too large: provided {provided} bytes, maximum {maximum} bytes")]
    TooLarge { provided: u64, maximum: u64 },

    #[error("invalid transaction validity interval: {0}")]
    ValidityInterval(#[from] InvalidValidityInterval),
}

#[expect(clippy::too_many_arguments)]
pub fn execute<C>(
    context: &mut C,
    network_name: &NetworkName,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    governance_activity: &GovernanceActivity,
    pointer: TransactionPointer,
    is_valid: bool,
    mut transaction_body: TransactionBody,
    transaction_witness_set: &WitnessSet,
    transaction_auxiliary_data: Option<&AuxiliaryData>,
    tx_size: u64,
) -> Result<Vec<TransactionInput>, PhaseOneError>
where
    C: ValidationContext + fmt::Debug,
{
    let transaction_id = transaction_body.id();

    let network: Network = (*network_name).into();

    fail_on_network_mismatch(transaction_body.network_id, network)?;

    fail_on_tx_size_too_large(tx_size, protocol_parameters)?;

    validity_interval::execute(
        transaction_body.validity_interval(),
        transaction_witness_set.redeemer.is_some(),
        era_history,
        pointer.slot,
    )?;

    metadata::execute(&transaction_body, transaction_auxiliary_data, protocol_parameters.protocol_version)?;

    certificates::execute(
        context,
        network,
        protocol_parameters,
        era_history,
        governance_activity,
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

    inputs::execute(context, transaction_body.inputs.deref(), transaction_body.reference_inputs.as_deref())?;

    if transaction_witness_set.redeemer.is_some() {
        collateral::execute(
            context,
            transaction_body.collateral.as_deref(),
            transaction_body.collateral_return.as_ref(),
            transaction_body.total_collateral,
            transaction_body.fee,
            protocol_parameters,
        )?;
    }

    mint::execute(context, transaction_body.mint.as_ref());

    outputs::execute(
        context,
        protocol_parameters,
        network,
        mem::take(&mut transaction_body.collateral_return).map(|x| vec![x]).unwrap_or_default(),
        SupplementalDatumPolicy::Disallow,
        |_index| {
            if is_valid {
                return None;
            }

            // NOTE(1): Collateral outputs are indexed based off the number of normal outputs.
            //
            // NOTE(2): We must process collateral before processing normal outputs, or, store
            // the output length elsewhere since after having consumed the outputs, the .len()
            // will always return zero.
            let offset = transaction_body.outputs.len() as u64;
            Some(TransactionInput { transaction_id, index: offset })
        },
    )?;

    outputs::execute(
        context,
        protocol_parameters,
        network,
        mem::take(&mut transaction_body.outputs),
        SupplementalDatumPolicy::Allow,
        |index| {
            if !is_valid {
                return None;
            }

            Some(TransactionInput { transaction_id, index })
        },
    )?;

    withdrawals::execute(context, mem::take(&mut transaction_body.withdrawals).map(|xs| xs.to_vec()), network)?;

    proposals::execute(
        context,
        network,
        protocol_parameters,
        era_history,
        (transaction_id, pointer),
        mem::take(&mut transaction_body.proposals).map(|xs| xs.to_vec()),
    )?;

    voting_procedures::execute(context, mem::take(&mut transaction_body.votes));

    vkey_witness::execute(
        context,
        transaction_id,
        transaction_witness_set.bootstrap_witness.as_deref(),
        transaction_witness_set.vkeywitness.as_deref(),
    )?;

    scripts::execute(context, transaction_witness_set, transaction_body.validity_interval(), protocol_parameters)?;

    // At last, consume inputs
    let consumed_inputs = if is_valid {
        transaction_body.inputs.to_vec()
    } else {
        transaction_body.collateral.map(|x| x.to_vec()).unwrap_or_default()
    };

    Ok(consumed_inputs)
}

fn fail_on_tx_size_too_large(provided: u64, protocol_parameters: &ProtocolParameters) -> Result<(), PhaseOneError> {
    let maximum = protocol_parameters.max_transaction_size;
    if provided > maximum {
        return Err(PhaseOneError::TooLarge { provided, maximum });
    }
    Ok(())
}

fn fail_on_network_mismatch(provided: Option<NetworkId>, network: Network) -> Result<(), PhaseOneError> {
    if let Some(network_id) = provided {
        let provided: Network = u8::from(network_id).into();
        if network != provided {
            return Err(PhaseOneError::InvalidNetworkID { expected: network, provided });
        }
    }

    Ok(())
}
