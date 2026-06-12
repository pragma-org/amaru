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

use amaru_kernel::{MemoizedTransactionOutput, Point, TransactionInput};

use crate::state::{AnchoredVolatileFragment, VolatileFragment, volatile::VolatileStore};

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
        self.aggregate.utxo.produced.get(input)
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
    use amaru_kernel::{Hash, Slot};
    use proptest::prelude::*;

    use super::*;
    use crate::{state::diff_set::DiffSet, tests::fake_output};

    const VOLATILE_WINDOW: usize = 6;

    prop_compose! {
        fn unique_lifecycle_diffs()(
            plan in prop::collection::btree_map(
                0u8..16,
                (0usize..VOLATILE_WINDOW, prop::option::of(0usize..VOLATILE_WINDOW)),
                0..16,
            )
        ) -> Vec<DiffSet<TransactionInput, MemoizedTransactionOutput>> {
            let mut diffs: Vec<DiffSet<TransactionInput, MemoizedTransactionOutput>> =
                (0..VOLATILE_WINDOW).map(|_| DiffSet::default()).collect();

            for (tag, (produced_at, consume_offset)) in plan {
                diffs[produced_at].produce(test_input(tag), fixed_output());
                if let Some(offset) = consume_offset {
                    let consumed_at = produced_at + 1 + offset;
                    if consumed_at < VOLATILE_WINDOW {
                        diffs[consumed_at].consume(test_input(tag));
                    }
                }
            }

            diffs
        }
    }

    fn series_from(diffs: &[DiffSet<TransactionInput, MemoizedTransactionOutput>]) -> VolatileSeries {
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

    fn test_input(tag: u8) -> TransactionInput {
        TransactionInput { transaction_id: Hash::new([tag; 32]), index: 0 }
    }

    fn fixed_output() -> MemoizedTransactionOutput {
        fake_output("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335")
    }

    fn naive_resolve<'a>(
        diffs: &'a [DiffSet<TransactionInput, MemoizedTransactionOutput>],
        input: &TransactionInput,
    ) -> Option<&'a MemoizedTransactionOutput> {
        for diff in diffs.iter().rev() {
            if diff.consumed.contains(input) {
                return None;
            }
            if let Some(output) = diff.produced.get(input) {
                return Some(output);
            }
        }
        None
    }

    fn naive_has_consumed(
        diffs: &[DiffSet<TransactionInput, MemoizedTransactionOutput>],
        input: &TransactionInput,
    ) -> bool {
        diffs.iter().any(|diff| diff.consumed.contains(input))
    }
}
