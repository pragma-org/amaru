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

use std::sync::Arc;
use amaru_kernel::{Hash, Hasher, HEADER_HASH_SIZE};
use amaru_kernel::cbor::Encode;
use crate::strategies::{MempoolSeqNo, TxOrigin, TxRejectReason};

/// An simple mempool interface to add transactions and forge blocks when needed.
pub trait Mempool<Tx: Send + Sync + 'static>: Send + Sync
where
{
    /// Add a new transaction to the mempool.
    ///
    /// TODO: Have the mempool perform its own set of validations and possibly fail to add new
    /// elements. This is non-trivial, since it requires the mempool to have ways of re-validating
    /// transactions provided a slightly different context.
    ///
    /// We shall circle back to this once we've done some progress on the ledger validations and
    /// the so-called ledger slices.
    ///
    ///  originating from this node (wallet, API).
    ///
    ///  Should perform full validation using the ledger (phase-1 & phase-2),
    ///  apply mempool rules (TTL, max size, min fee, etc.), and
    ///  return the assigned `MempoolSeqNo` if accepted.
    /// Should perform full validation using the ledger (phase-1 & phase-2),
    /// apply mempool rules (TTL, max size, min fee, etc.), and
    /// return the assigned `MempoolSeqNo` if accepted.
    fn add(&self, tx: Tx) -> Result<(), TxRejectReason> {
        let _ = self.insert(tx, TxOrigin::Local)?;
        Ok(())
    }

    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<(TxId, MempoolSeqNo), TxRejectReason>;

    /// Take transactions out of the mempool, with the intent of forging a new block.
    ///
    /// TODO: Have this function take _constraints_, such as the block max size or max execution
    /// units and select transactions accordingly.
    fn take(&self) -> Vec<Tx>;

    /// Take note of a transaction happening outside of the mempool. This should in principle
    /// invalidate transactions within the mempool that are now considered invalid.
    fn acknowledge<TxKey: Ord, I>(&self, tx: &Tx, keys: fn(&Tx) -> I)
    where
        I: IntoIterator<Item=TxKey>;

    /// Retrieve a transaction by its id.
    fn get_tx(&self, tx_id: &TxId) -> Option<Arc<Tx>>;

    /// Retrieve a list of transactions from a given sequence number, up to a given limit.
    fn tx_ids_since(
        &self,
        from_seq: MempoolSeqNo,
        limit: usize,
    ) -> Vec<(TxId, MempoolSeqNo)>;

    /// Retrieve a list of transactions for the given ids.
    fn get_txs_for_ids(
        &self,
        ids: &[TxId],
    ) -> Vec<Arc<Tx>>;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TxId(Hash<32>);

impl TxId {
    pub fn new(hash: Hash<32>) -> Self {
        TxId(hash)
    }

    pub fn from<Tx: Encode<()>>(tx: Tx) -> Self {
        let hash = Hasher::<{ HEADER_HASH_SIZE * 8 }>::hash_cbor(&tx);
        TxId(hash)
    }
}
