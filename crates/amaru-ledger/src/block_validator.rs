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

use amaru_kernel::{Block, IsHeader, Point, Tip, Transaction};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros_traits::{
    BaseReadChainStore, CanValidateBlocks, CanValidateTxs, ReadChainStore, TransactionValidationError,
    can_validate_blocks::BlockValidationError,
};
use amaru_plutus::arena_pool::ArenaPool;
use anyhow::anyhow;

use crate::{
    rules::block::BlockValidation,
    state::State,
    state_snapshot::StateSnapshot,
    store::{HistoricalStores, Store},
};

/// This data type encapsulates the ledger state in order to implement various traits supporting the validation of blocks:
///
///  - `CanValidateBlocks` validates block transactions.
///  - `CanValidateTxs` validates individual transactions.
pub struct BlockValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    pub state: State<S, HS>,
    pub chain_store: Arc<dyn ReadChainStore>,
    pub vm_eval_pool: ArenaPool,
}

impl<S, HS> Clone for BlockValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            chain_store: self.chain_store.clone(),
            vm_eval_pool: self.vm_eval_pool.clone(),
        }
    }
}

impl<S: Store + Send, HS: HistoricalStores + Send> BlockValidator<S, HS> {
    pub fn new(
        state: State<S, HS>,
        chain_store: Arc<dyn ReadChainStore>,
        vm_eval_pool: ArenaPool,
    ) -> anyhow::Result<Self> {
        Ok(Self { state, chain_store, vm_eval_pool })
    }

    pub fn get_tip(&self) -> Point {
        self.state.load().tip().into_owned()
    }
}

impl<S, HS> CanValidateTxs for BlockValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    fn validate_tx(&self, tx: &Transaction) -> Result<(), TransactionValidationError> {
        let view = self.state.load();
        view.validate_tx(tx, view.tip().slot_or_default(), &self.vm_eval_pool)
            .map_err(|error| TransactionValidationError::from(anyhow!(error)))
    }
}

#[async_trait::async_trait]
impl<S, HS> CanValidateBlocks for BlockValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    fn roll_forward_block(
        &self,
        point: &Point,
        block: Block,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        self.forward_block(point, block)
    }

    fn switch_to_fork(
        &self,
        _old_tip: &Point,
        new_tip: &Point,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        let (rollback_point, forward_points) = self.find_rollback_point(new_tip)?;

        // Use one atomic transaction over a candidate `StateSnapshot`.
        // The candidate is published only if the rollback and all the roll forwards succeed.
        self.state.atomically(|view| {
            let result = self.switch_fork(view, rollback_point, forward_points);
            let publish = matches!(result, Ok(Ok(_)));
            (result, publish)
        })
    }

    fn contains_point(&self, point: &Point) -> bool {
        self.state.load().contains_volatile_point(point)
    }

    fn tip(&self) -> Point {
        self.state.load().tip().into_owned()
    }

    fn volatile_tip(&self) -> Option<Tip> {
        self.state.load().volatile_tip()
    }
}

impl<S, HS> BlockValidator<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    /// Roll forward a block on the ledger
    fn forward_block(
        &self,
        point: &Point,
        block: Block,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        match self.state.roll_forward(point, block, &self.vm_eval_pool) {
            BlockValidation::Valid(metrics) => Ok(Ok(metrics)),
            BlockValidation::Invalid(_, _, details) => {
                Ok(Err(BlockValidationError::new(anyhow!("Invalid block: {details}"))))
            }
            BlockValidation::Err(err) => Err(BlockValidationError::new(anyhow!(err))),
        }
    }

    /// Roll the candidate `state_snapshot` back to `rollback_point` and replay each block in `forward_points`
    /// on top of it.
    fn switch_fork(
        &self,
        state_snapshot: &mut StateSnapshot<S, HS>,
        rollback_point: Point,
        forward_points: Vec<Point>,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        state_snapshot.rollback_to(&rollback_point).map_err(|err| BlockValidationError::new(anyhow!(err)))?;
        let mut last = Ok(LedgerMetrics::default());
        let to_do = forward_points.len();
        for (i, point) in forward_points.into_iter().enumerate() {
            let block = self
                .chain_store
                .load_block(&point.hash())
                .map_err(|err| BlockValidationError::new(anyhow!(err)))?
                .ok_or_else(|| BlockValidationError::new(anyhow!("Block not found in chain store: {point}")))?
                .decode()
                .map_err(|e| BlockValidationError::new(e.into()))?;
            match state_snapshot.roll_forward(&point, block, &self.vm_eval_pool) {
                BlockValidation::Valid(metrics) => {
                    last = Ok(metrics);
                    let done = i + 1;
                    if done % 100 == 0 {
                        tracing::info!(%done, %to_do, "rolling forward ledger to reach fork tip");
                    }
                }
                BlockValidation::Invalid(_, _, details) => {
                    return Ok(Err(BlockValidationError::new(anyhow!("Invalid block: {details}"))));
                }
                BlockValidation::Err(err) => {
                    return Err(BlockValidationError::new(anyhow!(err)));
                }
            }
        }
        Ok(last)
    }

    /// Return a point on the ledger volatile state that is an ancestor of the tip, and all the points
    /// in between.
    fn find_rollback_point(&self, tip: &Point) -> Result<(Point, Vec<Point>), BlockValidationError> {
        let state_snapshot = self.state.load();
        // search will abort at this point
        let ledger_tip = state_snapshot.immutable_tip();
        let mut current_hash = tip.hash();
        let mut forward_points = Vec::new();
        loop {
            let (current_header, valid) =
                self.chain_store.load_header_with_validity(&current_hash).ok_or_else(|| {
                    BlockValidationError::new(anyhow!(
                        "failed to load header {current_hash} from store while searching for rollback point"
                    ))
                })?;
            let current_point = current_header.point();

            if valid == Some(false) {
                return Err(BlockValidationError::new(anyhow!(
                    "block built on invalid block. tip {tip}, invalid {current_point}"
                )));
            }

            if current_point < ledger_tip {
                return Err(BlockValidationError::new(anyhow!(
                    "invalid rollback. rollback_point {current_hash}, max_point {tip}"
                )));
            }

            if current_point == ledger_tip || state_snapshot.contains_volatile_point(&current_point) {
                forward_points.reverse();
                return Ok((current_point, forward_points));
            }
            forward_points.push(current_point);

            // NOTE: parent links are validated by track_peers already, and we are younger than ledger_tip
            current_hash = current_header.parent().ok_or_else(|| {
                BlockValidationError::new(anyhow!(
                    "reached genesis block while searching for rollback point for {current_hash}"
                ))
            })?;
        }
    }
}
