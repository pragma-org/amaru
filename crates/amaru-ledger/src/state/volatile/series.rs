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

use std::{collections::VecDeque, mem};

use amaru_kernel::{ComparableProposalId, MemoizedTransactionOutput, Point, PoolId, StakeCredential, TransactionInput};
use amaru_observability::debug_span;

use crate::state::{
    AnchoredVolatileFragment,
    volatile::{
        AccountBind, CommitteeMemberBind, DRepBind, Existence, VolatileAggregate, VolatileSequence, VolatileState,
    },
};

#[derive(Debug, Default)]
#[cfg_attr(feature = "test-utils", derive(Clone))]
pub struct VolatileSeries {
    sequence: VecDeque<AnchoredVolatileFragment>,
    aggregate: VolatileAggregate,
}

impl VolatileState for VolatileSeries {
    // --------------------------------------------------------------------------------------- UTxOs
    fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.aggregate.resolve_input(input)
    }

    fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.aggregate.has_consumed_input(input)
    }

    // --------------------------------------------------------------------------------------- Pools
    type Pool = bool;
    fn resolve_pool(&self, pool_id: PoolId) -> Self::Pool {
        // Whether the given pool is registered (or re-registered) anywhere in this series' aggregate.
        // Deferred retirements do not affect this; reaping is handled one level up, in the volatile DB.
        self.aggregate.resolve_pool(pool_id)
    }

    // ------------------------------------------------------------------------------------ Accounts
    type Account = Existence<AccountBind>;
    fn resolve_account(&self, credential: &StakeCredential) -> Self::Account {
        self.aggregate.resolve_account(credential)
    }

    fn has_withdrawal(&self, credential: &StakeCredential) -> bool {
        self.aggregate.has_withdrawal(credential)
    }

    // --------------------------------------------------------------------------------------- DReps
    type DRep = Existence<DRepBind>;
    fn resolve_drep(&self, credential: &StakeCredential) -> Self::DRep {
        self.aggregate.resolve_drep(credential)
    }

    // ----------------------------------------------------------------------------------- CCMembers
    type CCMember = Existence<CommitteeMemberBind>;
    fn resolve_cc_member(&self, credential: &StakeCredential) -> Self::CCMember {
        self.aggregate.resolve_cc_member(credential)
    }

    // ----------------------------------------------------------------------------------- Proposals
    type Proposal = Existence<()>;
    fn resolve_proposal(&self, id: &ComparableProposalId) -> Self::Proposal {
        self.aggregate.resolve_proposal(id)
    }
}

impl VolatileSequence for VolatileSeries {
    type Item = AnchoredVolatileFragment;

    fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    fn len(&self) -> usize {
        self.sequence.len()
    }

    fn view_back(&self) -> Option<&Self::Item> {
        self.sequence.back()
    }

    fn view_front(&self) -> Option<&Self::Item> {
        self.sequence.front()
    }

    fn has_point(&self, point: &Point) -> bool {
        self.sequence.binary_search_by_key(point, |anchored| anchored.point()).is_ok()
    }

    fn iter(&self) -> impl Iterator<Item = &Self::Item> {
        self.sequence.iter()
    }

    fn into_iter(self) -> impl Iterator<Item = Self::Item> {
        self.sequence.into_iter()
    }

    fn pop_front(&mut self) -> Option<Self::Item> {
        let popped = self.sequence.pop_front()?;
        if self.sequence.is_empty() {
            self.aggregate = VolatileAggregate::default();
        } else {
            self.aggregate.remove_fragment(&popped.fragment);
        }
        Some(popped)
    }

    fn push_back(&mut self, item: Self::Item) {
        self.aggregate.add_fragment(&item.fragment);
        self.sequence.push_back(item);
    }
}

impl VolatileSeries {
    /// Rebuild the aggregate from scratch by re-folding the surviving sequence. Only rollback uses
    /// this; stabilization retracts a single fragment off the front exactly and incrementally (see
    /// [`VolatileAggregate::remove_fragment`]).
    ///
    /// Rollback *could* be incremental too, peel the discarded tail newest-first, the mirror of
    /// stabilization, but that would need an exact back-removal on every field, and `utxo` doesn't
    /// have one. It is a collapsed [`crate::state::volatile::DiffSet`]: when a later fragment
    /// consumes an input an earlier one produced, the collapse discards that produced value
    /// outright. Retracting the oldest fragment never needs it back (nothing earlier remains), but
    /// retracting the newest would have to restore it, and it is gone. A rollback discards a whole
    /// suffix at once and fires relatively infrequently, so re-folding it is cheap and obviously
    /// correct; the exact incremental path is reserved for stabilization, which runs on every block.
    fn new_aggregate(&mut self) {
        debug_span!(ledger::volatile::AGGREGATE).in_scope(|| {
            self.aggregate = VolatileAggregate::default();
            for anchored in &self.sequence {
                self.aggregate.add_fragment(&anchored.fragment);
            }
        });
    }

    pub fn rollback_to(&mut self, point: &Point) -> Result<VecDeque<AnchoredVolatileFragment>, String> {
        let ix = self.sequence.binary_search_by_key(point, |anchored| anchored.point()).map_err(|e| e.to_string())?;
        let recovery = self.sequence.split_off(ix + 1);
        self.new_aggregate();
        Ok(recovery)
    }

    pub fn undo_rollback(&mut self, point: &Point, fragments: VecDeque<AnchoredVolatileFragment>) {
        let ix = self
            .sequence
            .binary_search_by_key(point, |anchored| anchored.point())
            .unwrap_or_else(|e| unreachable!("failed to undo_rollback, fork point {point} is gone: {e}"));
        self.sequence.truncate(ix + 1);
        self.sequence.extend(fragments);
        self.new_aggregate();
    }

    pub fn clear(&mut self) -> Self {
        Self { sequence: mem::take(&mut self.sequence), aggregate: mem::take(&mut self.aggregate) }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{Hash, Slot};
    use proptest::prelude::*;

    use super::*;
    use crate::{state::volatile::DiffSet, tests::fake_output};

    const VOLATILE_WINDOW: usize = 6;

    fn series_from(diffs: &[DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>]) -> VolatileSeries {
        let mut series = VolatileSeries::default();
        for (index, diff) in diffs.iter().enumerate() {
            let mut anchored = AnchoredVolatileFragment::fixture(index as u64, index as u8);
            anchored.fragment.utxo = diff.clone();
            series.push_back(anchored);
        }
        series
    }

    fn test_input(tag: u8) -> TransactionInput {
        TransactionInput { transaction_id: Hash::new([tag; 32]), index: 0 }
    }

    fn fixed_output() -> MemoizedTransactionOutput {
        fake_output("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335")
    }

    prop_compose! {
        /// A window of [`DiffSet`]s where each tagged UTxO has a unique lifecycle: produced once and
        /// optionally consumed at a strictly later index. Mirrors UTxO uniqueness, so a newest-first
        /// walk has a well-defined answer for every key.
        fn any_utxo_diffset(volatile_window: usize)(
            plan in prop::collection::vec(
                (0usize..volatile_window, prop::option::of(0usize..volatile_window)),
                0..16,
            )
        ) -> Vec<DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>> {
            let mut diffs: Vec<DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>> =
                (0..volatile_window).map(|_| DiffSet::default()).collect();

            for (tag, (produced_at, consume_offset)) in plan.into_iter().enumerate() {
                diffs[produced_at].produce(test_input(tag as u8), Arc::new(fixed_output()));
                if let Some(offset) = consume_offset {
                    let consumed_at = produced_at + 1 + offset;
                    if consumed_at < volatile_window {
                        diffs[consumed_at].consume(test_input(tag as u8));
                    }
                }
            }

            diffs
        }
    }

    /// Brute-force oracle: resolve `input` by walking `diffs` newest -> oldest. First consumed -> `None`,
    /// first produce -> `Some`. The reference the maintained aggregate is checked against.
    fn naive_resolve<'a>(
        diffs: &'a [DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>],
        input: &TransactionInput,
    ) -> Option<&'a Arc<MemoizedTransactionOutput>> {
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
        diffs: &[DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>],
        input: &TransactionInput,
    ) -> bool {
        diffs.iter().any(|diff| diff.consumed.contains(input))
    }

    proptest! {
        #[test]
        fn aggregated_lookups_match_naive_walk(diffs in any_utxo_diffset(VOLATILE_WINDOW)) {
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
        fn aggregated_lookups_match_naive_walk_after_stabilization(diffs in any_utxo_diffset(VOLATILE_WINDOW)) {
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
            diffs in any_utxo_diffset(VOLATILE_WINDOW),
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
