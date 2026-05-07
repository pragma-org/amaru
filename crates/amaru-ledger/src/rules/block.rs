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

use std::{
    fmt::{self, Display},
    ops::{ControlFlow, FromResidual, Residual, Try},
    process::{ExitCode, Termination},
};

use amaru_kernel::{
    Block, EraHistory, ExUnits, HasExUnits, Hash, HeaderHash, NetworkName, ProtocolParameters, Slot, Transaction,
    TransactionId, TransactionPointer, size::BLOCK_BODY,
};
use amaru_observability::trace_span;
use amaru_plutus::arena_pool::ArenaPool;
use thiserror::Error;

use crate::{
    context::ValidationContext,
    rules::transaction::{self, phase_one::PhaseOneError, phase_two::PhaseTwoError},
    state::ValidationContextError,
    store::GovernanceActivity,
};

pub mod body_hash;
pub mod body_size;
pub mod ex_units;
pub mod header_size;
pub mod header_version;

#[derive(Debug, Error)]
pub enum TransactionInvalid {
    #[error("transaction failed phase one validation: {0}")]
    PhaseOneError(#[from] PhaseOneError),
    #[error("transaction failed phase two validation: {0}")]
    PhaseTwoError(#[from] PhaseTwoError),
}
#[derive(Debug)]
pub enum InvalidBlockDetails {
    BlockSizeMismatch { supplied: u64, actual: u64 },
    TooManyExUnits { provided: ExUnits, max: ExUnits },
    HeaderSizeTooBig { supplied: u64, max: u64 },
    InvalidBodyHash { header: Hash<BLOCK_BODY>, actual: Hash<BLOCK_BODY> },
    HeaderProtVerTooHigh { header_major: u64, max_major: u64 },
    Transaction { transaction_hash: TransactionId, transaction_index: u32, violation: TransactionInvalid },
}

#[derive(Debug, Error)]
pub enum TransactionValidationFailed {
    #[error("transaction {transaction_hash} is invalid: {violation}")]
    Transaction { transaction_hash: TransactionId, violation: TransactionInvalid },
    #[error("failed to prepare transaction {transaction_hash} for validation: {error}")]
    Preparation { transaction_hash: TransactionId, error: ValidationContextError },
}

#[derive(Debug)]
pub enum BlockValidation<A, E> {
    Valid(A),
    Invalid(Slot, HeaderHash, InvalidBlockDetails),
    Err(E),
}

pub enum BlockValidationResidual<E> {
    Invalid(Slot, HeaderHash, InvalidBlockDetails),
    Err(E),
}

fn display_ex_units(ex_units: &ExUnits) -> String {
    format!("ExUnits {{ mem: {}, steps: {} }}", ex_units.mem, ex_units.steps)
}

impl Display for InvalidBlockDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InvalidBlockDetails::BlockSizeMismatch { supplied, actual } => {
                write!(f, "Block size mismatch: supplied {}, actual {}", supplied, actual)
            }
            InvalidBlockDetails::TooManyExUnits { provided, max } => {
                write!(f, "Too many ExUnits: provided {}, max {}", display_ex_units(provided), display_ex_units(max))
            }
            InvalidBlockDetails::HeaderSizeTooBig { supplied, max } => {
                write!(f, "Header size too big: supplied {}, max {}", supplied, max)
            }
            InvalidBlockDetails::InvalidBodyHash { header, actual } => {
                write!(f, "Invalid body hash: header says {}, actual {}", header, actual)
            }
            InvalidBlockDetails::HeaderProtVerTooHigh { header_major, max_major } => {
                write!(f, "Header protocol version too high: {} > {}", header_major, max_major)
            }
            InvalidBlockDetails::Transaction { transaction_hash, transaction_index, violation } => {
                write!(f, "Transaction {} at index {} is invalid: {}", transaction_hash, transaction_index, violation)
            }
        }
    }
}

impl<A> BlockValidation<A, anyhow::Error> {
    pub fn bail(msg: String) -> Self {
        BlockValidation::Err(anyhow::Error::msg(msg))
    }

    pub fn anyhow<E>(err: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        BlockValidation::Err(anyhow::Error::new(err))
    }

    pub fn context<C>(self, context: C) -> Self
    where
        C: std::fmt::Display + Send + Sync + 'static,
    {
        match self {
            BlockValidation::Err(err) => BlockValidation::Err(err.context(context)),
            BlockValidation::Invalid { .. } | BlockValidation::Valid { .. } => self,
        }
    }
}

impl<A, E> Termination for BlockValidation<A, E> {
    fn report(self) -> ExitCode {
        match self {
            Self::Valid { .. } => ExitCode::SUCCESS,
            Self::Invalid { .. } | Self::Err { .. } => ExitCode::FAILURE,
        }
    }
}

impl<A, E> Try for BlockValidation<A, E> {
    type Output = A;
    type Residual = BlockValidationResidual<E>;

    fn from_output(result: Self::Output) -> Self {
        Self::Valid(result)
    }

    fn branch(self) -> ControlFlow<Self::Residual, Self::Output> {
        match self {
            Self::Valid(result) => ControlFlow::Continue(result),
            Self::Invalid(slot, id, violation) => {
                ControlFlow::Break(BlockValidationResidual::Invalid(slot, id, violation))
            }
            Self::Err(err) => ControlFlow::Break(BlockValidationResidual::Err(err)),
        }
    }
}

impl<A, E> FromResidual for BlockValidation<A, E> {
    fn from_residual(residual: BlockValidationResidual<E>) -> Self {
        match residual {
            BlockValidationResidual::Invalid(slot, id, violation) => BlockValidation::Invalid(slot, id, violation),
            BlockValidationResidual::Err(err) => BlockValidation::Err(err),
        }
    }
}

impl<A, E> Residual<A> for BlockValidationResidual<E> {
    type TryType = BlockValidation<A, E>;
}

pub fn execute<C, S: From<C>>(
    context: &mut C,
    arena_pool: &ArenaPool,
    network: &NetworkName,
    protocol_params: &ProtocolParameters,
    era_history: &EraHistory,
    governance_activity: &GovernanceActivity,
    block: Block,
) -> BlockValidation<(), anyhow::Error>
where
    C: ValidationContext<FinalState = S> + fmt::Debug,
{
    let _span = trace_span!(amaru_observability::amaru::ledger::state::VALIDATE_BLOCK);
    let _guard = _span.enter();

    let slot = Slot::from(block.header.header_body.slot);

    let header_hash = block.header_hash();

    let with_block_context = |result| match result {
        Ok(out) => BlockValidation::Valid(out),
        Err(err) => BlockValidation::Invalid(slot, header_hash, err),
    };

    with_block_context(header_size::block_header_size_valid(block.header_len(), protocol_params))?;

    with_block_context(body_size::block_body_size_valid(&block))?;

    with_block_context(body_hash::block_body_hash_valid(&block))?;

    with_block_context(header_version::block_header_version_valid(&block, protocol_params))?;

    with_block_context(ex_units::block_ex_units_valid(block.ex_units(), protocol_params))?;

    // using `zip` here instead of enumerate as it is safer to cast from u32 to usize than usize to u32
    // Realistically, we're never gonna hit the u32 limit with the number of transactions in a block (a boy can dream)
    for (i, transaction, tx_size) in block {
        let transaction_hash = transaction.body.id();

        if let Err(err) = validate_transaction(
            context,
            arena_pool,
            network,
            protocol_params,
            era_history,
            governance_activity,
            TransactionPointer { slot, transaction_index: i as usize },
            &transaction,
            tx_size,
        ) {
            return with_block_context(Err(InvalidBlockDetails::Transaction {
                transaction_hash,
                transaction_index: i,
                violation: match err {
                    TransactionValidationFailed::Transaction { violation, .. } => violation,
                    TransactionValidationFailed::Preparation { error, .. } => {
                        return BlockValidation::Err(error.into());
                    }
                },
            }));
        };
    }

    BlockValidation::Valid(())
}

/// Validate a single transaction against a validation context.
///
/// This runs:
///
/// - Phase-one and phase-two ledger rules.
/// - Records required vkey witnesses, and consumes the transaction inputs in the provided context after both
///   phases succeed.
///
/// The caller is responsible for preparing the context with the UTxO and other ledger slices required by the transaction.
#[expect(clippy::too_many_arguments)]
pub fn validate_transaction<C>(
    context: &mut C,
    arena_pool: &ArenaPool,
    network: &NetworkName,
    protocol_params: &ProtocolParameters,
    era_history: &EraHistory,
    governance_activity: &GovernanceActivity,
    pointer: TransactionPointer,
    transaction: &Transaction,
    tx_size: u64,
) -> Result<(), TransactionValidationFailed>
where
    C: ValidationContext + fmt::Debug,
{
    let transaction_hash = transaction.body.id();

    transaction.body.required_signers.as_deref().unwrap_or(&[]).iter().for_each(|vk_hash| {
        context.require_vkey_witness(*vk_hash);
    });

    let consumed_inputs = transaction::phase_one::execute(
        context,
        network,
        protocol_params,
        era_history,
        governance_activity,
        pointer,
        transaction.is_expected_valid,
        transaction.body.clone(),
        &transaction.witnesses,
        transaction.auxiliary_data.as_ref(),
        tx_size,
    )
    .map_err(|err| TransactionValidationFailed::Transaction { transaction_hash, violation: err.into() })?;

    transaction::phase_two::execute(
        context,
        arena_pool,
        network,
        protocol_params,
        era_history,
        pointer,
        transaction.is_expected_valid,
        &transaction.body,
        &transaction.witnesses,
    )
    .map_err(|err| TransactionValidationFailed::Transaction { transaction_hash, violation: err.into() })?;

    consumed_inputs.into_iter().for_each(|input| context.consume(input));
    Ok(())
}
