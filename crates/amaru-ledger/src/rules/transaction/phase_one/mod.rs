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

use std::{fmt, mem, ops::Deref, time::Instant};

use amaru_kernel::{
    HasTransactionId, Network, ProtocolParameters, TransactionInput, TransactionRef, cardano::value::Balance,
    utils::duration::elapsed_and_reset,
};
use amaru_observability::debug_span;
use thiserror::Error;

use crate::{
    context::ValidationContext,
    rules::transaction::{Environment, phase_two::PreparationError},
};

pub mod certificates;
pub use certificates::InvalidCertificates;

pub mod fees;
pub use fees::{InvalidCollateral, InvalidFees};

pub mod inputs;
pub use inputs::InvalidInputs;

pub mod metadata;
pub use metadata::InvalidTransactionMetadata;

pub mod outputs;
pub use outputs::InvalidOutputs;

pub mod proposals;

pub mod verification_key_witness;
pub use verification_key_witness::InvalidVerificationKeyWitness;

pub mod voting_procedures;
pub use voting_procedures::InvalidVotingProcedures;

pub mod treasury;
pub use treasury::InvalidTreasury;

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

    #[error("script preparation failed: {0}")]
    ScriptPreparation(PreparationError),

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

    #[error("invalid treasury: {0}")]
    Treasury(#[from] InvalidTreasury),

    #[error("invalid transaction validity interval: {0}")]
    ValidityInterval(#[from] InvalidValidityInterval),

    #[error("value not preserved: balance = {0}")]
    ValueNotPreserved(Balance),
}

/// Runs the phase-one rules of a transaction that declares its scripts pass. Returns the inputs it spends: its regular
/// inputs.
pub(crate) fn when_valid<C>(
    context: &mut C,
    environment: &Environment<'_>,
    transaction: TransactionRef<'_>,
) -> Result<Vec<TransactionInput>, PhaseOneError>
where
    C: ValidationContext + fmt::Debug,
{
    use amaru_observability::amaru::ledger::rules::PHASE_ONE;

    let span = debug_span!(ledger::rules::PHASE_ONE);
    let _guard = span.enter();
    let mut meter = Instant::now();

    let Environment {
        arena_pool,
        protocol_parameters,
        era_history,
        governance_activity,
        guardrail_script,
        pointer,
        ..
    } = *environment;
    let mut transaction_body = transaction.body.clone();
    let transaction_id = transaction_body.tx_id();

    let network: Network = environment.network.into();

    fail_on_network_mismatch(transaction_body.network, network)?;
    fail_on_tx_size_too_large(transaction.len(), protocol_parameters)?;
    span.record(PHASE_ONE::FIELD_PREFLIGHT_MICROS, elapsed_and_reset(&mut meter));

    validity_interval::execute(
        transaction_body.validity_interval(),
        transaction.witnesses.redeemer.is_some(),
        era_history,
        pointer.slot,
    )?;
    span.record(PHASE_ONE::FIELD_VALIDITY_INTERVAL_MICROS, elapsed_and_reset(&mut meter));

    metadata::execute(arena_pool, &transaction_body, transaction.auxiliary_data, protocol_parameters.protocol_version)?;
    span.record(PHASE_ONE::FIELD_METADATA_MICROS, elapsed_and_reset(&mut meter));

    let inputs = inputs::execute(
        context,
        mem::take(&mut transaction_body.inputs),
        transaction_body.reference_inputs.as_deref(),
        transaction_body.collateral.as_deref().map(|xs| xs.to_vec()).unwrap_or_default(),
        protocol_parameters,
    )?;
    span.record(PHASE_ONE::FIELD_INPUTS_MICROS, elapsed_and_reset(&mut meter));

    fees::when_valid(
        context,
        fees::DeclaredFees {
            fees: transaction_body.fee,
            collateral: inputs.collateral,
            collateral_return: transaction_body.collateral_return.as_deref(),
            declared_collateral: transaction_body.total_collateral,
            tx_size: transaction.len(),
            witness_set: transaction.witnesses.deref(),
            ref_scripts_size: inputs.ref_scripts_size,
            protocol_parameters,
        },
    )?;
    span.record(PHASE_ONE::FIELD_FEES_MICROS, elapsed_and_reset(&mut meter));

    treasury::when_valid(context, transaction_body.treasury_value, transaction_body.donation.map(u64::from))?;
    span.record(PHASE_ONE::FIELD_TREASURY_MICROS, elapsed_and_reset(&mut meter));

    mint::execute(context, transaction_body.mint.as_ref());
    span.record(PHASE_ONE::FIELD_MINT_MICROS, elapsed_and_reset(&mut meter));

    outputs::when_valid(
        context,
        arena_pool,
        protocol_parameters,
        network,
        transaction_id,
        mem::take(&mut transaction_body.outputs),
        mem::take(&mut transaction_body.collateral_return).map(|output| *output),
    )?;
    span.record(PHASE_ONE::FIELD_OUTPUTS_MICROS, elapsed_and_reset(&mut meter));

    withdrawals::when_valid(
        context,
        mem::take(&mut transaction_body.withdrawals).map(|xs| xs.into_iter().collect()),
        network,
    )?;
    span.record(PHASE_ONE::FIELD_WITHDRAWALS_MICROS, elapsed_and_reset(&mut meter));

    certificates::when_valid(
        context,
        network,
        protocol_parameters,
        era_history,
        governance_activity,
        pointer,
        mem::take(&mut transaction_body.certificates),
    )?;
    span.record(PHASE_ONE::FIELD_CERTIFICATES_MICROS, elapsed_and_reset(&mut meter));

    proposals::when_valid(
        context,
        network,
        protocol_parameters,
        era_history,
        guardrail_script,
        (transaction_id, pointer),
        mem::take(&mut transaction_body.proposals).map(|xs| xs.into_iter().collect()),
    )?;
    span.record(PHASE_ONE::FIELD_PROPOSALS_MICROS, elapsed_and_reset(&mut meter));

    voting_procedures::when_valid(
        context,
        protocol_parameters.protocol_version,
        era_history,
        pointer,
        mem::take(&mut transaction_body.votes),
    )?;
    span.record(PHASE_ONE::FIELD_VOTES_MICROS, elapsed_and_reset(&mut meter));

    for vk_hash in transaction_body.required_signers.as_deref().unwrap_or(&[]) {
        context.require_verification_key_witness(*vk_hash);
    }

    let witness_set = transaction.witnesses.deref();

    verification_key_witness::execute(
        context,
        transaction_id,
        witness_set.bootstrap_witness.as_deref(),
        witness_set.verification_key_witness.as_deref(),
    )?;
    span.record(PHASE_ONE::FIELD_SIGNATURES_MICROS, elapsed_and_reset(&mut meter));

    scripts::execute(
        context,
        witness_set,
        transaction_body.validity_interval(),
        protocol_parameters,
        transaction_body.script_data_hash,
    )?;
    span.record(PHASE_ONE::FIELD_SCRIPTS_MICROS, elapsed_and_reset(&mut meter));

    let transaction_balance = context.balance();
    if !transaction_balance.is_zero() {
        return Err(PhaseOneError::ValueNotPreserved(transaction_balance));
    }

    Ok(inputs.spent)
}

/// Runs the phase-one rules of a transaction that declares its scripts fail. Returns the spent collateral inputs.
pub(crate) fn when_invalid<C>(
    context: &mut C,
    environment: &Environment<'_>,
    transaction: TransactionRef<'_>,
) -> Result<Vec<TransactionInput>, PhaseOneError>
where
    C: ValidationContext + fmt::Debug,
{
    use amaru_observability::amaru::ledger::rules::PHASE_ONE;

    let span = debug_span!(ledger::rules::PHASE_ONE);
    let _guard = span.enter();
    let mut meter = Instant::now();

    let Environment { arena_pool, protocol_parameters, era_history, pointer, .. } = *environment;
    let mut transaction_body = transaction.body.clone();
    let transaction_id = transaction_body.tx_id();

    let network: Network = environment.network.into();

    fail_on_network_mismatch(transaction_body.network, network)?;
    fail_on_tx_size_too_large(transaction.len(), protocol_parameters)?;
    span.record(PHASE_ONE::FIELD_PREFLIGHT_MICROS, elapsed_and_reset(&mut meter));

    validity_interval::execute(
        transaction_body.validity_interval(),
        transaction.witnesses.redeemer.is_some(),
        era_history,
        pointer.slot,
    )?;
    span.record(PHASE_ONE::FIELD_VALIDITY_INTERVAL_MICROS, elapsed_and_reset(&mut meter));

    metadata::execute(arena_pool, &transaction_body, transaction.auxiliary_data, protocol_parameters.protocol_version)?;
    span.record(PHASE_ONE::FIELD_METADATA_MICROS, elapsed_and_reset(&mut meter));

    let inputs = inputs::execute(
        context,
        mem::take(&mut transaction_body.inputs),
        transaction_body.reference_inputs.as_deref(),
        transaction_body.collateral.as_deref().map(|xs| xs.to_vec()).unwrap_or_default(),
        protocol_parameters,
    )?;
    span.record(PHASE_ONE::FIELD_INPUTS_MICROS, elapsed_and_reset(&mut meter));

    fees::when_invalid(
        context,
        fees::DeclaredFees {
            fees: transaction_body.fee,
            collateral: inputs.collateral,
            collateral_return: transaction_body.collateral_return.as_deref(),
            declared_collateral: transaction_body.total_collateral,
            tx_size: transaction.len(),
            witness_set: transaction.witnesses.deref(),
            ref_scripts_size: inputs.ref_scripts_size,
            protocol_parameters,
        },
    )?;
    span.record(PHASE_ONE::FIELD_FEES_MICROS, elapsed_and_reset(&mut meter));

    treasury::when_invalid(context, transaction_body.donation.map(u64::from));
    span.record(PHASE_ONE::FIELD_TREASURY_MICROS, elapsed_and_reset(&mut meter));

    mint::execute(context, transaction_body.mint.as_ref());
    span.record(PHASE_ONE::FIELD_MINT_MICROS, elapsed_and_reset(&mut meter));

    outputs::when_invalid(
        context,
        arena_pool,
        protocol_parameters,
        network,
        transaction_id,
        mem::take(&mut transaction_body.outputs),
        mem::take(&mut transaction_body.collateral_return).map(|output| *output),
    )?;
    span.record(PHASE_ONE::FIELD_OUTPUTS_MICROS, elapsed_and_reset(&mut meter));

    withdrawals::when_invalid(
        context,
        mem::take(&mut transaction_body.withdrawals).map(|xs| xs.into_iter().collect()),
        network,
    )?;
    span.record(PHASE_ONE::FIELD_WITHDRAWALS_MICROS, elapsed_and_reset(&mut meter));

    certificates::when_invalid(context, protocol_parameters, mem::take(&mut transaction_body.certificates));
    span.record(PHASE_ONE::FIELD_CERTIFICATES_MICROS, elapsed_and_reset(&mut meter));

    proposals::when_invalid(
        context,
        protocol_parameters,
        mem::take(&mut transaction_body.proposals).map(|xs| xs.into_iter().collect()),
    );
    span.record(PHASE_ONE::FIELD_PROPOSALS_MICROS, elapsed_and_reset(&mut meter));

    voting_procedures::when_invalid(context, mem::take(&mut transaction_body.votes));
    span.record(PHASE_ONE::FIELD_VOTES_MICROS, elapsed_and_reset(&mut meter));

    for vk_hash in transaction_body.required_signers.as_deref().unwrap_or(&[]) {
        context.require_verification_key_witness(*vk_hash);
    }

    let witness_set = transaction.witnesses.deref();

    verification_key_witness::execute(
        context,
        transaction_id,
        witness_set.bootstrap_witness.as_deref(),
        witness_set.verification_key_witness.as_deref(),
    )?;
    span.record(PHASE_ONE::FIELD_SIGNATURES_MICROS, elapsed_and_reset(&mut meter));

    scripts::execute(
        context,
        witness_set,
        transaction_body.validity_interval(),
        protocol_parameters,
        transaction_body.script_data_hash,
    )?;
    span.record(PHASE_ONE::FIELD_SCRIPTS_MICROS, elapsed_and_reset(&mut meter));

    let transaction_balance = context.balance();
    if !transaction_balance.is_zero() {
        return Err(PhaseOneError::ValueNotPreserved(transaction_balance));
    }

    Ok(inputs.collateral_inputs)
}

fn fail_on_tx_size_too_large(provided: u64, protocol_parameters: &ProtocolParameters) -> Result<(), PhaseOneError> {
    let maximum = u64::from(protocol_parameters.max_transaction_size);
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
