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

use parking_lot::Mutex;

use crate::{MempoolError, MempoolSeqNo, TxId, TxInsertResult, TxOrigin, TxSubmissionMempool};

/// Optional method overrides for [`OverridingMempool`].
/// Each override receives a reference to the underlying mempool and the method arguments.
/// Overrides are stored in a mutex because they use `FnMut`.
#[allow(clippy::type_complexity)]
struct Overrides<Tx: Send + Sync + 'static> {
    insert: Option<
        Box<dyn FnMut(&dyn TxSubmissionMempool<Tx>, Tx, TxOrigin) -> Result<TxInsertResult, MempoolError> + Send>,
    >,
    get_tx: Option<Box<dyn FnMut(&dyn TxSubmissionMempool<Tx>, &TxId) -> Option<Tx> + Send>>,
    contains: Option<Box<dyn FnMut(&dyn TxSubmissionMempool<Tx>, &TxId) -> bool + Send>>,
    tx_ids_since: Option<
        Box<dyn FnMut(&dyn TxSubmissionMempool<Tx>, MempoolSeqNo, u16) -> Vec<(TxId, u32, MempoolSeqNo)> + Send>,
    >,
    get_txs_for_ids: Option<Box<dyn FnMut(&dyn TxSubmissionMempool<Tx>, &[TxId]) -> Vec<Tx> + Send>>,
    mempool_txs: Option<Box<dyn FnMut(&dyn TxSubmissionMempool<Tx>) -> Vec<Tx> + Send>>,
    remove_txs: Option<Box<dyn FnMut(&dyn TxSubmissionMempool<Tx>, &[TxId]) -> Result<(), MempoolError> + Send>>,
    last_seq_no: Option<Box<dyn FnMut(&dyn TxSubmissionMempool<Tx>) -> MempoolSeqNo + Send>>,
    is_near_capacity: Option<Box<dyn FnMut(&dyn TxSubmissionMempool<Tx>, u64) -> bool + Send>>,
}

impl<Tx: Send + Sync + 'static> Default for Overrides<Tx> {
    fn default() -> Self {
        Self {
            insert: None,
            get_tx: None,
            contains: None,
            tx_ids_since: None,
            get_txs_for_ids: None,
            mempool_txs: None,
            remove_txs: None,
            last_seq_no: None,
            is_near_capacity: None,
        }
    }
}

/// A mempool that wraps a `dyn TxSubmissionMempool<Tx>` and allows overriding any method
/// with a supplied function. When an override is installed, it receives a reference to the
/// underlying mempool, all method arguments, and computes the return value. Non-overridden
/// methods delegate to the underlying mempool.
///
/// Overrides use `FnMut` and are stored in a `parking_lot::Mutex` to allow mutation.
pub struct OverridingMempool<Tx: Send + Sync + 'static> {
    inner: Arc<dyn TxSubmissionMempool<Tx>>,
    overrides: Mutex<Overrides<Tx>>,
}

impl<Tx: Send + Sync + 'static> OverridingMempool<Tx> {
    /// Create a new builder for an overriding mempool wrapping the given mempool.
    pub fn builder(inner: Arc<dyn TxSubmissionMempool<Tx>>) -> OverridingMempoolBuilder<Tx> {
        OverridingMempoolBuilder { inner, overrides: Overrides::default() }
    }
}

/// Builder for [`OverridingMempool`] that accepts override functions via `impl FnMut`.
pub struct OverridingMempoolBuilder<Tx: Send + Sync + 'static> {
    inner: Arc<dyn TxSubmissionMempool<Tx>>,
    overrides: Overrides<Tx>,
}

impl<Tx: Send + Sync + 'static> OverridingMempoolBuilder<Tx> {
    pub fn with_insert<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn TxSubmissionMempool<Tx>, Tx, TxOrigin) -> Result<TxInsertResult, MempoolError> + Send + 'static,
    {
        self.overrides.insert = Some(Box::new(f));
        self
    }

    pub fn with_get_tx<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn TxSubmissionMempool<Tx>, &TxId) -> Option<Tx> + Send + 'static,
    {
        self.overrides.get_tx = Some(Box::new(f));
        self
    }

    pub fn with_contains<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn TxSubmissionMempool<Tx>, &TxId) -> bool + Send + 'static,
    {
        self.overrides.contains = Some(Box::new(f));
        self
    }

    pub fn with_tx_ids_since<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn TxSubmissionMempool<Tx>, MempoolSeqNo, u16) -> Vec<(TxId, u32, MempoolSeqNo)> + Send + 'static,
    {
        self.overrides.tx_ids_since = Some(Box::new(f));
        self
    }

    pub fn with_get_txs_for_ids<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn TxSubmissionMempool<Tx>, &[TxId]) -> Vec<Tx> + Send + 'static,
    {
        self.overrides.get_txs_for_ids = Some(Box::new(f));
        self
    }

    pub fn with_mempool_txs<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn TxSubmissionMempool<Tx>) -> Vec<Tx> + Send + 'static,
    {
        self.overrides.mempool_txs = Some(Box::new(f));
        self
    }

    pub fn with_remove_txs<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn TxSubmissionMempool<Tx>, &[TxId]) -> Result<(), MempoolError> + Send + 'static,
    {
        self.overrides.remove_txs = Some(Box::new(f));
        self
    }

    pub fn with_last_seq_no<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn TxSubmissionMempool<Tx>) -> MempoolSeqNo + Send + 'static,
    {
        self.overrides.last_seq_no = Some(Box::new(f));
        self
    }

    pub fn with_is_near_capacity<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn TxSubmissionMempool<Tx>, u64) -> bool + Send + 'static,
    {
        self.overrides.is_near_capacity = Some(Box::new(f));
        self
    }

    pub fn build(self) -> OverridingMempool<Tx> {
        OverridingMempool { inner: self.inner, overrides: Mutex::new(self.overrides) }
    }
}

impl<Tx: Send + Sync + 'static> TxSubmissionMempool<Tx> for OverridingMempool<Tx> {
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<TxInsertResult, MempoolError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.insert {
            Some(f) => f(self.inner.as_ref(), tx, tx_origin),
            None => self.inner.insert(tx, tx_origin),
        }
    }

    fn get_tx(&self, tx_id: &TxId) -> Option<Tx> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.get_tx {
            Some(f) => f(self.inner.as_ref(), tx_id),
            None => self.inner.get_tx(tx_id),
        }
    }

    fn contains(&self, tx_id: &TxId) -> bool {
        let mut overrides = self.overrides.lock();
        match &mut overrides.contains {
            Some(f) => f(self.inner.as_ref(), tx_id),
            None => self.inner.contains(tx_id),
        }
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.tx_ids_since {
            Some(f) => f(self.inner.as_ref(), from_seq, limit),
            None => self.inner.tx_ids_since(from_seq, limit),
        }
    }

    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Tx> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.get_txs_for_ids {
            Some(f) => f(self.inner.as_ref(), ids),
            None => self.inner.get_txs_for_ids(ids),
        }
    }

    fn mempool_txs(&self) -> Vec<Tx> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.mempool_txs {
            Some(f) => f(self.inner.as_ref()),
            None => self.inner.mempool_txs(),
        }
    }

    fn remove_txs(&self, ids: &[TxId]) -> Result<(), MempoolError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.remove_txs {
            Some(f) => f(self.inner.as_ref(), ids),
            None => self.inner.remove_txs(ids),
        }
    }

    fn last_seq_no(&self) -> MempoolSeqNo {
        let mut overrides = self.overrides.lock();
        match &mut overrides.last_seq_no {
            Some(f) => f(self.inner.as_ref()),
            None => self.inner.last_seq_no(),
        }
    }

    fn is_near_capacity(&self, additional_bytes: u64) -> bool {
        let mut overrides = self.overrides.lock();
        match &mut overrides.is_near_capacity {
            Some(f) => f(self.inner.as_ref(), additional_bytes),
            None => self.inner.is_near_capacity(additional_bytes),
        }
    }
}
