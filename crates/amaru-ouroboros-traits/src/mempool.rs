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

use amaru_kernel::cbor::Encode;
use amaru_kernel::peer::Peer;
use amaru_kernel::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;

/// An simple mempool interface to:
///
/// - Add transactions and forge blocks when needed.
pub trait Mempool<Tx: Send + Sync + 'static>: Send + Sync {
    /// Add a new, local, transaction to the mempool.
    ///
    /// TODO: Have the mempool perform its own set of validations and possibly fail to add new
    /// elements. This is non-trivial, since it requires the mempool to have ways of re-validating
    /// transactions provided a slightly different context.
    ///
    /// We shall circle back to this once we've done some progress on the ledger validations and
    /// the so-called ledger slices.
    ///
    /// Return the assigned `MempoolSeqNo` if accepted.
    fn add(&self, tx: Tx) -> Result<(), TxRejectReason> {
        let _ = self.insert(tx, TxOrigin::Local)?;
        Ok(())
    }

    /// Insert a transaction into the mempool, specifying its origin.
    /// A TxOrigin::Local origin indicates the transaction was created on the current node,
    /// A TxOrigin::Remote(origin_peer) indicates the transaction was received from a remote peer.
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<(TxId, MempoolSeqNo), TxRejectReason>;

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

    /// Retrieve a transaction by its id.
    fn get_tx(&self, tx_id: &TxId) -> Option<Arc<Tx>>;

    fn contains(&self, tx_id: &TxId) -> bool {
        self.get_tx(tx_id).is_some()
    }

    /// Retrieve a list of transactions from a given sequence number (inclusive), up to a given limit.
    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)>;

    /// Wait until the mempool has at least the given number of _valid_ transactions.
    /// Those transactions will be sent to an upstream peer.
    ///
    /// When the mempool already has at least the required number of transactions,
    /// this future will resolve to `true` immediately.
    ///
    /// Otherwise, if for some reason the mempool cannot reach the required number, it should return
    /// false.
    fn wait_for_at_least(&self, required: u16) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;

    /// Retrieve a list of transactions for the given ids.
    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Arc<Tx>>;
}

/// Identifier for a transaction in the mempool.
/// It is derived from the hash of the encoding of the transaction as CBOR.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TxId(Hash<32>);

impl TxId {
    pub fn to_vec(&self) -> Vec<u8> {
        self.0.as_ref().to_vec()
    }

    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl TxId {
    pub fn new(hash: Hash<32>) -> Self {
        TxId(hash)
    }

    pub fn from<Tx: Encode<()>>(tx: Tx) -> Self {
        TxId(Hasher::<{ 32 * 8 }>::hash_cbor(&tx))
    }
}

/// Sequence number assigned to a transaction when inserted into the mempool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MempoolSeqNo(pub u64);

impl MempoolSeqNo {
    pub fn next(&self) -> MempoolSeqNo {
        MempoolSeqNo(self.0 + 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, thiserror::Error)]
pub enum TxRejectReason {
    #[error("Mempool is full")]
    MempoolFull,
    #[error("Transaction is a duplicate")]
    Duplicate,
    #[error("Transaction is invalid")]
    Invalid,
}

/// Origin of a transaction being inserted into the mempool:
/// - Local: created locally
/// - Remote(Peer): received from a remote peer
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TxOrigin {
    Local,
    Remote(Peer),
}
