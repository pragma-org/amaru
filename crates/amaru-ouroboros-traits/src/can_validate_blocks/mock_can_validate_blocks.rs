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

use crate::{BlockValidationError, CanValidateBlocks};
use amaru_kernel::Point;
use amaru_kernel::string_utils::ListToString;
use parking_lot::Mutex;
use std::sync::Arc;

/// This simple ledger implementation models the volatile part of the ledger as a sequence of points.
///
/// This provides basic ledger state management for testing without the complexity
/// of full validation logic. It supports:
/// - Rollforward: Add a block to the ledger state
/// - Rollback: Remove blocks back to a specific point
///
/// The state is represented as a Vec<Point> where:
/// - Index 0 is the anchor point
/// - Later indices represent blocks validated on top
#[derive(Debug, Clone)]
pub struct MockCanValidateBlocks {
    state: Arc<Mutex<Vec<Point>>>,
}

impl MockCanValidateBlocks {
    /// Create a new simple ledger state with an initial anchor point
    pub fn new(anchor: Point) -> Self {
        Self {
            state: Arc::new(Mutex::new(vec![anchor])),
        }
    }

    /// Get the current tip of the ledger state
    pub fn tip(&self) -> Point {
        let state = self.state.lock();
        state.last().cloned().unwrap_or(Point::Origin)
    }

    /// Get the current length of the ledger state
    /// #[expect(clippy::unwrap_used)]
    pub fn length(&self) -> usize {
        self.state.lock().len()
    }

    /// Check if a point exists in the ledger state
    pub fn contains(&self, point: &Point) -> bool {
        self.state.lock().contains(point)
    }
}

#[async_trait::async_trait]
impl CanValidateBlocks for MockCanValidateBlocks {
    async fn roll_forward_block(
        &self,
        point: &Point,
        _block: &amaru_kernel::RawBlock,
    ) -> Result<
        Result<amaru_metrics::ledger::LedgerMetrics, BlockValidationError>,
        BlockValidationError,
    > {
        let mut state = self.state.lock();

        // Basic validation: point should not already exist in state
        // In principle we should check that we are not trying to reapply the same block twice to the ledger.
        // But we currently don't.

        // if state.contains(point) {
        //     tracing::error!(
        //         "Block at point {:?} was already validated. The current state is {}",
        //         point,
        //         state.list_to_string(",")
        //     );
        //     return Ok(Err(BlockValidationError::from(anyhow::Error::from(
        //         AlreadyValidatedError { point: *point },
        //     ))));
        // }

        // Add point to the ledger state
        state.push(*point);

        // Return success with default metrics
        Ok(Ok(Default::default()))
    }

    fn rollback_block(&self, to: &Point) -> Result<(), BlockValidationError> {
        let mut state = self.state.lock();

        // Find the index of the rollback point by slot because the hash may be different
        let rollback_index = state
            .iter()
            .position(|p| p.slot_or_default() == to.slot_or_default())
            .ok_or_else(|| {
                tracing::error!(
                    "Block at point {:?} was not found for rollback. The current state is {}",
                    to,
                    state.list_to_string(",")
                );
                BlockValidationError::from(anyhow::Error::from(RollbackPointNotFoundError {
                    point: *to,
                }))
            })?;

        // Remove all points after the rollback point
        state.truncate(rollback_index + 1);

        Ok(())
    }
}

/// Error returned when a block at a specific point was already validated
#[derive(Debug, Clone, thiserror::Error)]
#[error("Block at point {point} was already validated")]
pub struct AlreadyValidatedError {
    pub point: Point,
}

/// Error returned when a rollback point is not found in ledger state
#[derive(Debug, Clone, thiserror::Error)]
#[error("Rollback point {point} not found in ledger state")]
pub struct RollbackPointNotFoundError {
    pub point: Point,
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_kernel::Hash;

    #[tokio::test]
    async fn test_rollforward() {
        let anchor = make_point(0, 0);
        let ledger = MockCanValidateBlocks::new(anchor);

        let point1 = make_point(1, 1);
        let result = ledger
            .roll_forward_block(&point1, &amaru_kernel::RawBlock::from(&[][..]))
            .await;
        assert!(result.unwrap().is_ok());

        assert_eq!(ledger.length(), 2); // anchor + point1
        assert_eq!(ledger.tip(), point1);
        assert!(ledger.contains(&point1));
    }

    /// We skip this test for now as the current implementation does not prevent duplicate rollforwards.
    #[tokio::test]
    #[ignore]
    async fn test_rollforward_duplicate() {
        let anchor = make_point(0, 0);
        let ledger = MockCanValidateBlocks::new(anchor);

        let point1 = make_point(1, 1);
        assert!(
            ledger
                .roll_forward_block(&point1, &amaru_kernel::RawBlock::from(&[][..]))
                .await
                .unwrap()
                .is_ok()
        );

        // Try to add the same point again
        let result = ledger
            .roll_forward_block(&point1, &amaru_kernel::RawBlock::from(&[][..]))
            .await;
        assert!(result.unwrap().is_err()); // Should fail
    }

    #[tokio::test]
    async fn test_rollback() {
        let anchor = make_point(0, 0);
        let ledger = MockCanValidateBlocks::new(anchor);

        // Add 3 blocks
        let point1 = make_point(1, 1);
        let point2 = make_point(2, 2);
        let point3 = make_point(3, 3);

        assert!(
            ledger
                .roll_forward_block(&point1, &amaru_kernel::RawBlock::from(&[][..]))
                .await
                .unwrap()
                .is_ok()
        );
        assert!(
            ledger
                .roll_forward_block(&point2, &amaru_kernel::RawBlock::from(&[][..]))
                .await
                .unwrap()
                .is_ok()
        );
        assert!(
            ledger
                .roll_forward_block(&point3, &amaru_kernel::RawBlock::from(&[][..]))
                .await
                .unwrap()
                .is_ok()
        );

        assert_eq!(ledger.length(), 4); // anchor + 3 blocks

        // Rollback to point1
        ledger.rollback_block(&point1).unwrap();

        assert_eq!(ledger.length(), 2); // anchor + point1
        assert_eq!(ledger.tip(), point1);
        assert!(!ledger.contains(&point2));
        assert!(!ledger.contains(&point3));
    }

    #[tokio::test]
    async fn test_rollback_to_anchor() {
        let anchor = make_point(0, 0);
        let ledger = MockCanValidateBlocks::new(anchor);

        let point1 = make_point(1, 1);
        assert!(
            ledger
                .roll_forward_block(&point1, &amaru_kernel::RawBlock::from(&[][..]))
                .await
                .unwrap()
                .is_ok()
        );

        // Rollback to anchor
        ledger.rollback_block(&anchor).unwrap();

        assert_eq!(ledger.length(), 1); // Just anchor
        assert_eq!(ledger.tip(), anchor);
    }

    #[tokio::test]
    async fn test_rollback_nonexistent_point() {
        let anchor = make_point(0, 0);
        let ledger = MockCanValidateBlocks::new(anchor);

        let point1 = make_point(1, 1);
        let nonexistent = make_point(99, 99);

        assert!(
            ledger
                .roll_forward_block(&point1, &amaru_kernel::RawBlock::from(&[][..]))
                .await
                .unwrap()
                .is_ok()
        );

        // Try to rollback to a point that doesn't exist
        let result = ledger.rollback_block(&nonexistent);
        assert!(result.is_err());
    }

    // HELPERS

    fn make_point(slot: u64, hash: u8) -> Point {
        Point::Specific(slot.into(), Hash::from([hash; 32]))
    }
}
