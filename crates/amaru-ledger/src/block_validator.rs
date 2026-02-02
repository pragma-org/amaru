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

use crate::{
    rules::block::BlockValidation,
    state,
    store::{HistoricalStores, Store},
};
use amaru_kernel::{EraHistory, GlobalParameters, NetworkName, Point, RawBlock};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros_traits::{CanValidateBlocks, can_validate_blocks::BlockValidationError};
use amaru_plutus::arena_pool::ArenaPool;
use anyhow::anyhow;
use std::sync::{Arc, Mutex};

/// This data type encapsulate the ledger state in order to implement the `CanValidateBlocks` trait.
/// and be able to validate blocks (including rollback).
pub struct BlockValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    pub state: Arc<Mutex<state::State<S, HS>>>,
    pub vm_eval_pool: ArenaPool,
}

impl<S: Store + Send, HS: HistoricalStores + Send> BlockValidator<S, HS> {
    pub fn new(
        store: S,
        snapshots: HS,
        vm_eval_pool: ArenaPool,
        network: NetworkName,
        era_history: EraHistory,
        global_parameters: GlobalParameters,
    ) -> anyhow::Result<Self> {
        let state = state::State::new(store, snapshots, network, era_history, global_parameters)?;
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
            vm_eval_pool,
        })
    }

    #[expect(clippy::unwrap_used)]
    pub fn get_tip(&self) -> Point {
        let state = self.state.lock().unwrap();
        state.tip().into_owned()
    }
}

#[async_trait::async_trait]
impl<S, HS> CanValidateBlocks for BlockValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    #[expect(clippy::unwrap_used)]
    async fn roll_forward_block(
        &self,
        point: &Point,
        raw_block: &RawBlock,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        let mut state = self.state.lock().unwrap();
        match state.roll_forward(point, raw_block, &self.vm_eval_pool) {
            BlockValidation::Valid(metrics) => Ok(Ok(metrics)),
            BlockValidation::Invalid(_, _, details) => Ok(Err(BlockValidationError::new(anyhow!(
                "Invalid block: {details}"
            )))),
            BlockValidation::Err(err) => Err(BlockValidationError::new(anyhow!(err))),
        }
    }

    #[expect(clippy::unwrap_used)]
    fn rollback_block(&self, to: &Point) -> Result<(), BlockValidationError> {
        let mut state = self.state.lock().unwrap();
        state
            .rollback_to(to)
            .map_err(|e| BlockValidationError::new(anyhow!(e)))
    }
}
