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

use std::collections::BTreeSet;

use amaru_kernel::{BlockHeight, Point, Tip};
use amaru_metrics::ledger::LedgerMetrics;
use async_trait::async_trait;
use parking_lot::Mutex;

use crate::{CanValidateBlocks, can_validate_blocks::BlockValidationError};

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
}

impl Default for MockBlockValidator {
    fn default() -> Self {
        MockBlockValidator::new(Point::Origin)
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
}

#[async_trait]
impl CanValidateBlocks for MockBlockValidator {
    async fn roll_forward_block(
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

    fn switch_to_fork(&self, to: &Point) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        let mut inner = self.inner.lock();
        if inner.rollback_fails {
            return Err(BlockValidationError::new(anyhow::anyhow!("mock rollback failed")));
        }
        if inner.ledger_fails.contains(to) {
            return Err(BlockValidationError::new(anyhow::anyhow!("mock ledger failed")));
        }
        if inner.validate_fails.contains(to) {
            return Ok(Err(BlockValidationError::new(anyhow::anyhow!("mock validation failed"))));
        }
        inner.tip = *to;
        inner.contains.retain(|p| p.slot_or_default() <= to.slot_or_default());
        inner.contains.insert(*to);
        Ok(Ok(Default::default()))
    }

    fn tip(&self) -> Point {
        self.inner.lock().tip
    }

    fn volatile_tip(&self) -> Option<Tip> {
        let inner = self.inner.lock();
        inner.contains.last().map(|p| Tip::new(*p, BlockHeight::from(inner.contains.len() as u64) + 1))
    }
}
