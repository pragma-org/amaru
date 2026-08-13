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

use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use amaru_kernel::{Block, Point, Tip, Transaction};
use amaru_ledger::{
    rules::block::BlockValidation,
    state::State,
    store::{HistoricalStores, Store},
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros_traits::{
    BaseReadChainStore, CanValidateBlocks, CanValidateTxs, ChainStore, ForkSwitchOutcome, HasStakePools, PoolSummaries,
    ReadChainStore, TransactionValidationError, can_validate_blocks::BlockValidationError,
};
use amaru_plutus::arena_pool::ArenaPool;
use anyhow::anyhow;

/// This data type encapsulate the ledger state in order to implement the `CanValidateBlocks` trait.
/// and be able to validate blocks (including rollback).
#[derive(Clone)]
pub struct BlockValidator<S: Store, HS: HistoricalStores> {
    state: Arc<Mutex<State<S, HS>>>,
    vm_eval_pool: ArenaPool,
    chain_store: Arc<dyn ChainStore>,
}

impl<S: Store + Send + Sync, HS: HistoricalStores + Send + Sync + 'static> CanValidateTxs for BlockValidator<S, HS> {
    fn validate_tx(&self, tx: &Transaction) -> Result<(), TransactionValidationError> {
        let state = self.state.lock().map_err(|error| {
            TransactionValidationError::from(anyhow!("failed to acquire ledger state lock: {error}"))
        })?;
        state
            .validate_tx(tx, state.tip().slot_or_default(), &self.vm_eval_pool)
            .map_err(|error| TransactionValidationError::from(anyhow!(error)))
    }
}

impl<S: Store, HS: HistoricalStores + Send + Sync + 'static> BlockValidator<S, HS> {
    pub fn new(state: State<S, HS>, vm_eval_pool: ArenaPool, chain_store: Arc<dyn ChainStore>) -> Self {
        Self { state: Arc::new(Mutex::new(state)), vm_eval_pool, chain_store }
    }

    /// Set callback invoked when a new stake distribution is computed/available.
    /// The provided PoolSummaries should be used to update resources for header validation.
    #[expect(clippy::unwrap_used)]
    pub fn set_on_stake_dist_updated(&self, callback: Arc<dyn Fn(PoolSummaries) + Send + Sync>) {
        let mut state = self.state.lock().unwrap();
        state.set_on_stake_dist_updated(callback);
    }
}

#[async_trait::async_trait]
impl<S: Store + Send + Sync, HS: HistoricalStores + Send + Sync + 'static> CanValidateBlocks for BlockValidator<S, HS> {
    #[expect(clippy::unwrap_used)]
    async fn roll_forward_block(
        &self,
        block: Block,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        let mut state = self.state.lock().unwrap();
        match state.roll_forward(&block, &self.vm_eval_pool) {
            BlockValidation::Valid(metrics) => Ok(Ok(metrics)),
            BlockValidation::Invalid(_, details) => {
                Ok(Err(BlockValidationError::new(anyhow!("Invalid block: {details}"))))
            }
            BlockValidation::Err(err) => Err(BlockValidationError::new(anyhow!(err))),
        }
    }

    #[expect(clippy::unwrap_used)]
    fn switch_to_fork(&self, fork_point: &Point, to: &Tip) -> Result<ForkSwitchOutcome, BlockValidationError> {
        // Get all the headers of the block to apply between the fork point and the expected new tip
        let Some(forward_tips) = self.chain_store.ancestors_between(fork_point, to.hash()) else {
            return Err(BlockValidationError::new(anyhow!(
                "the stored headers do not form a chain from {fork_point} to {}",
                to.point()
            )));
        };

        // Load the blocks corresponding to the headers to apply, in order, from the chain store.
        let forward_blocks = forward_tips
            .iter()
            .map(|forward_tip| {
                self.chain_store
                    .load_block(&forward_tip.hash())
                    .map_err(|e| BlockValidationError::new(anyhow!(e)))?
                    .ok_or_else(|| BlockValidationError::new(anyhow!("block not found")))?
                    .decode()
                    .map_err(|e| BlockValidationError::new(anyhow!(e)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut state = self.state.lock().unwrap();

        if forward_blocks.is_empty() {
            // We are not supposed to switch to a fork that would be a no-op for the ledger.
            return Err(BlockValidationError::new(anyhow!("block already applied to the ledger: {}", to.point())));
        }

        // Now switch the fork in the ledger by rolling back to the fork point and then rolling forward the blocks to the new tip.
        state
            .switch_to_fork(fork_point, forward_blocks, &self.vm_eval_pool)
            .map_err(|err| BlockValidationError::from(anyhow!(err)))
    }

    #[expect(clippy::unwrap_used)]
    fn tip(&self) -> Point {
        let state = self.state.lock().unwrap();
        state.tip().into_owned()
    }

    #[expect(clippy::unwrap_used)]
    fn volatile_tip(&self) -> Option<Tip> {
        let state = self.state.lock().unwrap();
        state.volatile_tip()
    }
}

impl<S, HS> HasStakePools for BlockValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send + Sync + 'static,
{
    fn registered_relay_socket_addrs(&self) -> Result<BTreeSet<SocketAddr>, BlockValidationError> {
        #[expect(clippy::unwrap_used)]
        {
            let state = self.state.lock().unwrap();
            state.registered_relay_socket_addrs().map_err(|e| BlockValidationError::new(anyhow!(e)))
        }
    }
}
