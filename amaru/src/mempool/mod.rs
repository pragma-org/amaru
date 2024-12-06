use crate::ledger;
use pallas_primitives::conway::Tx;
use std::collections::BTreeSet;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

// State
// ----------------------------------------------------------------------------

/// The state of the mempool is split into two sub-components:
///
/// - A _ledger_state_ which contains a view of a ledger state, which is used to validate
///   transactions.
///
/// - An _ephemeral_ state, which is maintained as a sequence of diff operations to be applied on
///   top of the _ledger_state_. It should be re-calculated as transactions are added or removed
///   from the mempool.
pub struct State<S, E>
where
    S: ledger::store::Store<Error = E>,
{
    /// The current view of the ledger state.
    ledger_state: ledger::state::State<S, E>,

    /// The ephemeral state, which is applied on top of the ledger_state using transactions
    /// in the mempool.
    ephemeral: ledger::state::VolatileDB,
}

/// A transaction in the mempool with optional _M_ metadata type useful for ordering transactions.
pub trait MempoolTx<M>: Ord + PartialEq {
    /// Any additional _metadata_ associated with the transaction. For example, it could include a timestamp
    /// of when it was received, a reference to the peer that sent it, or some priority value used
    /// to order transactions.
    fn metadata(&self) -> Option<M> {
        None
    }

    /// The transaction itself.
    fn tx(&self) -> &Tx;
}

/// An ordered queue of transactions.
pub type TxQueue<T> = BTreeSet<T>;

/// The mempool is a queue of transactions that are waiting to be validated and applied to an
/// ephemeral ledger state.
pub struct Mempool<T, M>
where
    T: MempoolTx<M>,
{
    /// The queue of transactions. The ordering of transactions are defined by the Ord trait of the
    /// type T and left to the implementer.
    transactions: Arc<RwLock<TxQueue<T>>>,

    _marker: std::marker::PhantomData<M>,
}

impl<T: MempoolTx<M>, M> Default for Mempool<T, M> {
    fn default() -> Self {
        Mempool {
            transactions: Arc::new(RwLock::new(BTreeSet::new())),
            _marker: Default::default(),
        }
    }
}

impl<T: MempoolTx<M>, M> Mempool<T, M> {
    pub fn new() -> Self {
        Mempool::default()
    }

    pub fn insert(&self, tx: T) {
        self.transactions.write().unwrap().insert(tx);
    }

    pub fn len(&self) -> usize {
        self.transactions.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.transactions.read().unwrap().is_empty()
    }

    pub fn iter(&self) -> RwLockReadGuard<'_, BTreeSet<T>> {
        self.transactions.read().unwrap()
    }

    pub fn iter_mut(&self) -> RwLockWriteGuard<'_, BTreeSet<T>> {
        self.transactions.write().unwrap()
    }

    pub fn clear(&self) {
        self.transactions.write().unwrap().clear();
    }

    pub fn insert_all<Iter: IntoIterator<Item = T>>(&self, txs: Iter) {
        self.transactions.write().unwrap().extend(txs);
    }

    pub fn remove(&self, tx: &T) {
        self.transactions.write().unwrap().remove(tx);
    }

    pub fn contains(&self, tx: &T) -> bool {
        self.transactions.read().unwrap().contains(tx)
    }
}
