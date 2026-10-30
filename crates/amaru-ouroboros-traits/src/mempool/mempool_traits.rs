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

use amaru_kernel::{TransactionId, cbor::WithOriginalBytes};

use crate::mempool::{MempoolSeqNo, MempoolState, TxInsertResult, TxOrigin};

/// A mempool interface to:
///
/// - Retrieve transactions to be included in a new block.
/// - Support the transaction submission protocol to share transactions with peers.
///
pub trait Mempool<Tx: Send + Sync + 'static>: Send + Sync {
    /// Insert a transaction into the mempool, specifying its origin.
    /// A TxOrigin::Local origin indicates the transaction was created on the current node,
    /// A TxOrigin::Remote(origin_peer) indicates the transaction was received from a remote peer.
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> TxInsertResult;

    /// Retrieve a transaction by its id.
    fn get_tx(&self, tx_id: &TransactionId) -> Option<Tx>;

    /// Return true if the mempool contains a transaction with the given id.
    fn contains(&self, tx_id: &TransactionId) -> bool {
        self.get_tx(tx_id).is_some()
    }

    /// Retrieve a list of transaction ids from a given sequence number (inclusive), up to a given limit.
    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TransactionId, u32, MempoolSeqNo)>;

    /// Retrieve a list of transactions for the given ids.
    fn get_txs_for_ids(&self, ids: &[TransactionId]) -> Vec<Tx>;

    /// Retrieve all transactions currently active for relay.
    fn mempool_txs(&self) -> Vec<Tx>;

    /// Remove transactions from the active relay set.
    fn remove_txs(&self, _ids: &[TransactionId]);

    /// Get the last assigned sequence number in the mempool.
    fn last_seq_no(&self) -> MempoolSeqNo;

    /// Returns `true` if accepting `additional_bytes` would push the
    /// mempool past its configured maximum byte size.
    fn is_near_capacity(&self, additional_bytes: u64) -> bool;

    /// Snapshot of the mempool's tx count and cumulative CBOR size, used
    /// to refresh observability gauges. Returned together so observers can
    /// avoid taking the underlying lock twice.
    fn state(&self) -> MempoolState;

    /// Take transactions out of the mempool, with the intent of forging a new block.
    ///
    /// TODO: Have this function take _constraints_, such as the block max size or max execution
    /// units and select transactions accordingly.
    fn take(&self) -> Vec<Tx> {
        Vec::new()
    }
}

pub type ResourceMempool<Tx> = Arc<dyn Mempool<WithOriginalBytes<Tx>>>;
