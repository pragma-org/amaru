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

use amaru_kernel::Tx;
use amaru_kernel::tx_submission_events::TxId;
use amaru_ouroboros_traits::{MempoolSeqNo, TxOrigin, TxRejectReason, TxSubmissionMempool};
use pure_stage::{
    BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, ExternalEffectSync, Resources, SendData,
};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

/// Implementation of Mempool using pure_stage::Effects.
#[derive(Clone)]
pub struct MemoryPool<T> {
    effects: Effects<T>,
}

impl<T> MemoryPool<T> {
    pub fn new(effects: Effects<T>) -> MemoryPool<T> {
        MemoryPool { effects }
    }

    /// This function runs an external effect synchronously.
    pub fn external_sync<E: ExternalEffectSync + serde::Serialize + 'static>(
        &self,
        effect: E,
    ) -> E::Response
    where
        T: SendData + Sync,
    {
        self.effects.external_sync(effect)
    }
}

impl<T: SendData + Sync> TxSubmissionMempool<Tx> for MemoryPool<T> {
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
        self.external_sync(Insert::new(tx, tx_origin))
    }

    fn get_tx(&self, tx_id: &TxId) -> Option<Arc<Tx>> {
        self.external_sync(GetTx::new(tx_id.clone()))
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
        self.external_sync(TxIdsSince::new(from_seq, limit))
    }

    fn wait_for_at_least(
        &self,
        seq_no: MempoolSeqNo,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        self.effects.external(WaitForAtLeast::new(seq_no))
    }

    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Arc<Tx>> {
        self.external_sync(GetTxsForIds::new(ids))
    }

    fn last_seq_no(&self) -> MempoolSeqNo {
        self.external_sync(LastSeqNo)
    }
}

// EXTERNAL EFFECTS DEFINITIONS

pub type ResourceMempool = Arc<dyn TxSubmissionMempool<Tx>>;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Insert {
    tx: Tx,
    tx_origin: TxOrigin,
}

impl Insert {
    pub fn new(tx: Tx, tx_origin: TxOrigin) -> Self {
        Self { tx, tx_origin }
    }
}

impl ExternalEffect for Insert {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources
                .get::<ResourceMempool>()
                .expect("ResourceMempool requires a mempool")
                .clone();
            mempool.insert(self.tx, self.tx_origin)
        })
    }
}

impl ExternalEffectAPI for Insert {
    type Response = Result<(TxId, MempoolSeqNo), TxRejectReason>;
}

impl ExternalEffectSync for Insert {}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct GetTx {
    tx_id: TxId,
}

impl GetTx {
    pub fn new(tx_id: TxId) -> Self {
        Self { tx_id }
    }
}

impl ExternalEffect for GetTx {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources
                .get::<ResourceMempool>()
                .expect("ResourceMempool requires a mempool")
                .clone();
            mempool.get_tx(&self.tx_id)
        })
    }
}

impl ExternalEffectAPI for GetTx {
    type Response = Option<Arc<Tx>>;
}

impl ExternalEffectSync for GetTx {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct TxIdsSince {
    mempool_seqno: MempoolSeqNo,
    limit: u16,
}

impl TxIdsSince {
    pub fn new(mempool_seqno: MempoolSeqNo, limit: u16) -> Self {
        Self {
            mempool_seqno,
            limit,
        }
    }
}

impl ExternalEffect for TxIdsSince {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources
                .get::<ResourceMempool>()
                .expect("ResourceMempool requires a mempool")
                .clone();
            mempool.tx_ids_since(self.mempool_seqno, self.limit)
        })
    }
}

impl ExternalEffectAPI for TxIdsSince {
    type Response = Vec<(TxId, u32, MempoolSeqNo)>;
}

impl ExternalEffectSync for TxIdsSince {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct WaitForAtLeast {
    seq_no: MempoolSeqNo,
}

impl WaitForAtLeast {
    pub fn new(seq_no: MempoolSeqNo) -> Self {
        Self { seq_no }
    }
}

impl ExternalEffect for WaitForAtLeast {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            let mempool = resources
                .get::<ResourceMempool>()
                .expect("ResourceMempool requires a mempool")
                .clone();
            mempool.wait_for_at_least(self.seq_no).await
        })
    }
}

impl ExternalEffectAPI for WaitForAtLeast {
    type Response = bool;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct GetTxsForIds {
    tx_ids: Vec<TxId>,
}

impl GetTxsForIds {
    pub fn new(ids: &[TxId]) -> Self {
        Self {
            tx_ids: ids.to_vec(),
        }
    }
}

impl ExternalEffect for GetTxsForIds {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources
                .get::<ResourceMempool>()
                .expect("ResourceMempool requires a mempool")
                .clone();
            mempool.get_txs_for_ids(&self.tx_ids)
        })
    }
}

impl ExternalEffectAPI for GetTxsForIds {
    type Response = Vec<Arc<Tx>>;
}

impl ExternalEffectSync for GetTxsForIds {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct LastSeqNo;

impl ExternalEffect for LastSeqNo {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources
                .get::<ResourceMempool>()
                .expect("ResourceMempool requires a mempool")
                .clone();
            mempool.last_seq_no()
        })
    }
}

impl ExternalEffectAPI for LastSeqNo {
    type Response = MempoolSeqNo;
}

impl ExternalEffectSync for LastSeqNo {}
