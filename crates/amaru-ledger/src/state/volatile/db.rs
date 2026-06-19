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

use amaru_kernel::{Epoch, Lovelace, MemoizedTransactionOutput, Point, PoolId, StakeCredential, TransactionInput};

use crate::state::{
    AnchoredVolatileFragment,
    overlay::StateOverlay,
    volatile::{AccountBind, CommitteeBind, DRepBind, Existence, VolatileSeries, VolatileStore},
};

/// Pools need existence only. `Gone` means reaped at the boundary.
pub type PoolExistence = Existence<()>;

#[derive(Default)]
pub struct VolatileDB {
    current: VolatileSeries,
    draining: Option<VolatileSeries>,
    overlay: StateOverlay,
}

impl VolatileStore for VolatileDB {
    fn is_empty(&self) -> bool {
        self.current.is_empty() && self.draining.as_ref().is_none_or(|draining| draining.is_empty())
    }

    fn len(&self) -> usize {
        self.current.len() + self.draining.as_ref().map(|draining| draining.len()).unwrap_or_default()
    }

    fn view_back(&self) -> Option<&AnchoredVolatileFragment> {
        self.current.view_back().or(self.draining.as_ref().and_then(|draining| draining.view_back()))
    }

    fn view_front(&self) -> Option<&AnchoredVolatileFragment> {
        self.draining.as_ref().and_then(|draining| draining.view_front()).or(self.current.view_front())
    }

    fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        if self.has_consumed_input(input) {
            return None;
        }

        self.current.resolve_input(input).or(self.draining.as_ref().and_then(|draining| draining.resolve_input(input)))
    }

    fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.current.has_consumed_input(input)
            || self.draining.as_ref().is_some_and(|draining| draining.has_consumed_input(input))
    }

    fn contains(&self, point: &Point) -> bool {
        self.current.contains(point) || self.draining.as_ref().is_some_and(|draining| draining.contains(point))
    }

    fn pop_front(&mut self) -> Option<AnchoredVolatileFragment> {
        if let Some(draining) = self.draining.as_mut() {
            let popped = draining.pop_front();
            if draining.is_empty() {
                self.draining = None;
            }
            return popped.or_else(|| self.current.pop_front());
        }

        self.current.pop_front()
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
        if let Some(first) =
            self.draining.as_ref().and_then(|draining| draining.view_front()).or(self.current.view_front())
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

        if self.current.contains(point) {
            self.current.rollback_to(point)?;
            return Ok(());
        }

        if self.draining.as_ref().is_some_and(|draining| draining.contains(point)) {
            // If we are rolling back to a point in the draining sequence, we need to
            // promote the remaining sequence (after truncating) to current.
            let mut draining = self.draining.take().expect("draining is Some");

            draining.rollback_to(point)?;

            self.current = draining;

            return Ok(());
        }

        Err(point)
    }

    fn clear(&mut self) {
        self.current.clear();
        self.draining = None;
    }

    fn iter(&self) -> impl Iterator<Item = &AnchoredVolatileFragment> {
        self.draining.as_ref().map(|d| d.iter()).into_iter().flatten().chain(self.current.iter())
    }
}

impl VolatileDB {
    /// Construct an empty volatile DB whose overlay is anchored to the given epoch.
    pub fn new(epoch: Epoch) -> Self {
        Self { current: VolatileSeries::default(), draining: None, overlay: StateOverlay::new(epoch) }
    }

    /// A read-only handle on the epoch-transition overlay.
    pub fn overlay(&self) -> &StateOverlay {
        &self.overlay
    }

    /// A mutable handle on the epoch-transition overlay.
    pub fn overlay_mut(&mut self) -> &mut StateOverlay {
        &mut self.overlay
    }

    /// Whether a pool exists per the volatile state, precedence `current -> overlay (reaping) ->
    /// draining`. A re-registration in `current` cancels a boundary reaping; otherwise a reaping is
    /// `Gone`. `Unknown` means consult the stable store.
    pub fn has_pool(&self, pool_id: &PoolId) -> PoolExistence {
        if self.current.pool_exists(pool_id) {
            Existence::Exists(())
        } else if self.overlay.is_pool_retired(pool_id) {
            Existence::Gone
        } else if self.draining.as_ref().is_some_and(|draining| draining.pool_exists(pool_id)) {
            Existence::Exists(())
        } else {
            Existence::Unknown
        }
    }

    /// Resolve a stake account across the volatile layers, precedence `current -> draining`. A `Gone`
    /// from `current` short-circuits; a fresh re-registration supersedes the closing epoch, a
    /// bind-only update layers over it.
    pub fn resolve_account(&self, credential: &StakeCredential) -> Existence<AccountBind> {
        self.current.resolve_account(credential).layer_over(|| {
            self.draining.as_ref().map_or(Existence::Unknown, |draining| draining.resolve_account(credential))
        })
    }

    /// An account's withdrawable balance from its `base` (stable rewards, or `0` if freshly
    /// registered in the window): add the overlay's pending boundary credit, but a volatile
    /// withdrawal zeroes it.
    pub fn resolve_reward_balance(&self, credential: &StakeCredential, base: Lovelace) -> Lovelace {
        let credit = self.overlay.pending_reward_credit(credential);
        if self.current.withdrew(credential) {
            0 // withdrawn after the boundary credit
        } else if self.draining.as_ref().is_some_and(|draining| draining.withdrew(credential)) {
            credit // withdrawn before the boundary credit
        } else {
            base + credit
        }
    }

    /// Resolve a DRep across the volatile layers, precedence `current -> draining`. A `Gone`
    /// from `current` short-circuits; a fresh re-registration supersedes the closing epoch, a
    /// bind-only update layers over it.
    pub fn resolve_drep(&self, credential: &StakeCredential) -> Existence<DRepBind> {
        self.current.resolve_drep(credential).layer_over(|| {
            self.draining.as_ref().map_or(Existence::Unknown, |draining| draining.resolve_drep(credential))
        })
    }

    /// Resolve a CC member across the volatile layers, precedence `current -> overlay (enactment) ->
    /// draining`. A boundary add/remove sits above the closing epoch but below the new epoch's
    /// blocks, mirroring pool reaping. `Unknown` means consult the stable store.
    pub fn resolve_committee(&self, credential: &StakeCredential) -> Existence<CommitteeBind> {
        self.current.resolve_committee(credential).layer_over(|| {
            self.overlay.committee_verdict(credential).layer_over(|| {
                self.draining.as_ref().map_or(Existence::Unknown, |draining| draining.resolve_committee(credential))
            })
        })
    }

    /// A CC member's term from its `base` (stable or freshly added): the overlay's pending term wins
    /// during the straddle, otherwise the base flows through.
    pub fn resolve_committee_term(&self, credential: &StakeCredential, base: Option<Epoch>) -> Option<Epoch> {
        self.overlay.pending_committee_term(credential).unwrap_or(base)
    }

    /// Seal the live series at an epoch boundary: the `current` series, which, by the protocol
    /// pre-condition, holds only the closing epoch's blocks, becomes the `draining` series, and a
    /// fresh empty `current` is opened for the new epoch. This keeps each series epoch-homogeneous.
    ///
    /// No-op when `current` is empty: there is nothing to seal, `draining` stays `None`, and
    /// homogeneity still holds because an empty `current` only ever takes new-epoch blocks.
    ///
    /// The `assert!` confirms that at most one  epoch boundary is ever inside the window
    /// and any prior `draining` series has fully drained long before the next boundary arrives.
    /// A violation would mean two boundaries inside the window, impossible under the protocol.
    pub fn seal(&mut self) {
        assert!(
            self.draining.is_none(),
            "sealing while a draining series is still present; two epoch boundaries inside the k-block window?"
        );

        if self.current.is_empty() {
            return;
        }

        self.draining = Some(mem::take(&mut self.current));
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
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };

    use amaru_kernel::{Epoch, Hash, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, Point, Slot, StakeCredential};
    use num::Zero;
    use proptest::prelude::*;
    use test_case::test_case;

    use super::*;
    use crate::{
        epoch_transition::{
            Computed, Effective, GovernanceUpdates, PoolsEpochTransitionUpdates, Rewards, RewardsState,
        },
        governance::ratification::{CommitteeUpdate, ProposalsRoots},
        state::volatile::test_support::*,
        summary::SafeRatio,
    };

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
        db.seal();
        assert!(db.draining.is_some() && db.current.is_empty());

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
    fn seal_opens_draining_and_resets_current() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));

        db.seal();

        assert!(db.draining.is_some(), "draining should hold the sealed series");
        assert_eq!(db.current.len(), 0, "current should be reset to empty");
        assert_eq!(db.len(), 2, "total length is unchanged by sealing");
    }

    #[test]
    fn seal_is_a_noop_on_empty_current() {
        let mut db = VolatileDB::default();

        db.seal();

        assert!(db.draining.is_none(), "sealing an empty current must not open a draining series");
    }

    #[test]
    #[should_panic(expected = "two epoch boundaries")]
    fn seal_panics_if_draining_already_present() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.seal();

        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.seal();
    }

    #[test]
    fn pop_front_drains_draining_then_nulls_it() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.seal();
        db.push_back(AnchoredVolatileFragment::fixture(30, 3));

        assert_eq!(db.pop_front().map(|fragment| fragment.slot()), Some(Slot::from(10)));
        assert!(db.draining.is_some(), "draining still holds one block");

        assert_eq!(db.pop_front().map(|fragment| fragment.slot()), Some(Slot::from(20)));
        assert!(db.draining.is_none(), "draining is nulled once it empties");

        assert_eq!(db.pop_front().map(|fragment| fragment.slot()), Some(Slot::from(30)));
        assert!(db.is_empty());
    }

    #[test]
    fn cross_series_views_span_both_series_oldest_to_newest() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.seal();
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
        db.seal();
        db.push_back(current_block);

        assert_eq!(db.resolve_input(&input).is_some(), resolvable);
        assert_eq!(db.has_consumed_input(&input), consumed);
    }

    #[test]
    fn len_counts_both_series_until_draining_empties() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.seal();
        db.push_back(AnchoredVolatileFragment::fixture(30, 3));

        assert_eq!(db.len(), 3, "two draining blocks plus one current block");
        assert!(!db.is_empty());

        db.pop_front();
        assert_eq!(db.len(), 2);

        db.pop_front();
        assert_eq!(db.len(), 1, "draining drained empty; only current is counted");
        assert!(db.draining.is_none());
        assert!(!db.is_empty());

        db.pop_front();
        assert!(db.is_empty());
        assert_eq!(db.len(), 0);
    }

    proptest! {
        #[test]
        fn db_resolve_matches_naive_walk_over_both_series(
            diffs in unique_lifecycle_diffs(),
            seal_after in 1usize..VOLATILE_WINDOW,
        ) {
            let mut db = VolatileDB::default();
            for (index, diff) in diffs.iter().enumerate() {
                if index == seal_after {
                    db.seal();
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

    #[test_case(None, Some(Act::Reg) => Expect::Registered ; "registered in current")]
    #[test_case(None, Some(Act::Unreg) => Expect::Gone ; "deregistered in current")]
    #[test_case(Some(Act::Reg), None => Expect::Registered ; "registered in draining, untouched in current")]
    #[test_case(Some(Act::Unreg), None => Expect::Gone ; "deregistered in draining shadows the stable store")]
    #[test_case(Some(Act::Reg), Some(Act::Unreg) => Expect::Gone ; "current deregistration overrides draining")]
    #[test_case(Some(Act::Unreg), Some(Act::Reg) => Expect::Registered ; "current re-registration cancels draining tombstone")]
    #[test_case(None, None => Expect::Unknown ; "untouched everywhere defers to the stable store")]
    fn resolve_account_precedence(draining: Option<Act>, current: Option<Act>) -> Expect {
        let mut db = VolatileDB::default();
        if let Some(act) = draining {
            db.push_back(account_block(10, act));
        }
        db.seal();
        if let Some(act) = current {
            db.push_back(account_block(20, act));
        }

        match db.resolve_account(&cred(1)) {
            Existence::Exists(_) => Expect::Registered,
            Existence::Gone => Expect::Gone,
            Existence::Unknown => Expect::Unknown,
        }
    }

    #[test]
    fn reward_balance_is_zeroed_by_a_volatile_withdrawal() {
        // No withdrawal and no pending overlay credit: the base flows through untouched.
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        assert_eq!(db.resolve_reward_balance(&cred(1), 100), 100);

        // A withdrawal in `current` (post-boundary) zeroes the balance.
        let mut db = VolatileDB::default();
        db.push_back(withdrawal_block(10));
        assert_eq!(db.resolve_reward_balance(&cred(1), 100), 0);

        // A withdrawal in `draining` (pre-boundary) with no pending credit also leaves nothing.
        let mut db = VolatileDB::default();
        db.push_back(withdrawal_block(10));
        db.seal();
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        assert_eq!(db.resolve_reward_balance(&cred(1), 100), 0);
    }

    #[test]
    fn reward_balance_folds_in_the_pending_overlay_credit_during_the_straddle() {
        let mut db = VolatileDB::default();
        let computed = Rewards::<Computed>::new(0, 0, BTreeMap::from([(cred(1), 5_000_000)]));
        let effective = Rewards::<Effective>::new(computed, std::iter::once(cred(1)));
        *db.overlay_mut().rewards_mut() = RewardsState::Effective(effective);

        // The pending boundary credit is added on top of the stable base.
        assert_eq!(db.resolve_reward_balance(&cred(1), 100), 5_000_100);

        // Withdrawn in `draining`, before the boundary credit: only the credit remains.
        db.push_back(withdrawal_block(10));
        db.seal();
        assert_eq!(db.resolve_reward_balance(&cred(1), 100), 5_000_000);

        // Withdrawn again in `current`, after the boundary credit: nothing remains.
        db.push_back(withdrawal_block(20));
        assert_eq!(db.resolve_reward_balance(&cred(1), 100), 0);
    }

    #[test_case(None, Some(CommitteeAct::Auth) => Expect::Registered ; "hot-auth in current")]
    #[test_case(None, Some(CommitteeAct::Resign) => Expect::Gone ; "resigned in current")]
    #[test_case(Some(CommitteeAct::Auth), None => Expect::Registered ; "hot-auth in draining, untouched in current")]
    #[test_case(Some(CommitteeAct::Resign), None => Expect::Gone ; "resigned in draining shadows the stable store")]
    #[test_case(Some(CommitteeAct::Auth), Some(CommitteeAct::Resign) => Expect::Gone ; "current resignation overrides draining")]
    #[test_case(None, None => Expect::Unknown ; "untouched everywhere defers to the stable store")]
    fn resolve_committee_precedence(draining: Option<CommitteeAct>, current: Option<CommitteeAct>) -> Expect {
        let mut db = VolatileDB::default();
        if let Some(act) = draining {
            db.push_back(committee_block(10, act));
        }
        db.seal();
        if let Some(act) = current {
            db.push_back(committee_block(20, act));
        }

        match db.resolve_committee(&cred(1)) {
            Existence::Exists(_) => Expect::Registered,
            Existence::Gone => Expect::Gone,
            Existence::Unknown => Expect::Unknown,
        }
    }

    #[test]
    fn resolve_committee_reflects_the_pending_boundary() {
        // Added at the boundary: a fresh member with the pending term, no stable row needed.
        let mut db = VolatileDB::default();
        db.overlay_mut().transition(
            None,
            PoolsEpochTransitionUpdates::default(),
            committee_update(Some(CommitteeUpdate::ChangeMembers {
                added: BTreeMap::from([(cred(1), Epoch::from(99))]),
                removed: BTreeSet::new(),
                threshold: SafeRatio::zero(),
            })),
        );
        assert!(matches!(db.resolve_committee(&cred(1)), Existence::Exists(_)));
        assert_eq!(db.resolve_committee_term(&cred(1), None), Some(Epoch::from(99)));

        // Removed at the boundary: a tombstone that shadows the stale stable entry.
        let mut db = VolatileDB::default();
        db.overlay_mut().transition(
            None,
            PoolsEpochTransitionUpdates::default(),
            committee_update(Some(CommitteeUpdate::ChangeMembers {
                added: BTreeMap::new(),
                removed: BTreeSet::from([cred(1)]),
                threshold: SafeRatio::zero(),
            })),
        );
        assert!(matches!(db.resolve_committee(&cred(1)), Existence::Gone));

        // No-confidence keeps members, so membership defers down, but the term goes inactive.
        let mut db = VolatileDB::default();
        db.overlay_mut().transition(
            None,
            PoolsEpochTransitionUpdates::default(),
            committee_update(Some(CommitteeUpdate::NoConfidence)),
        );
        assert!(matches!(db.resolve_committee(&cred(1)), Existence::Unknown));
        assert_eq!(db.resolve_committee_term(&cred(1), Some(Epoch::from(50))), None);
    }

    // HELPERS

    #[derive(Clone, Copy)]
    enum Where {
        Draining,
        Current,
    }

    #[derive(Clone, Copy)]
    enum CommitteeAct {
        Auth,
        Resign,
    }

    #[derive(Clone, Copy)]
    enum Act {
        Reg,
        Unreg,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Expect {
        Registered,
        Gone,
        Unknown,
    }

    fn cred(tag: u8) -> StakeCredential {
        StakeCredential::AddrKeyhash(Hash::new([tag; 28]))
    }

    fn account_block(slot: u64, act: Act) -> AnchoredVolatileFragment {
        let mut block = AnchoredVolatileFragment::fixture(slot, slot as u8);
        match act {
            Act::Reg => block.fragment.accounts.register(cred(1), 2_000_000, None, None).unwrap(),
            Act::Unreg => block.fragment.accounts.unregister(cred(1)),
        }
        block
    }

    fn withdrawal_block(slot: u64) -> AnchoredVolatileFragment {
        let mut block = AnchoredVolatileFragment::fixture(slot, slot as u8);
        block.fragment.withdrawals.insert(cred(1));
        block
    }

    fn committee_block(slot: u64, act: CommitteeAct) -> AnchoredVolatileFragment {
        let mut block = AnchoredVolatileFragment::fixture(slot, slot as u8);
        match act {
            CommitteeAct::Auth => block.fragment.committee.bind_left(cred(1), Some(cred(2))).unwrap(),
            CommitteeAct::Resign => block.fragment.committee.unregister(cred(1)),
        }
        block
    }

    fn committee_update(committee: Option<CommitteeUpdate>) -> GovernanceUpdates {
        GovernanceUpdates {
            roots: ProposalsRoots {
                protocol_parameters: None,
                hard_fork: None,
                constitutional_committee: None,
                constitution: None,
            },
            protocol_parameters: PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            pruned_proposals: BTreeSet::new(),
            payouts: BTreeMap::new(),
            is_dormant_epoch: false,
            constitutional_committee: committee,
            new_constitution: None,
        }
    }
}
