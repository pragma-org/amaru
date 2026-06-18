// Copyright 2024 PRAGMA
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

use std::mem;

use amaru_kernel::{Epoch, MemoizedTransactionOutput, Point, PoolId, TransactionInput};

use crate::state::{
    AnchoredVolatileFragment,
    overlay::StateOverlay,
    volatile::{VolatileSeries, VolatileStore},
};

/// The volatile layers' verdict on whether a pool exists, used to decide whether a stable-store
/// read is still warranted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolExistence {
    /// Registered (or re-registered) in the volatile state
    Exists,
    /// Reaped by the pending epoch-boundary transition. The stable store still holds a stale entry until the overlay flushes,
    /// so the caller must *not* fall back to it.
    Retired,
    /// The volatile layers don't know; the caller should consult the stable store.
    Unknown,
}

#[derive(Default)]
pub struct VolatileDB {
    current: VolatileSeries,
    draining: VolatileSeries,
    /// The volatile bits of the in-flight epoch transition (computed rewards,
    /// pending pools and governance updates). Co-located with the two series so that reads and
    /// rollback stay cohesive: the overlay is the boundary layer that sits *between* `draining`
    /// (the closing epoch) and `current` (the opening epoch).
    overlay: StateOverlay,
}

impl VolatileStore for VolatileDB {
    fn is_empty(&self) -> bool {
        self.current.is_empty() && self.draining.is_empty()
    }

    fn len(&self) -> usize {
        self.current.len() + self.draining.len()
    }

    fn view_back(&self) -> Option<&AnchoredVolatileFragment> {
        self.current.view_back().or(self.draining.view_back())
    }

    fn view_front(&self) -> Option<&AnchoredVolatileFragment> {
        self.draining.view_front().or(self.current.view_front())
    }

    fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        if self.has_consumed_input(input) {
            return None;
        }

        self.current.resolve_input(input).or(self.draining.resolve_input(input))
    }

    fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.current.has_consumed_input(input) || self.draining.has_consumed_input(input)
    }

    fn contains(&self, point: &Point) -> bool {
        self.current.contains(point) || self.draining.contains(point)
    }

    fn pop_front(&mut self) -> Option<AnchoredVolatileFragment> {
        self.draining.pop_front().or_else(|| self.current.pop_front())
    }

    fn push_back(&mut self, fragment: AnchoredVolatileFragment) {
        // By design, we should never be pushing to the back of the draining sequence
        self.current.push_back(fragment);
    }

    #[allow(clippy::expect_used)]
    fn rollback_to<'a>(&mut self, point: &'a Point) -> Result<(), &'a Point> {
        let target_slot = point.slot_or_default();

        // Check if the target point is beyond the newest known fragment across both series
        // In this case we simply return Ok since it this would not change the volatile DB.
        // Use `self.view_back()` so the check still fires when `current` is empty but `draining` holds fragments.
        if let Some(last) = self.view_back()
            && last.slot() < target_slot
        {
            tracing::warn!(
                name: "rollback_to.beyond",
                %target_slot,
                last_slot = ?last.slot(),
                "Attempting to rollback to a point beyond the last known volatile fragment"
            );
            return Ok(());
        }

        // Check if the target point is before the active sequence
        // In this case we return an error since it means rolling back the stable DB
        if let Some(first) = self.view_front()
            && target_slot < first.slot()
        {
            tracing::error!(
                name: "rollback_to.before",
                %target_slot,
                first_slot = ?first.slot(),
                "Attempting to rollback to a point before the first known of the volatile fragment"
            );
            return Err(point);
        }

        // Now we know the target point is within our volatile DB.
        // Keep all elements with point <= target point.

        let should_rollback = if self.draining.contains(point) {
            // If we are rolling back to a point in the draining sequence, we need to
            // promote it as current while discarding the entire current series.
            self.current = std::mem::take(&mut self.draining);
            true
        } else {
            self.current.contains(point)
        };

        if should_rollback {
            self.current.rollback_to(point)?;
            return Ok(());
        }

        Err(point)
    }

    fn clear(&mut self) {
        self.current.clear();
        self.draining.clear();
    }

    fn iter(&self) -> impl Iterator<Item = &AnchoredVolatileFragment> {
        self.draining.iter().chain(self.current.iter())
    }
}

impl VolatileDB {
    /// Construct an empty volatile DB whose overlay is anchored to the given epoch.
    pub fn new(epoch: Epoch) -> Self {
        Self { current: VolatileSeries::default(), draining: VolatileSeries::default(), overlay: StateOverlay::new(epoch) }
    }

    /// A read-only handle on the epoch-transition overlay.
    pub fn overlay(&self) -> &StateOverlay {
        &self.overlay
    }

    /// A mutable handle on the epoch-transition overlay.
    pub fn overlay_mut(&mut self) -> &mut StateOverlay {
        &mut self.overlay
    }

    /// Determine whether a pool exists according to the volatile state, applying the precedence
    /// `current -> overlay (reaping) -> draining`.
    ///
    /// The overlay sits *between* the two series: a re-registration in `current` (the new
    /// epoch) cancels a boundary reaping, so it wins; otherwise a reaping makes the pool
    /// [`PoolExistence::Retired`]. When no layer knows about the pool, the
    /// result is [`PoolExistence::Unknown`] and the caller should consult the stable store.
    pub fn has_pool(&self, pool_id: &PoolId) -> PoolExistence {
        if self.current.pool_exists(pool_id) {
            PoolExistence::Exists
        } else if self.overlay.is_pool_retired(pool_id) {
            PoolExistence::Retired
        } else if self.draining.pool_exists(pool_id) {
            PoolExistence::Exists
        } else {
            PoolExistence::Unknown
        }
    }

    /// Mark the transition between two epochs by sealing the `current` series and turning it into
    /// the `draining` series. This keeps each series epoch-homogeneous since, by the protocol
    /// pre-condition, the `current` series holds only the closing epoch's blocks.
    ///
    /// No-op when `current` is empty: there is nothing to transition, `draining` stays `None`, and
    /// homogeneity still holds because an empty `current` only ever takes new-epoch blocks.
    ///
    /// The `assert!` guards the design's load-bearing precondition: `epochLength` (~10k blocks) is
    /// far larger than the volatile window `k` (2160 blocks at the time of writing), so at most one
    /// epoch boundary is ever inside the window and any prior `draining` series has fully drained
    /// long before the next boundary arrives. A violation would mean two boundaries inside the
    /// window, impossible under the protocol. We `assert!` rather than `debug_assert!` because the
    /// check is effectively free, and if some other bug ever broke the invariant, halting the node
    /// is far safer than silently overwriting `draining` and losing volatile history.
    pub fn transition(&mut self) {
        assert!(
            self.draining.is_empty(),
            "transitioning volatile series while a draining series is still present; two epoch boundaries inside the k-block window?"
        );

        self.draining = mem::take(&mut self.current);
    }
}

#[cfg(test)]
impl VolatileDB {
    pub fn fixture() -> Self {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.push_back(AnchoredVolatileFragment::fixture(30, 3));
        assert_eq!(db.len(), 3);
        db
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{Hash, Point, Slot};
    use proptest::prelude::*;
    use test_case::test_case;

    use super::*;
    use crate::state::volatile::test_support::*;

    #[test]
    fn test_rollback_to_point_before_sequence_fails() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Rollback to slot 5 (before the first element at slot 10)
        // This represents rolling back to a point in the stable DB
        let rollback_point = Point::Specific(Slot::from(5), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point);

        // This should fail
        // (rolling back to a point inside the stable DB is not allowed)
        assert!(result.is_err());
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn test_rollback_to_exact_last_element_should_succeed() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Rollback to slot 30 (the last element)
        let rollback_point = Point::Specific(Slot::from(30), Hash::new([0u8; 32]));

        // This should succeed, keeping all 3 elements
        let result = db.rollback_to(&rollback_point);

        assert!(result.is_ok(), "Rolling back to the exact slot of the last element should succeed");
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn test_rollback_to_middle_element_succeeds() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Rollback to slot 20 (middle element)
        let rollback_point = Point::Specific(Slot::from(20), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point);

        // This should succeed
        assert!(result.is_ok());
        assert_eq!(db.len(), 2, "Should keep elements at slots 10 and 20");
    }

    #[test]
    fn test_rollback_to_point_after_sequence_succeeds() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Try to rollback to slot 40 (after the sequence)
        let rollback_point = Point::Specific(Slot::from(40), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point);

        // This should succeed
        assert!(result.is_ok(), "Rolling back to a point after the sequence should succeed");
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn rollback_beyond_tip_is_a_noop_when_current_is_empty() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.transition();
        assert!(!db.draining.is_empty() && db.current.is_empty());

        let beyond = Point::Specific(Slot::from(30), Hash::new([0u8; 32]));

        assert!(db.rollback_to(&beyond).is_ok(), "rollback beyond the draining tip should be a no-op success");
        assert_eq!(db.len(), 2, "all draining fragments should be retained");
    }

    #[test]
    fn test_rollback_to_slot_between_elements_succeeds() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Rollback to slot 25 (between 20 and 30)
        let rollback_point = Point::Specific(Slot::from(25), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point);

        assert_eq!(result.unwrap_err(), &rollback_point);
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn test_consumed_input_is_tracked() {
        let input = test_input(1);
        let mut anchored = AnchoredVolatileFragment::fixture(10, 1);
        anchored.fragment.utxo.consume(input.clone());

        let mut db = VolatileDB::default();
        db.push_back(anchored);

        assert!(db.has_consumed_input(&input));
        assert!(db.resolve_input(&input).is_none());
    }

    #[test]
    fn test_rollback_removes_consumed_input_from_cache() {
        let input = test_input(1);
        let mut db = VolatileDB::default();
        let first = AnchoredVolatileFragment::fixture(10, 1);
        let first_point = first.point();
        db.push_back(first);

        let mut second = AnchoredVolatileFragment::fixture(20, 2);
        second.fragment.utxo.consume(input.clone());
        db.push_back(second);

        assert!(db.has_consumed_input(&input));

        db.rollback_to(&first_point).unwrap();

        assert!(!db.has_consumed_input(&input));
    }

    #[test]
    fn transition_opens_draining_and_resets_current() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));

        db.transition();

        assert!(!db.draining.is_empty(), "draining should hold the transitioned series");
        assert_eq!(db.current.len(), 0, "current should be reset to empty");
        assert_eq!(db.len(), 2, "total length is unchanged by transitioning");
    }

    #[test]
    fn transition_is_a_noop_on_empty_current() {
        let mut db = VolatileDB::default();

        db.transition();

        assert!(db.draining.is_empty(), "transitioning an empty current must not open a draining series");
    }

    #[test]
    #[should_panic(expected = "two epoch boundaries")]
    fn transition_panics_if_draining_already_present() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.transition();

        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.transition();
    }

    #[test]
    fn pop_front_drains_draining_then_nulls_it() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.transition();
        db.push_back(AnchoredVolatileFragment::fixture(30, 3));

        assert_eq!(db.pop_front().map(|fragment| fragment.slot()), Some(Slot::from(10)));
        assert!(!db.draining.is_empty(), "draining still holds one block");

        assert_eq!(db.pop_front().map(|fragment| fragment.slot()), Some(Slot::from(20)));
        assert!(db.draining.is_empty(), "draining is nulled once it empties");

        assert_eq!(db.pop_front().map(|fragment| fragment.slot()), Some(Slot::from(30)));
        assert!(db.is_empty());
    }

    #[test]
    fn cross_series_views_span_both_series_oldest_to_newest() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.transition();
        db.push_back(AnchoredVolatileFragment::fixture(30, 3));
        db.push_back(AnchoredVolatileFragment::fixture(40, 4));

        let slots = db.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>();
        assert_eq!(
            slots,
            vec![Slot::from(10), Slot::from(20), Slot::from(30), Slot::from(40)],
            "iter spans draining then current, oldest to newest"
        );
        assert_eq!(db.view_front().map(|fragment| fragment.slot()), Some(Slot::from(10)), "front is draining's oldest");
        assert_eq!(db.view_back().map(|fragment| fragment.slot()), Some(Slot::from(40)), "back is current's newest");
    }

    #[test_case(Some(Where::Draining), None, true, false; "produced in draining, unconsumed")]
    #[test_case(Some(Where::Current), None, true, false; "produced in current, unconsumed")]
    #[test_case(Some(Where::Draining), Some(Where::Current), false, true; "produced in draining, consumed in current")]
    #[test_case(None, Some(Where::Current), false, true; "consumed in current, produced only in the stable store")]
    fn cross_series_resolve_precedence(
        produce_in: Option<Where>,
        consume_in: Option<Where>,
        resolvable: bool,
        consumed: bool,
    ) {
        let input = test_input(1);
        let mut draining_block = AnchoredVolatileFragment::fixture(10, 1);
        let mut current_block = AnchoredVolatileFragment::fixture(20, 2);

        if let Some(layer) = produce_in {
            let block = match layer {
                Where::Draining => &mut draining_block,
                Where::Current => &mut current_block,
            };
            block.fragment.utxo.produce(input.clone(), Arc::new(fixed_output()));
        }

        if let Some(layer) = consume_in {
            let block = match layer {
                Where::Draining => &mut draining_block,
                Where::Current => &mut current_block,
            };
            block.fragment.utxo.consume(input.clone());
        }

        let mut db = VolatileDB::default();
        db.push_back(draining_block);
        db.transition();
        db.push_back(current_block);

        assert_eq!(db.resolve_input(&input).is_some(), resolvable);
        assert_eq!(db.has_consumed_input(&input), consumed);
    }

    #[test]
    fn len_counts_both_series_until_draining_empties() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.transition();
        db.push_back(AnchoredVolatileFragment::fixture(30, 3));

        assert_eq!(db.len(), 3, "two draining blocks plus one current block");
        assert!(!db.is_empty());

        db.pop_front();
        assert_eq!(db.len(), 2);

        db.pop_front();
        assert_eq!(db.len(), 1, "draining drained empty; only current is counted");
        assert!(db.draining.is_empty());
        assert!(!db.is_empty());

        db.pop_front();
        assert!(db.is_empty());
        assert_eq!(db.len(), 0);
    }

    proptest! {
        #[test]
        fn db_resolve_matches_naive_walk_over_both_series(
            diffs in unique_lifecycle_diffs(VOLATILE_WINDOW),
            transition_after in 1usize..VOLATILE_WINDOW,
        ) {
            let mut db = VolatileDB::default();
            for (index, diff) in diffs.iter().enumerate() {
                if index == transition_after {
                    db.transition();
                }
                let mut anchored = AnchoredVolatileFragment::fixture(index as u64, index as u8);
                anchored.fragment.utxo = diff.clone();
                db.push_back(anchored);
            }

            for tag in 0u8..16 {
                let input = test_input(tag);
                prop_assert_eq!(db.resolve_input(&input).is_some(), naive_resolve(&diffs, &input).is_some());
                prop_assert_eq!(db.has_consumed_input(&input), naive_has_consumed(&diffs, &input));
            }
        }
    }

    // HELPERS

    #[derive(Clone, Copy)]
    enum Where {
        Draining,
        Current,
    }
}
