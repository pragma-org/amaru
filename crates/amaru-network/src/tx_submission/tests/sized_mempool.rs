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

use amaru_mempool::strategies::InMemoryMempool;
use amaru_mempool::{DefaultCanValidateTransactions, MempoolConfig};
use amaru_ouroboros_traits::{
    CanValidateTransactions, Mempool, MempoolSeqNo, TransactionValidationError, TxId, TxOrigin,
    TxRejectReason, TxSubmissionMempool,
};
use pallas_primitives::conway::Tx;
use std::pin::Pin;
use std::sync::Arc;

/// A mempool wrapper that limits the effective capacity of the inner mempool.
/// When the last sequence number requested is beyond the capacity, it returns `false` on
/// `wait_for_at_least` in order for the client to:
///
/// - stop blocking on the server waiting for transaction ids.
/// - return Done.
///
pub struct SizedMempool {
    capacity: u64,
    inner_mempool: Arc<InMemoryMempool<Tx>>,
}

impl SizedMempool {
    pub fn new(capacity: u64, inner_mempool: Arc<InMemoryMempool<Tx>>) -> Self {
        SizedMempool {
            capacity,
            inner_mempool,
        }
    }

    pub fn with_capacity(capacity: u64) -> Self {
        SizedMempool::with_tx_validator(capacity, Arc::new(DefaultCanValidateTransactions))
    }

    pub fn with_tx_validator(
        capacity: u64,
        tx_validator: Arc<dyn CanValidateTransactions<Tx>>,
    ) -> Self {
        SizedMempool::new(
            capacity,
            Arc::new(InMemoryMempool::new(MempoolConfig::default(), tx_validator)),
        )
    }
}

impl CanValidateTransactions<Tx> for SizedMempool {
    fn validate_transaction(&self, tx: Tx) -> Result<(), TransactionValidationError> {
        self.inner_mempool.validate_transaction(tx)
    }
}

impl TxSubmissionMempool<Tx> for SizedMempool {
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
        self.inner_mempool.insert(tx, tx_origin)
    }

    fn get_tx(&self, tx_id: &TxId) -> Option<Tx> {
        self.inner_mempool.get_tx(tx_id)
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
        self.inner_mempool.tx_ids_since(from_seq, limit)
    }

    fn wait_for_at_least(
        &self,
        seq_no: MempoolSeqNo,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        // Return false if we are beyond capacity
        // otherwise delegate to inner mempool.
        if seq_no > MempoolSeqNo(self.capacity) {
            Box::pin(async move { false })
        } else {
            self.inner_mempool.wait_for_at_least(seq_no)
        }
    }

    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Tx> {
        self.inner_mempool.get_txs_for_ids(ids)
    }

    fn last_seq_no(&self) -> MempoolSeqNo {
        self.inner_mempool.last_seq_no()
    }
}

impl Mempool<Tx> for SizedMempool {
    fn take(&self) -> Vec<Tx> {
        self.inner_mempool.take()
    }

    fn acknowledge<TxKey: Ord, I>(&self, tx: &Tx, keys: fn(&Tx) -> I)
    where
        I: IntoIterator<Item = TxKey>,
        Self: Sized,
    {
        self.inner_mempool.acknowledge(tx, keys)
    }
}
