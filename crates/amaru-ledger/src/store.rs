// Copyright 2024 PRAGMA
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

pub mod columns;

use crate::summary::rewards::{Pots, RewardsSummary};
use amaru_kernel::{
    cbor, expect_stake_credential, Epoch, Lovelace, Point, PoolId, StakeCredential,
    TransactionInput, TransactionOutput,
};
use columns::*;
use std::{
    borrow::BorrowMut,
    collections::{BTreeMap, BTreeSet},
    io, iter,
};
use thiserror::Error;
use tracing::{info, instrument, Level};

#[derive(Debug, Error)]
#[error(transparent)]
pub enum OpenErrorKind {
    #[error(transparent)]
    IO(#[from] io::Error),
    #[error("no ledger stable snapshot found; at least one is expected")]
    NoStableSnapshot,
}

#[derive(Debug, Error)]
#[error(transparent)]
pub enum TipErrorKind {
    #[error("unable to decode database's tip")]
    Undecodable(#[from] cbor::decode::Error),
    #[error("no database tip. Did you forget to 'import' a snapshot first?")]
    Missing,
}

#[derive(Error, Debug)]
pub enum StoreError {
    #[error(transparent)]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("error sending work unit through output port")]
    Send,
    #[error("error opening the store")]
    Open(#[source] OpenErrorKind),
    #[error("error opening the tip")]
    Tip(#[source] TipErrorKind),
}

// Store
// ----------------------------------------------------------------------------

pub trait ReadOnlyStore {
    /// Get details about a specific Pool
    fn pool(&self, pool: &PoolId) -> Result<Option<pools::Row>, StoreError>;

    /// Get details about a specific UTxO
    fn utxo(&self, input: &TransactionInput) -> Result<Option<TransactionOutput>, StoreError>;

    /// Get current values of the treasury and reserves accounts.
    fn pots(&self) -> Result<Pots, StoreError>;

    /// Get details about all utxos
    fn iter_utxos(&self) -> Result<impl Iterator<Item = (utxo::Key, utxo::Value)>, StoreError>;

    /// Get details about all slot leaders
    fn iter_block_issuers(
        &self,
    ) -> Result<impl Iterator<Item = (slots::Key, slots::Value)>, StoreError>;

    /// Get details about all Pools
    fn iter_pools(&self) -> Result<impl Iterator<Item = (pools::Key, pools::Row)>, StoreError>;

    /// Get details about all accounts
    fn iter_accounts(
        &self,
    ) -> Result<impl Iterator<Item = (accounts::Key, accounts::Row)>, StoreError>;

    /// Get details about all dreps
    fn iter_dreps(&self) -> Result<impl Iterator<Item = (dreps::Key, dreps::Row)>, StoreError>;

    /// Get details about all proposals
    fn iter_proposals(
        &self,
    ) -> Result<impl Iterator<Item = (proposals::Key, proposals::Row)>, StoreError>;
}

pub trait Snapshot: ReadOnlyStore {
    fn epoch(&self) -> Epoch;
}

pub trait Store: ReadOnlyStore {
    /// The most recent snapshot. Note that we never starts from genesis; so there's always a
    /// snapshot available.
    fn most_recent_snapshot(&self) -> Option<Epoch> {
        self.snapshots().unwrap_or_default().last().cloned()
    }

    /// Get a list of all snapshots available. The list is ordered from the oldest to the newest.
    fn snapshots(&self) -> Result<Vec<Epoch>, StoreError>;

    /// Construct and save on-disk a snapshot of the store. The epoch number is used when
    /// there's no existing snapshot and, to ensure that snapshots are taken in order.
    ///
    /// Idempotent
    ///
    /// /!\ IMPORTANT /!\
    /// It is the **caller's** responsibility to ensure that the snapshot is done at the right
    /// moment. The store has no notion of when is an epoch boundary, and thus deferred that
    /// decision entirely to the caller owning the store.
    fn next_snapshot(&self, epoch: Epoch) -> Result<(), StoreError>;

    /// Create a new transaction context. This is used to perform updates on the store.
    fn create_transaction(&self) -> impl TransactionalContext<'_>;

    /// Access the tip of the stable store, corresponding to the latest point that was saved.
    fn tip(&self) -> Result<Point, StoreError>;
}

pub trait HistoricalStores {
    ///Access a `Snapshot` for a specific `Epoch`
    fn for_epoch(&self, epoch: Epoch) -> Result<impl Snapshot, StoreError>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Progress {
    Snapshot,
    BlockProcessed,
}

impl<C> cbor::encode::Encode<C> for Progress {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Progress::Snapshot => e.encode_with(0, ctx),
            Progress::BlockProcessed => e.encode_with(1, ctx),
        }?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Progress {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.decode_with(ctx).map(|v: u8| match v {
            0 => Progress::Snapshot,
            1 => Progress::BlockProcessed,
            _ => unreachable!("invalid progress value"),
        })
    }
}

/// A trait that provides a handle to perform atomic updates on the store.
pub trait TransactionalContext<'a> {
    /// Update the block processing process to State
    fn update_progress(&self, new_progress: Progress) -> Result<Option<Progress>, StoreError>;

    fn reset_fees(&self) -> Result<(), StoreError>;

    fn reset_blocks_count(&self) -> Result<(), StoreError>;

    fn apply_rewards(&self, rewards_summary: &mut RewardsSummary) -> Result<(), StoreError>;

    fn adjust_pots(
        &self,
        delta_treasury: u64,
        delta_reserves: u64,
        unclaimed_rewards: u64,
    ) -> Result<(), StoreError>;

    /// Add or remove entries to/from the store. The exact semantic of 'add' and 'remove' depends
    /// on the column type. All updates are atomatic and attached to the given `Point`.
    fn save(
        &self,
        point: &Point,
        issuer: Option<&pools::Key>,
        add: Columns<
            impl Iterator<Item = (utxo::Key, utxo::Value)>,
            impl Iterator<Item = pools::Value>,
            impl Iterator<Item = (accounts::Key, accounts::Value)>,
            impl Iterator<Item = (dreps::Key, dreps::Value)>,
            impl Iterator<Item = (cc_members::Key, cc_members::Value)>,
            impl Iterator<Item = (proposals::Key, proposals::Value)>,
        >,
        remove: Columns<
            impl Iterator<Item = utxo::Key>,
            impl Iterator<Item = (pools::Key, Epoch)>,
            impl Iterator<Item = accounts::Key>,
            impl Iterator<Item = dreps::Key>,
            impl Iterator<Item = cc_members::Key>,
            impl Iterator<Item = proposals::Key>,
        >,
        withdrawals: impl Iterator<Item = accounts::Key>,
        voting_dreps: BTreeSet<StakeCredential>,
    ) -> Result<(), StoreError>;

    /// Return deposits back to reward accounts.
    fn refund(
        &self,
        refunds: impl Iterator<Item = (StakeCredential, Lovelace)>,
    ) -> Result<(), StoreError>;

    fn set_pots(
        &self,
        treasury: Lovelace,
        reserves: Lovelace,
        fees: Lovelace,
    ) -> Result<(), StoreError>;

    /// Get current values of the treasury and reserves accounts.
    fn with_pots(
        &self,
        with: impl FnMut(Box<dyn BorrowMut<pots::Row> + '_>),
    ) -> Result<(), StoreError>;

    /// Provide an access to iterate over pools, in a way that enforces:
    ///
    /// 1. That mutations will be persisted on-disk
    ///
    /// 2. That all operations are consistent and atomic (the iteration occurs on a snapshot, and
    ///    the mutation apply to the iterated items)
    fn with_pools(&self, with: impl FnMut(pools::Iter<'_, '_>)) -> Result<(), StoreError>;

    /// Provide an access to iterate over accounts, similar to 'with_pools'.
    fn with_accounts(&self, with: impl FnMut(accounts::Iter<'_, '_>)) -> Result<(), StoreError>;

    /// Provide an iterator over slot leaders, similar to 'with_pools'. Note that slot leaders are
    /// stored as a bounded FIFO, so it only make sense to use this function at the end of an epoch
    /// (or at the beginning, before any block is applied, depending on your perspective).
    fn with_block_issuers(&self, with: impl FnMut(slots::Iter<'_, '_>)) -> Result<(), StoreError>;

    /// Provide an access to iterate over utxo, similar to 'with_pools'.
    fn with_utxo(&self, with: impl FnMut(utxo::Iter<'_, '_>)) -> Result<(), StoreError>;

    /// Provide an access to iterate over dreps, similar to 'with_pools'.
    fn with_dreps(&self, with: impl FnMut(dreps::Iter<'_, '_>)) -> Result<(), StoreError>;

    /// Provide an access to iterate over dreps, similar to 'with_pools'.
    fn with_proposals(&self, with: impl FnMut(proposals::Iter<'_, '_>)) -> Result<(), StoreError>;

    #[instrument(level = Level::TRACE, name = "tick.pool", skip_all)]
    fn tick_pools(&self, epoch: Epoch) -> Result<(), StoreError> {
        self.with_pools(|iterator| {
            for (_, pool) in iterator {
                pools::Row::tick(pool, epoch)
            }
        })
    }

    fn tick_proposals(&self, epoch: Epoch) -> Result<(), StoreError> {
        info!(epoch, "tick proposal");

        let mut refunds: BTreeMap<StakeCredential, Lovelace> = BTreeMap::new();

        self.with_proposals(|iterator| {
            for (_, item) in iterator {
                if let Some(row) = item.borrow() {
                    if epoch == row.valid_until + 2 {
                        refunds.insert(
                            expect_stake_credential(&row.proposal.reward_account),
                            row.proposal.deposit,
                        );
                    }
                }
            }
        })?;

        self.refund(refunds.into_iter())
    }

    /// Commit the transaction. This will persist all changes to the store.
    fn commit(self) -> Result<(), StoreError>;
}

// Columns
// ----------------------------------------------------------------------------

/// A summary of all database columns, in a single struct. This can be derived to provide updates
/// operations on multiple columns in a single db-transaction.
pub struct Columns<U, P, A, D, C, PP> {
    pub utxo: U,
    pub pools: P,
    pub accounts: A,
    pub dreps: D,
    pub cc_members: C,
    pub proposals: PP,
}

impl<U, P, A, D, C, PP> Default
    for Columns<
        iter::Empty<U>,
        iter::Empty<P>,
        iter::Empty<A>,
        iter::Empty<D>,
        iter::Empty<C>,
        iter::Empty<PP>,
    >
{
    fn default() -> Self {
        Self {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: iter::empty(),
            cc_members: iter::empty(),
            proposals: iter::empty(),
        }
    }
}
