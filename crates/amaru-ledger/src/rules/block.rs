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
    protocol_parameters::ProtocolParameters, AuxiliaryData, ExUnits, Hash, MintedBlock,
    OriginalHash, StakeCredential, TransactionPointer,
};
use std::ops::{ControlFlow, Deref, FromResidual, Try};
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
    UncategorizedError(String),
}

#[derive(Debug)]
pub enum BlockValidation {
    Valid,
    Invalid(InvalidBlockDetails),
}

impl Try for BlockValidation {
    type Output = ();
    type Residual = InvalidBlockDetails;

    fn from_output((): Self::Output) -> Self {
        BlockValidation::Valid
    }

    fn branch(self) -> ControlFlow<Self::Residual, Self::Output> {
        match self {
            BlockValidation::Valid => ControlFlow::Continue(()),
            BlockValidation::Invalid(e) => ControlFlow::Break(e),
        }
    }
}

impl FromResidual for BlockValidation {
    fn from_residual(residual: InvalidBlockDetails) -> Self {
        BlockValidation::Invalid(residual)
    }
}

impl From<BlockValidation> for Result<(), InvalidBlockDetails> {
    fn from(validation: BlockValidation) -> Self {
        match validation {
            BlockValidation::Valid => Ok(()),
            BlockValidation::Invalid(e) => Err(e),
        }
    }
}

#[instrument(level = Level::TRACE, skip_all)]
pub fn execute<C: ValidationContext<FinalState = S>, S: From<C>>(
    context: &mut C,
    protocol_params: ProtocolParameters,
    block: MintedBlock<'_>,
) -> BlockValidation {
    // Block level validations functions share the same signature.
    // Currently apply them one by one
    // TODO consider an abstract strategy pattern to apply them (e.g. in parallel, in a priority order, ...)
    let block_validation_fns = vec![
        header_size::block_header_size_valid,
        body_size::block_body_size_valid,
        ex_units::block_ex_units_valid,
    ];

    for block_validation_fn in block_validation_fns {
        block_validation_fn(context, &block, &protocol_params)?;
    }

    let failed_transactions = FailedTransactions::from_block(&block);

    let witness_sets = block.transaction_witness_sets.deref().to_vec();

    let transactions = block.transaction_bodies.to_vec();

    // using `zip` here instead of enumerate as it is safer to cast from u32 to usize than usize to u32
    // Realistically, we're never gonna hit the u32 limit with the number of transactions in a block (a boy can dream)
    for (i, transaction) in (0u32..).zip(transactions.into_iter()) {
        let transaction_hash = transaction.original_hash();

        let witness_set = match witness_sets.get(i as usize) {
            Some(witness_set) => witness_set,
            None => {
                return BlockValidation::Invalid(InvalidBlockDetails::UncategorizedError(format!(
                    "Witness set not found for transaction index {}",
                    i
                )));
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
                context.require_witness(StakeCredential::AddrKeyhash(*vk_hash));
            });

        let pointer = TransactionPointer {
            slot: From::from(block.header.header_body.slot),
            transaction_index: i as usize, // From u32
        };

        if let Err(err) = transaction::execute(
            context,
            &protocol_params,
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

    BlockValidation::Valid
}
