// Copyright 2026 PRAGMA
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

use std::sync::Arc;

use crate::mempool::{MempoolError, MempoolSeqNo, MempoolState, TxId, TxInsertResult, TxOrigin};

/// An simple mempool interface to:
///
/// - Retrieve transactions to be included in a new block.
/// - Acknowledge the transactions included in a block, so they can be removed from the pool.
/// - Support the transaction submission protocol to share transactions with peers.
///
pub trait Mempool<Tx: Send + Sync + 'static>: TxSubmissionMempool<Tx> + Send + Sync {
    /// Take transactions out of the mempool, with the intent of forging a new block.
    ///
    /// TODO: Have this function take _constraints_, such as the block max size or max execution
    /// units and select transactions accordingly.
    fn take(&self) -> Vec<Tx>;

    /// Take note of a transaction that has been included in a block.
    /// The keys function is used to detect all the transactions that should be removed from the mempool.
    /// (if a transaction in the mempool shares any of the transactions keys, it should be removed).
    fn acknowledge<TxKey: Ord, I>(&self, tx: &Tx, keys: fn(&Tx) -> I)
    where
        I: IntoIterator<Item = TxKey>,
        Self: Sized;
}

pub type ResourceMempool<Tx> = Arc<dyn TxSubmissionMempool<Tx>>;

pub trait TxSubmissionMempool<Tx: Send + Sync + 'static>: Send + Sync {
    /// Insert a transaction into the mempool, specifying its origin.
    /// A TxOrigin::Local origin indicates the transaction was created on the current node,
    /// A TxOrigin::Remote(origin_peer) indicates the transaction was received from a remote peer.
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<TxInsertResult, MempoolError>;

    /// Retrieve a transaction by its id.
    fn get_tx(&self, tx_id: &TxId) -> Option<Tx>;

    /// Return true if the mempool contains a transaction with the given id.
    fn contains(&self, tx_id: &TxId) -> bool {
        self.get_tx(tx_id).is_some()
    }

    /// Retrieve a list of transaction ids from a given sequence number (inclusive), up to a given limit.
    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)>;

    /// Retrieve a list of transactions for the given ids.
    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Tx>;

    /// Retrieve all transactions currently active for relay.
    fn mempool_txs(&self) -> Vec<Tx>;

    /// Remove transactions from the active relay set.
    fn remove_txs(&self, _ids: &[TxId]) -> Result<(), MempoolError>;

    /// Get the last assigned sequence number in the mempool.
    fn last_seq_no(&self) -> MempoolSeqNo;

    /// Returns `true` if accepting `additional_bytes` would push the
    /// mempool past its configured maximum byte size.
    fn is_near_capacity(&self, additional_bytes: u64) -> bool;

    /// Snapshot of the mempool's tx count and cumulative CBOR size, used
    /// to refresh observability gauges. Returned together so observers can
    /// avoid taking the underlying lock twice.
    fn state(&self) -> MempoolState;
}
