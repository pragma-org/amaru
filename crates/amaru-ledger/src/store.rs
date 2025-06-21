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
pub mod in_memory;

use crate::summary::Pots;
use amaru_kernel::{
    // NOTE: We have to import cbor as minicbor here because we derive 'Encode' and 'Decode' traits
    // instances for some types, and the macro rule handling that seems to be explicitly looking
    // for 'minicbor' in scope, and not an alias of any sort...
    cbor as minicbor,
    protocol_parameters::ProtocolParameters,
    CertificatePointer,
    Lovelace,
    Point,
    PoolId,
    StakeCredential,
    TransactionInput,
    TransactionOutput,
};
use columns::*;
use slot_arithmetic::Epoch;
use std::{borrow::BorrowMut, collections::BTreeSet, io, iter};
use thiserror::Error;

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
    #[error("no database tip. Did you forget to 'import' a snapshot first?")]
    Missing,
}

#[derive(Error, Debug)]
pub enum StoreError {
    #[error(transparent)]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("unable to decode database's value: {0}")]
    Undecodable(#[from] minicbor::decode::Error),
    #[error("error sending work unit through output port")]
    Send,
    #[error("error opening the store: {0}")]
    Open(#[source] OpenErrorKind),
    #[error("error opening the tip: {0}")]
    Tip(#[source] TipErrorKind),
    #[error("requested item not found")]
    NotFound,
}

// Store
// ----------------------------------------------------------------------------

pub trait ReadOnlyStore {
    /// Get the current protocol parameters for a given epoch, or most recent one
    fn get_protocol_parameters_for(&self, epoch: &Epoch) -> Result<ProtocolParameters, StoreError>;

    /// Get details about a specific Pool
    fn pool(&self, pool: &PoolId) -> Result<Option<pools::Row>, StoreError>;

    /// Get details about a specific Account
    fn account(&self, credential: &StakeCredential) -> Result<Option<accounts::Row>, StoreError>;

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
    #[allow(clippy::panic)]
    fn most_recent_snapshot(&self) -> Epoch {
        self.snapshots()
            .unwrap_or_default()
            .last()
            .copied()
            .unwrap_or_else(|| panic!("called 'epoch' on empty database?!"))
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

#[derive(Debug, PartialEq, Eq, minicbor::Encode, minicbor::Decode)]
pub enum EpochTransitionProgress {
    #[n(0)]
    EpochEnded,
    #[n(1)]
    SnapshotTaken,
    #[n(2)]
    EpochStarted,
}

/// A trait that provides a handle to perform atomic updates on the store.
pub trait TransactionalContext<'a> {
    /// Try to update the epoch transition progress so that we can recover from interruption within an
    /// epoch transition, if this ever happens.
    ///
    /// - return `True` and updates the store if the progress before the call matched the `from` argument.
    /// - returns `False` and does not update the store otherwise.
    fn try_epoch_transition(
        &self,
        from: Option<EpochTransitionProgress>,
        to: Option<EpochTransitionProgress>,
    ) -> Result<bool, StoreError>;

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
            impl Iterator<Item = (dreps::Key, CertificatePointer)>,
            impl Iterator<Item = cc_members::Key>,
            impl Iterator<Item = proposals::Key>,
        >,
        withdrawals: impl Iterator<Item = accounts::Key>,
        voting_dreps: BTreeSet<StakeCredential>,
    ) -> Result<(), StoreError>;

    /// Refund a deposit into an account. If the account no longer exists, returns the unrefunded
    /// deposit.
    fn refund(&self, credential: &accounts::Key, deposit: Lovelace)
        -> Result<Lovelace, StoreError>;

    /// Persist ProtocolParameters for a given epoch.
    fn set_protocol_parameters(
        &self,
        epoch: &Epoch,
        protocol_parameters: &ProtocolParameters,
    ) -> Result<(), StoreError>;

    /// Get current values of the treasury and reserves accounts, and possibly modify them.
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

    /// Commit the transaction. This will persist all changes to the store.
    fn commit(self) -> Result<(), StoreError>;

    /// Rollback the transaction. This will not persist any changes to the store.
    fn rollback(self) -> Result<(), StoreError>;
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
