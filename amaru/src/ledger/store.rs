pub mod columns;
pub mod rocksdb;

use super::kernel::{Epoch, Point, PoolId, PoolParams, TransactionInput, TransactionOutput};
use columns::pools;
use std::iter;

// Store
// ----------------------------------------------------------------------------

pub trait Store {
    type Error;

    /// Access the tip of the stable store, corresponding to the latest point that was saved.
    fn tip(&self) -> Result<Point, Self::Error>;

    /// Add or remove entries to/from the store. The exact semantic of 'add' and 'remove' depends
    /// on the column type. All updates are atomatic and attached to the given `Point`.
    fn save(
        &'_ self,
        point: &'_ Point,
        add: Columns<
            impl Iterator<Item = (TransactionInput, TransactionOutput)>,
            impl Iterator<Item = (PoolParams, Epoch)>,
        >,
        remove: Columns<
            impl Iterator<Item = TransactionInput>,
            impl Iterator<Item = (PoolId, Epoch)>,
        >,
    ) -> Result<(), Self::Error>;

    /// The most recent snapshot. Note that we never starts from genesis; so there's always a
    /// snapshot available.
    fn most_recent_snapshot(&self) -> Epoch;

    /// Construct and save on-disk a snapshot of the store. The epoch number is used when
    /// there's no existing snapshot and, to ensure that snapshots are taken in order.
    ///
    /// /!\ IMPORTANT /!\
    /// It is the **caller's** responsibility to ensure that the snapshot is done at the right
    /// moment. The store has no notion of when is an epoch boundary, and thus deferred that
    /// decision entirely to the caller owning the store.
    fn next_snapshot(&mut self, epoch: Epoch) -> Result<(), Self::Error>;

    /// Get details about a specific pool
    fn pool(&self, pool: &PoolId) -> Result<Option<pools::Row>, Self::Error>;

    /// Provide an access to iterate on pools, in a way that enforces:
    ///
    /// 1. That mutations will be persisted on-disk
    ///
    /// 2. That all operations are consistent and atomic (the iteration occurs on a snapshot, and
    ///    the mutation apply to the iterated items)
    fn with_pools(&self, with: impl FnMut(pools::Iter<'_, '_>)) -> Result<(), Self::Error>;
}

// Columns
// ----------------------------------------------------------------------------

/// A summary of all database columns, in a single struct. This can be derived to provide updates
/// operations on multiple columns in a single db-transaction.
pub struct Columns<U, P> {
    pub utxo: U,
    pub pools: P,
}

impl<U, P> Default for Columns<iter::Empty<U>, iter::Empty<P>> {
    fn default() -> Self {
        Self {
            utxo: iter::empty(),
            pools: iter::empty(),
        }
    }
}
