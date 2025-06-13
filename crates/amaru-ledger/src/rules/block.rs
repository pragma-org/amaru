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

pub mod body_size;
pub mod ex_units;
pub mod header_size;

use crate::{
    context::ValidationContext,
    rules::{transaction, transaction::InvalidTransaction},
    state::FailedTransactions,
};
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, AuxiliaryData, ExUnits, HasExUnits, Hash, MintedBlock,
    OriginalHash, TransactionPointer,
};
use slot_arithmetic::Slot;
use std::{
    fmt::Display,
    ops::{ControlFlow, Deref, FromResidual, Try},
    process::{ExitCode, Termination},
};
use tracing::{instrument, Level};

#[derive(Debug)]
pub enum InvalidBlockDetails {
    BlockSizeMismatch {
        supplied: usize,
        actual: usize,
    },
    TooManyExUnits {
        provided: ExUnits,
        max: ExUnits,
    },
    HeaderSizeTooBig {
        supplied: usize,
        max: usize,
    },
    Transaction {
        transaction_hash: Hash<32>,
        transaction_index: u32,
        violation: InvalidTransaction,
    },
}

#[derive(Debug)]
pub enum BlockValidation<A, E> {
    Valid(A),
    Invalid(InvalidBlockDetails),
    Err(E),
}

fn display_ex_units(ex_units: &ExUnits) -> String {
    format!(
        "ExUnits {{ mem: {}, steps: {} }}",
        ex_units.mem, ex_units.steps
    )
}

impl Display for InvalidBlockDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InvalidBlockDetails::BlockSizeMismatch { supplied, actual } => {
                write!(
                    f,
                    "Block size mismatch: supplied {}, actual {}",
                    supplied, actual
                )
            }
            InvalidBlockDetails::TooManyExUnits { provided, max } => {
                write!(
                    f,
                    "Too many ExUnits: provided {}, max {}",
                    display_ex_units(provided),
                    display_ex_units(max)
                )
            }
            InvalidBlockDetails::HeaderSizeTooBig { supplied, max } => {
                write!(f, "Header size too big: supplied {}, max {}", supplied, max)
            }
            InvalidBlockDetails::Transaction {
                transaction_hash,
                transaction_index,
                violation,
            } => write!(
                f,
                "Transaction {} at index {} is invalid: {}",
                transaction_hash, transaction_index, violation
            ),
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
    type Residual = Result<InvalidBlockDetails, E>;

    fn from_output(result: Self::Output) -> Self {
        Self::Valid(result)
    }

    fn branch(self) -> ControlFlow<Self::Residual, Self::Output> {
        match self {
            Self::Valid(result) => ControlFlow::Continue(result),
            Self::Invalid(violation) => ControlFlow::Break(Ok(violation)),
            Self::Err(err) => ControlFlow::Break(Err(err)),
        }
    }
}

impl<A, E> FromResidual for BlockValidation<A, E> {
    fn from_residual(residual: Result<InvalidBlockDetails, E>) -> Self {
        match residual {
            Ok(violation) => BlockValidation::Invalid(violation),
            Err(err) => BlockValidation::Err(err),
        }
    }
}

#[instrument(level = Level::TRACE, skip_all)]
pub fn execute<C: ValidationContext<FinalState = S>, S: From<C>>(
    context: &mut C,
    protocol_params: &ProtocolParameters,
    block: &MintedBlock<'_>,
) -> BlockValidation<(), anyhow::Error> {
    header_size::block_header_size_valid(block.header.raw_cbor(), protocol_params)?;

    body_size::block_body_size_valid(block)?;

    ex_units::block_ex_units_valid(block.ex_units(), protocol_params)?;

    let failed_transactions = FailedTransactions::from_block(block);

    let witness_sets = block.transaction_witness_sets.deref().to_vec();

    let transactions = block.transaction_bodies.deref().to_vec();

    // using `zip` here instead of enumerate as it is safer to cast from u32 to usize than usize to u32
    // Realistically, we're never gonna hit the u32 limit with the number of transactions in a block (a boy can dream)
    for (i, transaction) in (0u32..).zip(transactions.into_iter()) {
        let transaction_hash = transaction.original_hash();

        let witness_set = match witness_sets.get(i as usize) {
            Some(witness_set) => witness_set,
            None => {
                // TODO: Define a proper error for this.
                return BlockValidation::bail(format!(
                    "Witness set not found for transaction index {}",
                    i
                ));
            }
        };

        let auxiliary_data: Option<&AuxiliaryData> = block
            .auxiliary_data_set
            .iter()
            .find(|key_pair| key_pair.0 == i)
            .map(|key_pair| key_pair.1.deref());

        transaction
            .required_signers
            .as_deref()
            .map(|x| x.as_slice())
            .unwrap_or(&[])
            .iter()
            .for_each(|vk_hash| {
                context.require_vkey_witness(*vk_hash);
            });

        let pointer = TransactionPointer {
            slot: Slot::from(block.header.header_body.slot),
            transaction_index: i as usize, // From u32
        };

        if let Err(err) = transaction::execute(
            context,
            protocol_params,
            pointer,
            !failed_transactions.has(i),
            transaction,
            witness_set,
            auxiliary_data,
        ) {
            return BlockValidation::Invalid(InvalidBlockDetails::Transaction {
                transaction_hash,
                transaction_index: i,
                violation: err,
            });
        }
    }

    BlockValidation::Valid(())
}
