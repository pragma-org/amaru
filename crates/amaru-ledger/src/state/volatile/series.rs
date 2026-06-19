// Copyright 2026 PRAGMA
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

use std::collections::VecDeque;

use amaru_kernel::{MemoizedTransactionOutput, Point, PoolId, StakeCredential, TransactionInput};

use crate::state::{
    AnchoredVolatileFragment, VolatileFragment,
    volatile::{
        AccountBind, Existence, VolatileStore,
        fragment::{CommitteeBind, DRepBind},
    },
};

#[derive(Default)]
pub struct VolatileSeries {
    sequence: VecDeque<AnchoredVolatileFragment>,
    aggregate: VolatileFragment,
}

impl VolatileStore for VolatileSeries {
    fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    fn len(&self) -> usize {
        self.sequence.len()
    }

    fn view_back(&self) -> Option<&AnchoredVolatileFragment> {
        self.sequence.back()
    }

    fn view_front(&self) -> Option<&AnchoredVolatileFragment> {
        self.sequence.front()
    }

    fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.aggregate.utxo.produced.get(input).map(|output| output.as_ref())
    }

    fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.aggregate.utxo.consumed.contains(input)
    }

    fn contains(&self, point: &Point) -> bool {
        self.sequence.binary_search_by_key(point, |anchored| anchored.point()).is_ok()
    }

    fn pop_front(&mut self) -> Option<AnchoredVolatileFragment> {
        let popped = self.sequence.pop_front();
        if popped.is_some() {
            self.recompute_aggregate();
        }
        popped
    }

    fn push_back(&mut self, fragment: AnchoredVolatileFragment) {
        self.aggregate.compose(&fragment.fragment);
        self.sequence.push_back(fragment);
    }

    fn rollback_to<'a>(&mut self, point: &'a Point) -> Result<(), &'a Point> {
        let ix = self.sequence.binary_search_by_key(point, |anchored| anchored.point()).map_err(|_| point)?;

        self.sequence.truncate(ix + 1);

        self.recompute_aggregate();
        Ok(())
    }

    fn clear(&mut self) {
        self.sequence.clear();
        self.aggregate = Default::default();
    }

    fn iter(&self) -> impl Iterator<Item = &AnchoredVolatileFragment> {
        self.sequence.iter()
    }
}

impl VolatileSeries {
    /// Whether the given pool is registered (or re-registered) anywhere in this series' aggregate.
    /// Deferred retirements do not affect this; reaping is handled one level up, in the volatile DB.
    pub fn pool_exists(&self, pool_id: &PoolId) -> bool {
        self.aggregate.pool_exists(pool_id)
    }

    /// This series' verdict on a stake account, read off its aggregate.
    pub fn resolve_account(&self, credential: &StakeCredential) -> Existence<AccountBind> {
        self.aggregate.resolve_account(credential)
    }

    /// This series' verdict on a DRep account, read off its aggregate.
    pub fn resolve_drep(&self, credential: &StakeCredential) -> Existence<DRepBind> {
        self.aggregate.resolve_drep(credential)
    }

    /// This series' verdict on a CC member, read off its aggregate.
    pub fn resolve_committee(&self, credential: &StakeCredential) -> Existence<CommitteeBind> {
        self.aggregate.resolve_committee(credential)
    }

    /// Whether this series withdrew the account's rewards anywhere in its aggregate.
    pub fn withdrew(&self, credential: &StakeCredential) -> bool {
        self.aggregate.withdrew(credential)
    }

    fn recompute_aggregate(&mut self) {
        let mut aggregate = VolatileFragment::default();
        for anchored in &self.sequence {
            aggregate.compose(&anchored.fragment);
        }

        self.aggregate = aggregate;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{Hash, Slot};
    use proptest::prelude::*;

    use super::*;
    use crate::state::{diff_set::DiffSet, volatile::test_support::*};

    fn series_from(diffs: &[DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>]) -> VolatileSeries {
        let mut series = VolatileSeries::default();
        for (index, diff) in diffs.iter().enumerate() {
            let mut anchored = AnchoredVolatileFragment::fixture(index as u64, index as u8);
            anchored.fragment.utxo = diff.clone();
            series.push_back(anchored);
        }
        series
    }

    proptest! {
        #[test]
        fn aggregated_lookups_match_naive_walk(diffs in unique_lifecycle_diffs()) {
            let series = series_from(&diffs);
            for tag in 0u8..16 {
                let input = test_input(tag);
                prop_assert_eq!(series.resolve_input(&input).is_some(), naive_resolve(&diffs, &input).is_some());
                prop_assert_eq!(series.has_consumed_input(&input), naive_has_consumed(&diffs, &input));
            }
        }
    }

    proptest! {
        #[test]
        fn aggregated_lookups_match_naive_walk_after_stabilization(diffs in unique_lifecycle_diffs()) {
            let mut series = series_from(&diffs);
            series.pop_front();
            let remaining = &diffs[1..];

            for tag in 0u8..16 {
                let input = test_input(tag);
                prop_assert_eq!(series.resolve_input(&input).is_some(), naive_resolve(remaining, &input).is_some());
                prop_assert_eq!(series.has_consumed_input(&input), naive_has_consumed(remaining, &input));
            }
        }
    }

    proptest! {
        #[test]
        fn aggregated_lookups_match_naive_walk_after_rollback(
            diffs in unique_lifecycle_diffs(),
            rollback_ix in 0usize..VOLATILE_WINDOW,
        ) {
            let mut series = series_from(&diffs);

            let point = Point::Specific(Slot::from(rollback_ix as u64), Hash::new([0u8; 32]));
            series.rollback_to(&point).unwrap();

            let remaining = &diffs[..=rollback_ix];

            for tag in 0u8..16 {
                let input = test_input(tag);
                prop_assert_eq!(series.resolve_input(&input).is_some(), naive_resolve(remaining, &input).is_some());
                prop_assert_eq!(series.has_consumed_input(&input), naive_has_consumed(remaining, &input));
            }
        }
    }

    proptest! {
        #[test]
        fn series_is_ordered_front_to_back(len in 1usize..12) {
            let mut series = VolatileSeries::default();
            for i in 0..len {
                series.push_back(AnchoredVolatileFragment::fixture((i as u64 + 1) * 10, i as u8));
            }

            let slots = series.iter().map(|anchored| anchored.slot()).collect::<Vec<_>>();
            let mut sorted = slots.clone();
            sorted.sort();

            prop_assert_eq!(&slots, &sorted);
            prop_assert_eq!(series.view_front().map(|anchored| anchored.slot()), slots.first().copied());
            prop_assert_eq!(series.view_back().map(|anchored| anchored.slot()), slots.last().copied());
        }
    }

    #[test]
    fn len_and_is_empty_track_the_sequence() {
        let mut series = VolatileSeries::default();
        assert!(series.is_empty());
        assert_eq!(series.len(), 0);

        series.push_back(AnchoredVolatileFragment::fixture(10, 1));
        series.push_back(AnchoredVolatileFragment::fixture(20, 2));
        assert!(!series.is_empty());
        assert_eq!(series.len(), 2);

        series.pop_front();
        assert_eq!(series.len(), 1);

        series.pop_front();
        assert!(series.is_empty());
        assert_eq!(series.len(), 0);
    }
}
