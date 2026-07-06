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

use std::sync::{Arc, Mutex};

use amaru_kernel::{Block, Point, Tip, Transaction};
use amaru_ledger::{
    rules::block::BlockValidation,
    state::State,
    store::{HistoricalStores, Store},
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros_traits::{
    BaseReadChainStore, CanValidateBlocks, CanValidateTxs, ChainStore, FindAncestorOnBestChainResult, ReadChainStore,
    TransactionValidationError, can_validate_blocks::BlockValidationError,
};
use amaru_plutus::arena_pool::ArenaPool;
use anyhow::anyhow;

/// This data type encapsulate the ledger state in order to implement the `CanValidateBlocks` trait.
/// and be able to validate blocks (including rollback).
pub struct BlockValidator<S: Store, HS: HistoricalStores> {
    state: Arc<Mutex<State<S, HS>>>,
    vm_eval_pool: ArenaPool,
    chain_store: Arc<dyn ChainStore>,
}

impl<S: Store, HS: HistoricalStores> Clone for BlockValidator<S, HS> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            vm_eval_pool: self.vm_eval_pool.clone(),
            chain_store: self.chain_store.clone(),
        }
    }
}

impl<S: Store + Send + Sync, HS: HistoricalStores + Send + Sync> CanValidateTxs for BlockValidator<S, HS> {
    fn validate_tx(&self, tx: &Transaction) -> Result<(), TransactionValidationError> {
        let state = self.state.lock().map_err(|error| {
            TransactionValidationError::from(anyhow!("failed to acquire ledger state lock: {error}"))
        })?;
        state
            .validate_tx(tx, state.tip().slot_or_default(), &self.vm_eval_pool)
            .map_err(|error| TransactionValidationError::from(anyhow!(error)))
    }
}

impl<S: Store, HS: HistoricalStores + Send> BlockValidator<S, HS> {
    pub fn new(state: State<S, HS>, vm_eval_pool: ArenaPool, chain_store: Arc<dyn ChainStore>) -> Self {
        Self { state: Arc::new(Mutex::new(state)), vm_eval_pool, chain_store }
    }
}

impl<S: Store, HS: HistoricalStores> BlockValidator<S, HS>
where
    HS: Send,
{
    #[expect(clippy::unwrap_used)]
    pub fn get_tip(&self) -> Point {
        let state = self.state.lock().unwrap();
        state.tip().into_owned()
    }
}

#[async_trait::async_trait]
impl<S: Store + Send + Sync, HS: HistoricalStores + Send + Sync> CanValidateBlocks for BlockValidator<S, HS> {
    #[expect(clippy::unwrap_used)]
    async fn roll_forward_block(
        &self,
        point: &Point,
        block: Block,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        let mut state = self.state.lock().unwrap();
        match state.roll_forward(point, block, &self.vm_eval_pool) {
            BlockValidation::Valid(metrics) => Ok(Ok(metrics)),
            BlockValidation::Invalid(_, _, details) => {
                Ok(Err(BlockValidationError::new(anyhow!("Invalid block: {details}"))))
            }
            BlockValidation::Err(err) => Err(BlockValidationError::new(anyhow!(err))),
        }
    }

    #[expect(clippy::unwrap_used)]
    fn switch_to_fork(&self, to: &Point) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        match self
            .chain_store
            .find_ancestor_on_best_chain(to.hash())
            .map_err(|error| BlockValidationError::new(anyhow!(error)))?
        {
            FindAncestorOnBestChainResult::StartHeaderNotFound => {
                Err(BlockValidationError::new(anyhow!("header missing for {}", to.hash())))
            }
            FindAncestorOnBestChainResult::NotFound => {
                Err(BlockValidationError::new(anyhow!("no ancestor on best chain chain {}", to.hash())))
            }
            FindAncestorOnBestChainResult::Found { fork_point, forward_points } => {
                let mut state = self.state.lock().unwrap();
                let mut ledger_metrics = LedgerMetrics::default();
                let state_recovery =
                    state.rollback_to(&fork_point).map_err(|error| BlockValidationError::from(anyhow!(error)))?;
                for point in forward_points.iter() {
                    let block = self
                        .chain_store
                        .load_block(&point.hash())
                        .map_err(|e| BlockValidationError::new(e.into()))?
                        .ok_or(BlockValidationError::new(anyhow::anyhow!("block not found")))?
                        .decode()
                        .map_err(|e| BlockValidationError::new(e.into()))?;

                    match state.roll_forward(point, block, &self.vm_eval_pool) {
                        BlockValidation::Valid(metrics) => {
                            ledger_metrics = metrics;
                        }
                        BlockValidation::Invalid(_, _, details) => {
                            state.recover(state_recovery);
                            return Ok(Err(BlockValidationError::new(anyhow!("Invalid block: {details}"))));
                        }
                        BlockValidation::Err(err) => return Err(BlockValidationError::new(anyhow!(err))),
                    }
                }
                Ok(Ok(ledger_metrics))
            }
        }
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
