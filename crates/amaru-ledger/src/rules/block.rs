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

pub use crate::rules::block::{
    body_size::InvalidBlockSize, ex_units::InvalidExUnits, header_size::InvalidBlockHeader,
};
use crate::{
    context::ValidationContext,
    rules::{transaction, transaction::InvalidTransaction},
    state::FailedTransactions,
};
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, AuxiliaryData, Hash, MintedBlock, OriginalHash,
    Redeemers, StakeCredential, TransactionPointer,
};
use std::ops::Deref;
use thiserror::Error;
use tracing::{instrument, Level};

#[derive(Debug, Error)]
pub enum InvalidBlock {
    #[error("Invalid block's size: {0}")]
    Size(#[from] InvalidBlockSize),

    #[error("Invalid block's execution units: {0}")]
    ExUnits(#[from] InvalidExUnits),

    #[error("Invalid block header: {0}")]
    Header(#[from] InvalidBlockHeader),

    #[error(
        "Invalid transaction (hash: {transaction_hash}, index: {transaction_index}): {violation} "
    )]
    Transaction {
        transaction_hash: Hash<32>,
        transaction_index: u32,
        violation: InvalidTransaction,
    },

    // TODO: This error shouldn't exist, it's a placeholder for better error handling in less straight forward cases
    #[error("Uncategorized error: {0}")]
    UncategorizedError(String),
}

#[instrument(level = Level::TRACE, skip_all)]
pub fn execute<C: ValidationContext<FinalState = S>, S: From<C>>(
    mut context: C,
    protocol_params: ProtocolParameters,
    block: MintedBlock<'_>,
) -> Result<S, InvalidBlock> {
    header_size::block_header_size_valid(block.header.raw_cbor(), &protocol_params)?;

    body_size::block_body_size_valid(&block.header.header_body, &block)?;

    // TODO: rewrite this to use iterators defined on `Redeemers` and `MaybeIndefArray`, ideally
    let ex_units = block
        .transaction_witness_sets
        .iter()
        .flat_map(|witness_set| {
            witness_set
                .redeemer
                .iter()
                .map(|redeemers| match redeemers.deref() {
                    Redeemers::List(list) => list.iter().map(|r| r.ex_units).collect::<Vec<_>>(),
                    Redeemers::Map(map) => map.iter().map(|(_, r)| r.ex_units).collect::<Vec<_>>(),
                })
        })
        .flatten()
        .collect::<Vec<_>>();

    ex_units::block_ex_units_valid(ex_units, &protocol_params)?;

    let failed_transactions = FailedTransactions::from_block(&block);

    let witness_sets = block.transaction_witness_sets.deref().to_vec();

    let transactions = block.transaction_bodies.to_vec();

    // using `zip` here instead of enumerate as it is safer to cast from u32 to usize than usize to u32
    // Realistically, we're never gonna hit the u32 limit with the number of transactions in a block (a boy can dream)
    for (i, transaction) in (0u32..).zip(transactions.into_iter()) {
        let transaction_hash = transaction.original_hash();

        let witness_set = witness_sets
            .get(i as usize)
            .ok_or(InvalidBlock::UncategorizedError(format!(
                "Missing witness set for transaction index {}",
                i
            )))?;

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

        transaction::execute(
            &mut context,
            &protocol_params,
            pointer,
            !failed_transactions.has(i),
            transaction,
            witness_set,
            auxiliary_data,
        )
        .map_err(|err| InvalidBlock::Transaction {
            transaction_hash,
            transaction_index: i,
            violation: err,
        })?;
    }

    Ok(context.into())
}
