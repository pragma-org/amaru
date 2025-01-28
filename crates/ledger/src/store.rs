pub mod columns;

use super::kernel::{Epoch, Point, PoolId, TransactionInput, TransactionOutput};
pub use crate::rewards::RewardsSummary;
use columns::*;
use miette::Diagnostic;
use std::{borrow::BorrowMut, io, iter};
use thiserror::Error;

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum OpenErrorKind {
    #[error(transparent)]
    IO(#[from] io::Error),
    #[error("no ledger stable snapshot found; at least one is expected")]
    NoStableSnapshot,
}

#[derive(Error, Diagnostic, Debug)]
pub enum StoreError {
    #[error(transparent)]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),
    #[error("error sending work unit through output port")]
    Send,
    #[error("error opening the store")]
    Open(#[source] OpenErrorKind),
}

// Store
// ----------------------------------------------------------------------------

pub trait Stores {
    fn live(&self) -> Result<impl Store, StoreError>;

    fn for_epoch<const N: u32>(&self, epoch: u32) -> Result<impl StoreForEpoch<N>, StoreError>;
}

pub trait StoreForEpoch<const N: u32>: Store {}

pub trait Store: Send + Sync {
    fn for_epoch(&self, epoch: Epoch) -> Result<Box<Self>, StoreError>;

    /// Access the tip of the stable store, corresponding to the latest point that was saved.
    fn tip(&self) -> Result<Point, StoreError>;

    /// Add or remove entries to/from the store. The exact semantic of 'add' and 'remove' depends
    /// on the column type. All updates are atomatic and attached to the given `Point`.
    fn save(
        &'_ self,
        point: &'_ Point,
        issuer: Option<&'_ pools::Key>,
        add: Columns<
            impl Iterator<Item = (utxo::Key, utxo::Value)>,
            impl Iterator<Item = pools::Value>,
            impl Iterator<Item = (accounts::Key, accounts::Value)>,
        >,
        remove: Columns<
            impl Iterator<Item = utxo::Key>,
            impl Iterator<Item = (pools::Key, Epoch)>,
            impl Iterator<Item = accounts::Key>,
        >,
    ) -> Result<(), StoreError>;

    /// The most recent snapshot. Note that we never starts from genesis; so there's always a
    /// snapshot available.
    fn most_recent_snapshot(&self) -> Epoch;

    /// Construct and save on-disk a snapshot of the store. The epoch number is used when
    /// there's no existing snapshot and, to ensure that snapshots are taken in order.
    ///
    /// Idempotent
    ///
    /// /!\ IMPORTANT /!\
    /// It is the **caller's** responsibility to ensure that the snapshot is done at the right
    /// moment. The store has no notion of when is an epoch boundary, and thus deferred that
    /// decision entirely to the caller owning the store.
    fn next_snapshot(
        &mut self,
        epoch: Epoch,
        rewards_summary: Option<RewardsSummary>,
    ) -> Result<(), StoreError>;

    /// Get details about a specific Pool
    fn pool(&self, pool: &PoolId) -> Result<Option<pools::Row>, StoreError>;

    /// Get details about a specific UTxO
    fn resolve_input(
        &self,
        input: &TransactionInput,
    ) -> Result<Option<TransactionOutput>, StoreError>;

    /// Get current values of the treasury and reserves accounts.
    fn with_pots<A>(
        &self,
        with: impl FnMut(Box<dyn BorrowMut<pots::Row> + '_>) -> A,
    ) -> Result<A, StoreError>;

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
}

// Columns
// ----------------------------------------------------------------------------

/// A summary of all database columns, in a single struct. This can be derived to provide updates
/// operations on multiple columns in a single db-transaction.
pub struct Columns<U, P, A> {
    pub utxo: U,
    pub pools: P,
    pub accounts: A,
}

impl<U, P, A> Default for Columns<iter::Empty<U>, iter::Empty<P>, iter::Empty<A>> {
    fn default() -> Self {
        Self {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
        }
    }
}
