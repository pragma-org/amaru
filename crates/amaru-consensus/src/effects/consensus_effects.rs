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

use crate::effects::{
    Base, BaseOps, Ledger, LedgerOps, Network, NetworkOps, Store,
    metrics_effects::{Metrics, MetricsOps},
};
use amaru_kernel::{BlockHeader, Transaction};
use amaru_ouroboros_traits::{ChainStore, TxSubmissionMempool};
use amaru_protocols::mempool_effects::MemoryPool;
use pure_stage::{Effects, SendData};
use std::sync::Arc;

/// This trait provides access to all effectful operations needed by consensus stages.
pub trait ConsensusOps: Send + Sync + Clone {
    /// Return a ChainStore implementation to store headers, get the best chain tip etc...
    fn store(&self) -> Arc<dyn ChainStore<BlockHeader>>;
    /// Return a NetworkOps implementation to access network operations, like fetch_block
    fn network(&self) -> impl NetworkOps;
    /// Return a TxSubmissionMempool implementation to access mempool operations, like get_tx to retrieve a transaction
    /// from the mempool.
    fn mempool(&self) -> Arc<dyn TxSubmissionMempool<Transaction>>;
    /// Return a LedgerOps implementation to access ledger operations, considering that it is a sub-system
    /// external to consensus.
    fn ledger(&self) -> Arc<dyn LedgerOps>;
    /// Return a BaseOps implementation to access basic operations, like sending messages to other stages.
    fn base(&self) -> impl BaseOps;
    /// Return a MetricsOps implementation to record metrics events.
    fn metrics(&self) -> impl MetricsOps;
}

/// Implementation of ConsensusOps using pure_stage::Effects.
#[derive(Clone)]
pub struct ConsensusEffects<T> {
    effects: Effects<T>,
}

impl<T: SendData + Sync + Clone> ConsensusEffects<T> {
    pub fn new(effects: Effects<T>) -> ConsensusEffects<T> {
        ConsensusEffects { effects }
    }

    pub fn store(&self) -> Arc<dyn ChainStore<BlockHeader>> {
        Arc::new(Store::new(self.effects.clone()))
    }

    pub fn mempool(&self) -> Arc<dyn TxSubmissionMempool<Transaction>> {
        Arc::new(MemoryPool::new(self.effects.clone()))
    }

    pub fn network(&self) -> impl NetworkOps {
        Network::new(&self.effects)
    }

    pub fn ledger(&self) -> Arc<dyn LedgerOps> {
        Arc::new(Ledger::new(self.effects.clone()))
    }

    pub fn base(&self) -> impl BaseOps {
        Base::new(&self.effects)
    }

    pub fn metrics(&self) -> impl MetricsOps {
        Metrics::new(&self.effects)
    }
}

impl<T: SendData + Sync + Clone> ConsensusOps for ConsensusEffects<T> {
    fn store(&self) -> Arc<dyn ChainStore<BlockHeader>> {
        self.store()
    }

    fn mempool(&self) -> Arc<dyn TxSubmissionMempool<Transaction>> {
        self.mempool()
    }

    fn network(&self) -> impl NetworkOps {
        self.network()
    }

    fn ledger(&self) -> Arc<dyn LedgerOps> {
        self.ledger()
    }

    fn base(&self) -> impl BaseOps {
        self.base()
    }

    fn metrics(&self) -> impl MetricsOps {
        self.metrics()
    }
}

/// This module provides mock implementations of ConsensusOps and its sub-traits for unit testing.
#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::errors::ProcessingFailed;
    use amaru_kernel::{Block, Peer, Point, PoolId, Tip};
    use amaru_mempool::strategies::InMemoryMempool;
    use amaru_metrics::{MetricsEvent, ledger::LedgerMetrics};
    use amaru_ouroboros::has_stake_distribution::GetPoolError;
    use amaru_ouroboros_traits::{
        BlockValidationError, HasStakeDistribution, PoolSummary, TxSubmissionMempool,
        can_validate_blocks::HeaderValidationError, in_memory_consensus_store::InMemConsensusStore,
    };
    use amaru_protocols::blockfetch::Blocks;
    use amaru_slot_arithmetic::Slot;
    use parking_lot::Mutex;
    use pure_stage::{
        BoxFuture, Instant, StageRef,
        serde::{from_cbor, to_cbor},
    };
    use serde::de::DeserializeOwned;
    use std::{collections::BTreeMap, future::ready, sync::Arc, time::Duration};

    #[derive(Clone)]
    pub struct MockConsensusOps {
        pub mock_store: InMemConsensusStore<BlockHeader>,
        pub mock_mempool: Arc<dyn TxSubmissionMempool<Transaction>>,
        pub mock_network: MockNetworkOps,
        pub mock_ledger: MockLedgerOps,
        pub mock_base: MockBaseOps,
        pub mock_metrics: MockMetricsOps,
    }

    #[allow(refining_impl_trait)]
    impl ConsensusOps for MockConsensusOps {
        fn store(&self) -> Arc<dyn ChainStore<BlockHeader>> {
            Arc::new(self.mock_store.clone())
        }

        fn network(&self) -> impl NetworkOps {
            self.mock_network.clone()
        }

        fn mempool(&self) -> Arc<dyn TxSubmissionMempool<Transaction>> {
            self.mock_mempool.clone()
        }

        fn ledger(&self) -> Arc<dyn LedgerOps> {
            Arc::new(self.mock_ledger.clone())
        }

        fn base(&self) -> impl BaseOps {
            self.mock_base.clone()
        }

        fn metrics(&self) -> impl MetricsOps {
            self.mock_metrics.clone()
        }
    }

    #[derive(Clone, Default)]
    pub struct MockNetworkOps;

    impl NetworkOps for MockNetworkOps {
        fn send_forward_event(
            &self,
            _peer: Peer,
            _header: BlockHeader,
        ) -> BoxFuture<'_, Result<(), ProcessingFailed>> {
            Box::pin(ready(Ok(())))
        }

        fn send_backward_event(
            &self,
            _peer: Peer,
            _header_tip: Tip,
        ) -> BoxFuture<'_, Result<(), ProcessingFailed>> {
            Box::pin(ready(Ok(())))
        }
    }

    #[derive(Default, Clone)]
    pub struct MockLedgerOps;

    impl LedgerOps for MockLedgerOps {
        fn validate_header(
            &self,
            _header: &BlockHeader,
            _ctx: opentelemetry::Context,
        ) -> BoxFuture<'_, Result<(), HeaderValidationError>> {
            Box::pin(async { Ok(()) })
        }

        fn validate_block(
            &self,
            _peer: &Peer,
            _point: &Point,
            _block: Block,
            _ctx: opentelemetry::Context,
        ) -> BoxFuture<'_, Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>>
        {
            Box::pin(async { Ok(Ok(LedgerMetrics::default())) })
        }

        fn rollback(
            &self,
            _peer: &Peer,
            _point: &Point,
            _ctx: opentelemetry::Context,
        ) -> BoxFuture<'static, anyhow::Result<(), ProcessingFailed>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl HasStakeDistribution for MockLedgerOps {
        fn get_pool(
            &self,
            _slot: Slot,
            _pool: &PoolId,
        ) -> Result<Option<PoolSummary>, GetPoolError> {
            Ok(None)
        }
    }

    #[derive(Clone)]
    pub struct MockBaseOps {
        messages: Arc<Mutex<BTreeMap<String, Vec<String>>>>,
        call_data: Arc<Mutex<Option<Vec<u8>>>>,
    }

    impl Default for MockBaseOps {
        fn default() -> Self {
            MockBaseOps {
                messages: Arc::new(Mutex::new(BTreeMap::new())),
                call_data: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl MockBaseOps {
        pub fn received(&self) -> BTreeMap<String, Vec<String>> {
            self.messages.lock().clone()
        }

        pub fn return_blocks(&self, blocks: Blocks) -> &Self {
            let mut call_data = self.call_data.lock();
            *call_data = Some(to_cbor(&blocks));
            self
        }
    }

    impl BaseOps for MockBaseOps {
        fn send<Msg: SendData + 'static>(
            &self,
            target: &StageRef<Msg>,
            msg: Msg,
        ) -> BoxFuture<'static, ()> {
            let mut messages = self.messages.lock();
            messages
                .entry(target.name().to_string())
                .or_default()
                .push(format!("{msg:?}"));
            Box::pin(async move {})
        }

        fn call<Req: SendData, Resp: SendData + DeserializeOwned>(
            &self,
            _target: &StageRef<Req>,
            _timeout: Duration,
            _msg: impl FnOnce(StageRef<Resp>) -> Req + Send + 'static,
        ) -> BoxFuture<'_, Option<Resp>> {
            Box::pin(async {
                // Clone the bytes out of the mutex guard so we don't hold the lock
                // while deserializing, and to avoid moving out of the guard.
                let data = self.call_data.lock().clone();
                data.map(|bytes| from_cbor(&bytes).expect("Failed to cast call data"))
            })
        }

        fn clock(&self) -> BoxFuture<'static, Instant> {
            Box::pin(async { Instant::at_offset(Duration::from_millis(0)) })
        }

        fn wait(&self, duration: Duration) -> BoxFuture<'static, Instant> {
            Box::pin(async move { Instant::at_offset(duration) })
        }

        fn terminate(&self) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }
    }

    impl BaseOps for &MockBaseOps {
        fn send<Msg: SendData + 'static>(
            &self,
            target: &StageRef<Msg>,
            msg: Msg,
        ) -> BoxFuture<'static, ()> {
            let mut messages = self.messages.lock();
            messages
                .entry(target.name().to_string())
                .or_default()
                .push(format!("{msg:?}"));
            Box::pin(async move {})
        }

        fn call<Req: SendData, Resp: SendData + DeserializeOwned>(
            &self,
            _target: &StageRef<Req>,
            _timeout: Duration,
            _msg: impl FnOnce(StageRef<Resp>) -> Req + Send + 'static,
        ) -> BoxFuture<'static, Option<Resp>> {
            Box::pin(async { None })
        }

        fn clock(&self) -> BoxFuture<'static, Instant> {
            Box::pin(async { Instant::at_offset(Duration::from_millis(0)) })
        }

        fn wait(&self, duration: Duration) -> BoxFuture<'static, Instant> {
            Box::pin(async move { Instant::at_offset(duration) })
        }

        fn terminate(&self) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }
    }

    #[derive(Default, Clone)]
    pub struct MockMetricsOps;

    impl MetricsOps for MockMetricsOps {
        fn record(&self, _event: MetricsEvent) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }
    }

    impl MetricsOps for &MockMetricsOps {
        fn record(&self, _event: MetricsEvent) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }
    }

    pub fn mock_consensus_ops() -> MockConsensusOps {
        MockConsensusOps {
            mock_store: InMemConsensusStore::new(),
            mock_mempool: Arc::new(InMemoryMempool::default()),
            mock_network: MockNetworkOps,
            mock_ledger: MockLedgerOps,
            mock_base: MockBaseOps::default(),
            mock_metrics: MockMetricsOps,
        }
    }
}
