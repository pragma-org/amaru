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

use std::{collections::BTreeSet, sync::Arc};

use amaru_kernel::{BlockHeight, IsHeader, Point, Tip};
use amaru_metrics::ledger::LedgerMetrics;
use async_trait::async_trait;
use parking_lot::Mutex;

use crate::{CanValidateBlocks, can_validate_blocks::BlockValidationError, stores::chain_store::ReadChainStore};

/// Configurable mock ledger for testing rollback/roll-forward paths.
pub struct MockBlockValidator {
    inner: Mutex<MockBlockValidatorInner>,
}

struct MockBlockValidatorInner {
    /// Points the ledger is considered to contain (validated).
    contains: BTreeSet<Point>,
    /// Current ledger tip.
    tip: Point,
    /// If set, rollback_block will return this error.
    rollback_fails: bool,
    /// If set, roll_forward_block will return Ok(Err(...)) for these points.
    validate_fails: BTreeSet<Point>,
    /// If set, roll_forward_block will return Err(...) for these points.
    ledger_fails: BTreeSet<Point>,
    /// If set, `switch_to_fork` walks this chain store to reconstruct the forward
    /// chain from the rollback point up to `new_tip`. Without it, only `new_tip`
    /// itself is treated as the forward block to apply.
    chain_store: Option<Arc<dyn ReadChainStore>>,
}

impl Default for MockBlockValidator {
    fn default() -> Self {
        Self::new(Point::Origin)
    }
}

impl MockBlockValidator {
    pub fn new(tip: Point) -> Self {
        Self {
            inner: Mutex::new(MockBlockValidatorInner {
                contains: BTreeSet::default(),
                tip,
                rollback_fails: false,
                validate_fails: BTreeSet::default(),
                ledger_fails: BTreeSet::default(),
                chain_store: None,
            }),
        }
    }

    pub fn with_contains(&self, point: Point) -> &Self {
        self.inner.lock().contains.insert(point);
        self
    }

    pub fn with_rollback_fails(&self, fails: bool) -> &Self {
        self.inner.lock().rollback_fails = fails;
        self
    }

    pub fn with_validate_fails(&self, point: Point) -> &Self {
        self.inner.lock().validate_fails.insert(point);
        self
    }

    pub fn with_ledger_fails(&self, point: Point) -> &Self {
        self.inner.lock().ledger_fails.insert(point);
        self
    }

    pub fn with_tip(&self, tip: Point) -> &Self {
        self.inner.lock().tip = tip;
        self
    }

    pub fn with_chain_store(&self, store: Arc<dyn ReadChainStore>) -> &Self {
        self.inner.lock().chain_store = Some(store);
        self
    }
}

#[async_trait]
impl CanValidateBlocks for MockBlockValidator {
    fn roll_forward_block(
        &self,
        point: &Point,
        _block: amaru_kernel::Block,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        let mut inner = self.inner.lock();
        if inner.ledger_fails.contains(point) {
            return Err(BlockValidationError::new(anyhow::anyhow!("mock ledger failed")));
        }
        if inner.validate_fails.contains(point) {
            return Ok(Err(BlockValidationError::new(anyhow::anyhow!("mock validation failed"))));
        }
        inner.contains.insert(*point);
        inner.tip = *point;
        Ok(Ok(Default::default()))
    }

    fn switch_to_fork(
        &self,
        _old_tip: &Point,
        new_tip: &Point,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        // First handle the rollback step.
        let (contains_snapshot, chain_store) = {
            let inner = self.inner.lock();
            if inner.rollback_fails {
                return Err(BlockValidationError::new(anyhow::anyhow!("mock rollback failed")));
            }
            (inner.contains.clone(), inner.chain_store.clone())
        };

        // Walk back from new_tip through the chain store, collecting forward points
        // (in reverse order). We stop when we reach a point that is already in `contains`
        // (the implicit rollback point) or run out of chain (genesis / unknown header).
        let mut cursor = *new_tip;
        let mut forward_chain = Vec::new();
        loop {
            if contains_snapshot.contains(&cursor) {
                break;
            }
            forward_chain.push(cursor);
            let Some(store) = &chain_store else { break };
            let Some(header) = store.load_header(&cursor.hash()) else { break };
            let Some(parent_hash) = header.parent() else { break };
            let Some(parent_header) = store.load_header(&parent_hash) else { break };
            cursor = parent_header.point();
        }
        forward_chain.reverse();

        // Check the entire forward chain for failures BEFORE mutating state, so that
        // a failure halfway through leaves `contains` and `tip` exactly as they were.
        let mut inner = self.inner.lock();
        for point in &forward_chain {
            if inner.ledger_fails.contains(point) {
                return Err(BlockValidationError::new(anyhow::anyhow!("mock ledger failed at {point}")));
            }
            if inner.validate_fails.contains(point) {
                return Ok(Err(BlockValidationError::new(anyhow::anyhow!("mock validation failed at {point}"))));
            }
        }
        // All blocks validate — commit the new state atomically.
        for point in &forward_chain {
            inner.contains.insert(*point);
        }
        inner.tip = *new_tip;
        Ok(Ok(Default::default()))
    }

    fn contains_point(&self, point: &Point) -> bool {
        self.inner.lock().contains.contains(point)
    }

    fn tip(&self) -> Point {
        self.inner.lock().tip
    }

    fn volatile_tip(&self) -> Option<Tip> {
        let inner = self.inner.lock();
        inner.contains.last().map(|p| Tip::new(*p, BlockHeight::from(inner.contains.len() as u64) + 1))
    }
}
