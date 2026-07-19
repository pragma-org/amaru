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

use amaru_kernel::{
    ComparableProposalId, DRepRegistration, MemoizedTransactionOutput, Point, PoolId, StakeCredential, TransactionInput,
};
use amaru_observability::debug_span;

use crate::state::{
    AnchoredVolatileFragment,
    diff_bind::DiffBind,
    volatile::{AccountBind, CommitteeMemberBind, Existence, VolatileAggregate, VolatileSequence, VolatileState},
};

/// Number of blocks after which, if no rollback has been observed, we forcefully re-compute the
/// aggregate. This number is chosen 'arbitrarily' but with a few considerations:
///
/// 1. We don't want the memory footprint of the volatile to grow *too much*. 100MB appears as a
///    good arbitrary upper-bound. At least, that's one dimension we have freedom to decide on.
///
/// 2. We make a gross approximation that 1 block equals 90KB (i.e. the maximum block size) of
///    memory allocation. In practice, it is far less since blocks contain a variety of
///    informations that we do not store here, but it seems like a reasonable limit again. Plus, we
///    do cleanup UTxOs, votes, and a couple of other things to reduce the growth.
///
/// 3. From there; we know that slot battles are "frequent enough" that they occurs somewhere
///    around every ~13 minutes on average. Or said differently, they occur every 40 blocks on
///    average. These don't necessarily lead to an observable rollback, but that gives a lower
///    bound.
///
/// 4. We use a safe margin on top of what was described in (3) for two reasons: it gives some
///    safety net in case where the block size would change due to a protocol parameter update
///    (although we do expect to have time to notify our users about hardware requirements increase
///    if that ever occurs); but also, it prevents the forced recompute to occur too often when
///    syncing, since no rollbacks can be observed during that time.
///
/// So putting it all together; using 1080 means that the memory footprint of the volatile
/// shouldn't grow beyond ~100MB. There should also be ~27 slot battles during that timeframe and
/// it is highly likely that *at least one* would lead to an observable rollback. Finally, when
/// syncing, the impact should be negligible as only one block every 1080 would cause an aggregate
/// recompute.
const DEFAULT_FORCED_RECOMPUTE_IN: usize = 4096;

#[derive(Debug)]
#[cfg_attr(feature = "test-utils", derive(Clone))]
pub struct VolatileSeries {
    forced_recompute_in: usize,
    sequence: VecDeque<AnchoredVolatileFragment>,
    aggregate: VolatileAggregate,
}

impl Default for VolatileSeries {
    fn default() -> Self {
        Self {
            forced_recompute_in: DEFAULT_FORCED_RECOMPUTE_IN,
            sequence: Default::default(),
            aggregate: Default::default(),
        }
    }
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
        self.aggregate.resolve_account(credential, || {
            DiffBind::fold(self.iter().map(|anchored| &anchored.fragment.accounts)).to_owned()
        })
    }

    fn has_withdrawal(&self, credential: &StakeCredential) -> bool {
        self.aggregate.has_withdrawal(credential)
    }

    // --------------------------------------------------------------------------------------- DReps
    type DRep = Existence<DRepRegistration>;
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
        } else if self.forced_recompute_in == 0 {
            self.new_aggregate()
        } else {
            self.aggregate.remove_fragment(&popped.fragment);
        }
        Some(popped)
    }

    fn push_back(&mut self, item: Self::Item) {
        self.forced_recompute_in = self.forced_recompute_in.saturating_sub(1);
        self.aggregate.add_fragment(&item.fragment);
        self.sequence.push_back(item);
    }
}

impl VolatileSeries {
    fn new_aggregate(&mut self) {
        self.forced_recompute_in = DEFAULT_FORCED_RECOMPUTE_IN;
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
        Self {
            sequence: mem::take(&mut self.sequence),
            aggregate: mem::take(&mut self.aggregate),
            forced_recompute_in: self.forced_recompute_in,
        }
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
        fn aggregated_lookups_match_naive_walk(diffs in unique_lifecycle_diffs(VOLATILE_WINDOW)) {
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
        fn aggregated_lookups_match_naive_walk_after_stabilization(diffs in unique_lifecycle_diffs(VOLATILE_WINDOW)) {
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
            diffs in unique_lifecycle_diffs(VOLATILE_WINDOW),
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
