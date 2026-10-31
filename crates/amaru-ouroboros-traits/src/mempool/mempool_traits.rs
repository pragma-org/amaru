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

use amaru_kernel::{Point, TransactionId, cbor::WithOriginalBytes};

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

    /// Borrow the current contents of the mempool.
    ///
    /// The read view holds the implementation's read lock, when one is needed, until it is
    /// dropped. Callers must finish using the view before mutating the mempool.
    fn read(&self) -> Box<dyn MempoolRead<Tx> + '_>;

    /// Return true if the mempool contains a transaction with the given id.
    fn contains(&self, tx_id: &TransactionId) -> bool {
        !self.read().get(std::slice::from_ref(tx_id)).is_empty()
    }

    /// Return transaction ids from `from_seq` (inclusive), up to `limit` entries.
    ///
    /// Results are ordered by their insertion sequence number.
    fn slice(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TransactionId, u32, MempoolSeqNo)>;

    /// Remove the identified transactions from the active relay set.
    fn remove(&self, ids: &[TransactionId]);

    /// Return the last sequence number assigned by the mempool.
    ///
    /// The value is monotonic and is not reused when transactions are removed.
    fn last(&self) -> MempoolSeqNo;

    /// Returns `true` if accepting `additional_bytes` would push the
    /// mempool past its configured maximum byte size.
    fn is_near_capacity(&self, additional_bytes: u64) -> bool;

    /// Snapshot of the mempool's tx count and cumulative CBOR size, used
    /// to refresh observability gauges. Returned together so observers can
    /// avoid taking the underlying lock twice.
    fn state(&self) -> MempoolState;

    /// Return the ledger tip against which the active transactions were last revalidated.
    fn tip(&self) -> Point;

    /// Record the ledger tip after the mempool has been revalidated against it.
    fn set_tip(&self, tip: Point);
}

/// Borrowed access to the transactions currently held by a mempool.
pub trait MempoolRead<Tx: Send + Sync + 'static> {
    /// Retrieve the transactions for the given ids without cloning them.
    ///
    /// Results are ordered by insertion sequence, and unknown ids are omitted.
    fn get(&self, ids: &[TransactionId]) -> Vec<&Tx>;

    /// Iterate over all active transactions in insertion order without cloning them.
    fn iter(&self) -> Box<dyn Iterator<Item = &Tx> + '_>;
}

pub type ResourceMempool<Tx> = Arc<dyn Mempool<WithOriginalBytes<Tx>>>;
