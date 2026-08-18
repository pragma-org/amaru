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
    AuxiliaryData, EraHistory, HasTransactionId, Hash, Lovelace, Network, NetworkName, ProtocolParameters,
    TransactionBody, TransactionInput, TransactionPointer, WitnessSet, cardano::value::Balance, size::SCRIPT,
};
use amaru_observability::debug_span;
use amaru_plutus::arena_pool::ArenaPool;
use thiserror::Error;

use crate::{
    context::ValidationContext, epoch_transition::GovernanceActivity,
    rules::transaction::phase_one::outputs::SupplementalDatumPolicy,
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

pub mod verification_key_witness;
pub use verification_key_witness::InvalidVerificationKeyWitness;

pub mod voting_procedures;
pub use voting_procedures::InvalidVotingProcedures;

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
    VerificationKeyWitness(#[from] InvalidVerificationKeyWitness),

    #[error("invalid transaction scripts: {0}")]
    Scripts(#[from] InvalidScripts),

    #[error("invalid collateral: {0}")]
    Collateral(#[from] InvalidCollateral),

    #[error("invalid proposals: {0}")]
    Proposals(#[from] proposals::InvalidProposals),

    #[error("invalid voting procedures: {0}")]
    VotingProcedures(#[from] InvalidVotingProcedures),

    #[error("invalid transaction metadata: {0}")]
    Metadata(#[from] InvalidTransactionMetadata),

    #[error("invalid network in transaction body: expected {expected:?} provided {provided:?}")]
    InvalidNetwork { expected: Network, provided: Network },

    #[error("transaction too large: provided {provided} bytes, maximum {maximum} bytes")]
    TooLarge { provided: u64, maximum: u64 },

    #[error("current treasury value mismatch: provided {provided}, expected {expected}")]
    TreasuryValueMismatch { provided: Lovelace, expected: Lovelace },

    #[error("invalid transaction validity interval: {0}")]
    ValidityInterval(#[from] InvalidValidityInterval),

    #[error("value not preserved: balance = {0}")]
    ValueNotPreserved(Balance),
}

#[expect(clippy::too_many_arguments)]
pub fn execute<C>(
    context: &mut C,
    arena_pool: &ArenaPool,
    network_name: NetworkName,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    governance_activity: GovernanceActivity,
    guardrail_script: Option<Hash<SCRIPT>>,
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
    let transaction_id = transaction_body.tx_id();

    let network: Network = network_name.into();

    fail_on_network_mismatch(transaction_body.network, network)?;

    fail_on_tx_size_too_large(tx_size, protocol_parameters)?;

    debug_span!(ledger::rules::phase_one::VALIDITY_INTERVAL).in_scope(|| {
        validity_interval::execute(
            transaction_body.validity_interval(),
            transaction_witness_set.redeemer.is_some(),
            era_history,
            pointer.slot,
        )
    })?;

    debug_span!(ledger::rules::phase_one::METADATA).in_scope(|| {
        metadata::execute(&transaction_body, transaction_auxiliary_data, protocol_parameters.protocol_version)
    })?;

    let ref_scripts_size = debug_span!(ledger::rules::phase_one::INPUTS).in_scope(|| {
        inputs::execute(
            context,
            transaction_body.inputs.deref(),
            transaction_body.reference_inputs.as_deref(),
            protocol_parameters,
        )
    })?;

    let fees = debug_span!(ledger::rules::phase_one::FEES).in_scope(|| {
        fees::execute(
            context,
            transaction_body.fee,
            tx_size,
            transaction_witness_set,
            ref_scripts_size,
            protocol_parameters,
        )
    })?;

    if is_valid && let Some(provided) = transaction_body.treasury_value {
        let expected = context.treasury();
        if provided != expected {
            return Err(PhaseOneError::TreasuryValueMismatch { provided, expected });
        }
    }

    // TODO: The 'collateral' rule group shouldn't exist
    //
    // This is a mix of witness and fees; and instead of duplicating the collateral traversing
    // logic in both, we should augment fees and witness handling to also account for
    // collaterals.
    let collateral = debug_span!(ledger::rules::phase_one::COLLATERAL).in_scope(|| {
        collateral::execute(
            context,
            transaction_body.collateral.as_deref(),
            transaction_body.collateral_return.as_deref(),
            transaction_body.total_collateral,
            transaction_body.fee,
            protocol_parameters,
            transaction_witness_set.redeemer.is_some(),
        )
    })?;

    context.add_fees(if is_valid { fees } else { collateral });

    debug_span!(ledger::rules::phase_one::MINT).in_scope(|| mint::execute(context, transaction_body.mint.as_ref()));

    debug_span!(ledger::rules::phase_one::OUTPUTS).in_scope(|| {
        outputs::execute(
            context,
            protocol_parameters,
            network,
            mem::take(&mut transaction_body.collateral_return).map(|x| vec![*x]).unwrap_or_default(),
            SupplementalDatumPolicy::Disallow,
            |_context, _index, _value| {
                if is_valid {
                    return None;
                }

                // NOTE(1): Collateral outputs are indexed based off the number of normal outputs.
                //
                // NOTE(2): We must process collateral before processing normal outputs, or, store
                // the output length elsewhere since after having consumed the outputs, the .len()
                // will always return zero.
                let offset = transaction_body.outputs.len() as u64;
                Some(TransactionInput { transaction_id: *transaction_id, index: offset })
            },
        )?;

        outputs::execute(
            context,
            protocol_parameters,
            network,
            mem::take(&mut transaction_body.outputs),
            SupplementalDatumPolicy::Allow,
            |context, index, value| {
                context.produce_value(value);

                if !is_valid {
                    return None;
                }

                Some(TransactionInput { transaction_id: *transaction_id, index })
            },
        )?;

        Ok::<_, PhaseOneError>(())
    })?;

    debug_span!(ledger::rules::phase_one::WITHDRAWALS).in_scope(|| {
        withdrawals::execute(
            context,
            mem::take(&mut transaction_body.withdrawals).map(|xs| xs.to_vec()),
            network,
            is_valid,
        )
    })?;

    // NOTE: Following validations (and state changes) are entirely skipped on invalid transactions
    //
    // For invalid transactions, we only count the deposits and refunds necessary for value
    // preservation which happens also in the case of invalid transactions.
    if is_valid {
        debug_span!(ledger::rules::phase_one::CERTIFICATES).in_scope(|| {
            certificates::execute(
                context,
                network,
                protocol_parameters,
                era_history,
                governance_activity,
                pointer,
                mem::take(&mut transaction_body.certificates),
            )
        })?;

        debug_span!(ledger::rules::phase_one::PROPOSALS).in_scope(|| {
            proposals::execute(
                context,
                network,
                protocol_parameters,
                era_history,
                guardrail_script,
                (transaction_id, pointer),
                mem::take(&mut transaction_body.proposals).map(|xs| xs.to_vec()),
            )
        })?;

        debug_span!(ledger::rules::phase_one::VOTES).in_scope(|| {
            voting_procedures::execute(
                context,
                protocol_parameters.protocol_version,
                era_history,
                pointer,
                mem::take(&mut transaction_body.votes),
            )
        })?;
    } else {
        debug_span!(ledger::rules::phase_one::CERTIFICATES).in_scope(|| {
            certificates::count_lovelace(context, protocol_parameters, mem::take(&mut transaction_body.certificates))
        });
        debug_span!(ledger::rules::phase_one::PROPOSALS).in_scope(|| {
            proposals::count_lovelace(
                context,
                protocol_parameters,
                mem::take(&mut transaction_body.proposals).map(|xs| xs.to_vec()),
            )
        });
    }

    if let Some(donation) = transaction_body.donation.map(u64::from) {
        debug_span!(ledger::rules::phase_one::DONATION).in_scope(|| {
            context.produce_lovelace(donation);
            context.add_donation(donation);
        });
    }

    debug_span!(ledger::rules::phase_one::SIGNATURES).in_scope(|| {
        for vk_hash in transaction_body.required_signers.as_deref().unwrap_or(&[]) {
            context.require_verification_key_witness(*vk_hash);
        }

        verification_key_witness::execute(
            context,
            transaction_id,
            transaction_witness_set.bootstrap_witness.as_deref(),
            transaction_witness_set.verification_key_witness.as_deref(),
        )
    })?;

    debug_span!(ledger::rules::phase_one::SCRIPTS).in_scope(|| {
        scripts::execute(
            context,
            arena_pool,
            transaction_witness_set,
            transaction_body.validity_interval(),
            protocol_parameters,
            transaction_body.script_data_hash,
        )
    })?;

    let transaction_balance = context.balance();
    if !transaction_balance.is_zero() {
        return Err(PhaseOneError::ValueNotPreserved(transaction_balance));
    }

    // At last, consume inputs
    Ok(if is_valid {
        transaction_body.inputs.to_vec()
    } else {
        transaction_body.collateral.map(|x| x.to_vec()).unwrap_or_default()
    })
}

fn fail_on_tx_size_too_large(provided: u64, protocol_parameters: &ProtocolParameters) -> Result<(), PhaseOneError> {
    let maximum = protocol_parameters.max_transaction_size;
    if provided > maximum {
        return Err(PhaseOneError::TooLarge { provided, maximum });
    }
    Ok(())
}

fn fail_on_network_mismatch(provided: Option<Network>, expected: Network) -> Result<(), PhaseOneError> {
    if let Some(provided) = provided
        && expected != provided
    {
        return Err(PhaseOneError::InvalidNetwork { expected, provided });
    }

    Ok(())
}
