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
use amaru_ledger::{rules::block::BlockValidation, state::State};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros_traits::{
    CanValidateBlocks, CanValidateTxs, ChainStore, TransactionValidationError,
    can_validate_blocks::BlockValidationError,
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores};
use anyhow::anyhow;

/// This data type encapsulate the ledger state in order to implement the `CanValidateBlocks` trait.
/// and be able to validate blocks (including rollback).
pub struct BlockValidator {
    state: Arc<Mutex<State<RocksDB, RocksDBHistoricalStores>>>,
    vm_eval_pool: ArenaPool,
    chain_store: Arc<dyn ChainStore>,
}

impl Clone for BlockValidator {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            vm_eval_pool: self.vm_eval_pool.clone(),
            chain_store: self.chain_store.clone(),
        }
    }
}

impl CanValidateTxs for BlockValidator {
    fn validate_tx(&self, tx: &Transaction) -> Result<(), TransactionValidationError> {
        let state = self.state.lock().map_err(|error| {
            TransactionValidationError::from(anyhow!("failed to acquire ledger state lock: {error}"))
        })?;
        state
            .validate_tx(tx, state.tip().slot_or_default(), &self.vm_eval_pool)
            .map_err(|error| TransactionValidationError::from(anyhow!(error)))
    }
}

impl BlockValidator {
    pub fn new(
        state: State<RocksDB, RocksDBHistoricalStores>,
        vm_eval_pool: ArenaPool,
        chain_store: Arc<dyn ChainStore>,
    ) -> Self {
        Self { state: Arc::new(Mutex::new(state)), vm_eval_pool, chain_store }
    }

    #[expect(clippy::unwrap_used)]
    pub fn get_tip(&self) -> Point {
        let state = self.state.lock().unwrap();
        state.tip().into_owned()
    }
}

#[async_trait::async_trait]
impl CanValidateBlocks for BlockValidator {
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
    fn switch_to_fork(&self, _to: &Point) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        let mut _state = self.state.lock().unwrap();
        // match state.switch_to_fork(to, &self.vm_eval_pool) {
        //     BlockValidation::Valid(metrics) => Ok(Ok(metrics)),
        //     BlockValidation::Invalid(_, _, details) => {
        //         Ok(Err(BlockValidationError::new(anyhow!("Invalid block: {details}"))))
        //     }
        //     BlockValidation::Err(err) => Err(BlockValidationError::new(anyhow!(err))),
        // }
        Ok(Ok(Default::default()))
    }

    #[expect(clippy::unwrap_used)]
    fn contains_point(&self, point: &Point) -> bool {
        let state = self.state.lock().unwrap();
        state.contains_volatile_point(point)
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
