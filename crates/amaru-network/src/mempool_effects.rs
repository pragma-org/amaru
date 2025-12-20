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
use amaru_ouroboros_traits::{
    CanValidateTransactions, MempoolSeqNo, TransactionValidationError, TxId, TxOrigin,
    TxRejectReason, TxSubmissionMempool,
};
use pure_stage::{
    BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, ExternalEffectSync, Resources, SendData,
};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

/// Implementation of Mempool effects using pure_stage::Effects.
///
/// It supports operations
///
/// - for the tx submission protocol
/// - for transaction validation
///
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

impl<T: SendData + Sync> CanValidateTransactions<Tx> for MemoryPool<T> {
    /// This effect uses the ledger to validate a transaction before adding it to the mempool.
    fn validate_transaction(&self, tx: Tx) -> Result<(), TransactionValidationError> {
        self.effects.external_sync(ValidateTransaction(tx))
    }
}

impl<T: SendData + Sync> TxSubmissionMempool<Tx> for MemoryPool<T> {
    /// This effect inserts a transaction into the mempool, specifying its origin.
    /// A TxOrigin::Local origin indicates the transaction was created on the current node,
    /// A TxOrigin::Remote(origin_peer) indicates the transaction was received from a remote peer
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
        self.external_sync(Insert::new(tx, tx_origin))
    }

    /// This effect retrieves a transaction by its id.
    /// It returns None if the transaction is not found.
    fn get_tx(&self, tx_id: &TxId) -> Option<Tx> {
        self.external_sync(GetTx::new(*tx_id))
    }

    /// This effect retrieves a list of transaction ids from a given sequence number (inclusive), up to a given limit.
    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
        self.external_sync(TxIdsSince::new(from_seq, limit))
    }

    /// This effect waits until the mempool reaches at least the given sequence number.
    fn wait_for_at_least(
        &self,
        seq_no: MempoolSeqNo,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        self.effects.external(WaitForAtLeast::new(seq_no))
    }

    /// This effect retrieves a list of transactions for the given ids.
    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Tx> {
        self.external_sync(GetTxsForIds::new(ids))
    }

    /// This effect gets the last assigned sequence number in the mempool.
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
                .expect("ResourceMempool requires a mempool");
            mempool.insert(self.tx, self.tx_origin)
        })
    }
}

impl ExternalEffectAPI for Insert {
    type Response = Result<(TxId, MempoolSeqNo), TxRejectReason>;
}

impl ExternalEffectSync for Insert {}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ValidateTransaction(Tx);

impl ExternalEffect for ValidateTransaction {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources
                .get::<ResourceMempool>()
                .expect("ResourceMempool requires a mempool");
            mempool.validate_transaction(self.0)
        })
    }
}

impl ExternalEffectAPI for ValidateTransaction {
    type Response = Result<(), TransactionValidationError>;
}

impl ExternalEffectSync for ValidateTransaction {}

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
                .expect("ResourceMempool requires a mempool");
            mempool.get_tx(&self.tx_id)
        })
    }
}

impl ExternalEffectAPI for GetTx {
    type Response = Option<Tx>;
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
                .expect("ResourceMempool requires a mempool");
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
                .expect("ResourceMempool requires a mempool");
            mempool.get_txs_for_ids(&self.tx_ids)
        })
    }
}

impl ExternalEffectAPI for GetTxsForIds {
    type Response = Vec<Tx>;
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
                .expect("ResourceMempool requires a mempool");
            mempool.last_seq_no()
        })
    }
}

impl ExternalEffectAPI for LastSeqNo {
    type Response = MempoolSeqNo;
}

impl ExternalEffectSync for LastSeqNo {}

#[cfg(test)]
mod tests {
    use amaru_kernel::{Nullable, Tx};
    use amaru_ouroboros_traits::{
        CanValidateTransactions, MempoolSeqNo, TransactionValidationError, TxId, TxOrigin,
        TxRejectReason, TxSubmissionMempool,
    };
    use pallas_primitives::Set;
    use pallas_primitives::conway::{PseudoTransactionBody, PseudoTx, WitnessSet};
    use std::pin::Pin;

    #[allow(dead_code)]
    pub struct ConstantMempool {
        tx: Tx,
    }

    impl ConstantMempool {
        #[allow(dead_code)]
        pub fn new() -> Self {
            let transaction_body = PseudoTransactionBody {
                inputs: Set::from(vec![]),
                outputs: vec![],
                fee: 0,
                ttl: None,
                certificates: None,
                withdrawals: None,
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: None,
                script_data_hash: None,
                required_signers: None,
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: None,
                voting_procedures: None,
                proposal_procedures: None,
                treasury_value: None,
                collateral: None,
                donation: None,
            };
            let transaction_witness_set = WitnessSet {
                vkeywitness: None,
                native_script: None,
                bootstrap_witness: None,
                plutus_v1_script: None,
                plutus_data: None,
                redeemer: None,
                plutus_v2_script: None,
                plutus_v3_script: None,
            };
            let tx: Tx = PseudoTx {
                transaction_body,
                transaction_witness_set,
                success: true,
                auxiliary_data: Nullable::Null,
            };
            Self { tx }
        }
    }

    impl TxSubmissionMempool<Tx> for ConstantMempool {
        fn insert(
            &self,
            tx: Tx,
            _tx_origin: TxOrigin,
        ) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
            Ok((TxId::from(&tx), MempoolSeqNo(1)))
        }

        fn get_tx(&self, _tx_id: &TxId) -> Option<Tx> {
            Some(self.tx.clone())
        }

        fn tx_ids_since(
            &self,
            _from_seq: MempoolSeqNo,
            _limit: u16,
        ) -> Vec<(TxId, u32, MempoolSeqNo)> {
            vec![(TxId::from(&self.tx), 100, MempoolSeqNo(1))]
        }

        fn wait_for_at_least(
            &self,
            _seq_no: MempoolSeqNo,
        ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
            Box::pin(async { true })
        }

        fn get_txs_for_ids(&self, _ids: &[TxId]) -> Vec<Tx> {
            vec![self.tx.clone()]
        }

        fn last_seq_no(&self) -> MempoolSeqNo {
            MempoolSeqNo(1)
        }
    }

    impl CanValidateTransactions<Tx> for ConstantMempool {
        fn validate_transaction(&self, _tx: Tx) -> Result<(), TransactionValidationError> {
            Ok(())
        }
    }
}
