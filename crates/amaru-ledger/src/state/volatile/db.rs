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

use amaru_kernel::{
    ComparableProposalId, Epoch, EraHistory, GlobalParameters, Lovelace, MemoizedTransactionOutput,
    PREPROD_DEFAULT_PROTOCOL_PARAMETERS, Point, PoolId, ProtocolParameters, StakeCredential, TermLimit,
    TransactionInput,
};

use crate::{
    epoch_transition::{
        Computed, Effective, GovernanceActivity, GovernanceUpdates, PoolsEpochTransitionUpdates, Rewards, RewardsState,
    },
    governance::ratification::ProposalsRoots,
    state::{
        AnchoredVolatileFragment, StateError,
        volatile::{
            AccountBind, CommitteeMemberBind, DRepBind, Existence, VolatileSequence, VolatileSeries, VolatileState,
            overlay::StateOverlay,
        },
    },
    store::{HistoricalStores, Store},
};

pub struct VolatileDB {
    /// The always active underlying volatiles series. New blocks are always added to the
    /// `current`. It represents the *most* recent part of the syncing window, but always contains
    /// block that belong to a single epoch.
    current: VolatileSeries,

    /// The tail of blocks that belong to a previous epoch. This is empty most of the time, except
    /// when the volatile is rolling into a new epoch. This is helpful to maintain one invariant:
    /// fragments always belong to the same epoch. This simplifies a lot of calculations down the
    /// line.
    draining: VolatileSeries,

    /// The volatile bits of the in-flight epoch transition (computed rewards,
    /// pending pools and governance updates). Co-located with the two series so that reads and
    /// rollback stay cohesive: the overlay is the boundary layer that sits *between* `draining`
    /// (the closing epoch) and `current` (the opening epoch).
    overlay: StateOverlay,

    /// Cached, always-present protocol parameters. This holds the current/base value; it is only
    /// ever *replaced* when the volatile overlay is flushed at an epoch boundary, never rolled
    /// back. Any in-flight *change* lives in the volatile overlay (see [`VolatileDB::overlay`]), so
    /// this must always be read through [`Self::protocol_parameters`] (which overlays the pending
    /// change) rather than via direct field access, to avoid inconsistencies.
    protocol_parameters: ProtocolParameters,

    /// Cached, always-present governance activity. Same lifecycle as `protocol_parameters`: replaced
    /// at flush, never rolled back, and read through [`Self::governance_activity`] to fold in any
    /// pending dormant-epoch bump from the volatile overlay.
    governance_activity: GovernanceActivity,
}

impl Default for VolatileDB {
    fn default() -> Self {
        Self::new(Epoch::default(), PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(), GovernanceActivity::default())
    }
}

impl VolatileState for VolatileDB {
    // --------------------------------------------------------------------------------------- UTxOs
    fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        if self.has_consumed_input(input) {
            return None;
        }

        self.current.resolve_input(input).or(self.draining.resolve_input(input))
    }

    fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.current.has_consumed_input(input) || self.draining.has_consumed_input(input)
    }

    // --------------------------------------------------------------------------------------- Pools
    type Pool = Existence<()>;
    fn resolve_pool(&self, pool_id: PoolId) -> Self::Pool {
        // Whether a pool exists per the volatile state, precedence `current -> overlay (reaping) ->
        // draining`. A re-registration in `current` cancels a boundary reaping; otherwise a reaping is
        // `Gone`. `Unknown` means consult the stable store.
        if self.current.resolve_pool(pool_id) {
            Existence::Exists(())
        } else if self.overlay.is_pool_retired(pool_id) {
            Existence::Gone
        } else if self.draining.resolve_pool(pool_id) {
            Existence::Exists(())
        } else {
            Existence::Unknown
        }
    }

    // ------------------------------------------------------------------------------------ Accounts
    type Account = (Existence<AccountBind>, RewardsAtTip);
    fn resolve_account(&self, credential: &StakeCredential) -> Self::Account {
        // Resolve a stake account across the volatile layers, precedence `current -> draining`. A `Gone`
        // from `current` short-circuits; a fresh re-registration supersedes the closing epoch, a
        // bind-only update layers over it.
        let account = self.current.resolve_account(credential).or_else(|| self.draining.resolve_account(credential));

        let rewards_at_tip = if self.current.withdrew(credential) {
            // rewards withdrawn after the boundary credit
            RewardsAtTip::Reset
        } else {
            let credit = self.overlay.pending_reward_credit(credential);
            if self.draining.withdrew(credential) {
                // rewards withdrawn before the boundary credit
                RewardsAtTip::Replace(credit)
            } else {
                // rewards not withdrawn, full bonus
                RewardsAtTip::Add(credit)
            }
        };

        (account, rewards_at_tip)
    }

    // --------------------------------------------------------------------------------------- DReps
    type DRep = Existence<DRepBind>;
    fn resolve_drep(&self, credential: &StakeCredential) -> Self::DRep {
        // Resolve a DRep across the volatile layers, precedence `current -> draining`. A `Gone`
        // from `current` short-circuits; a fresh re-registration supersedes the closing epoch, a
        // bind-only update layers over it.
        self.current.resolve_drep(credential).or_else(|| self.draining.resolve_drep(credential))
    }

    // ----------------------------------------------------------------------------------- CCMembers
    type CCMember = (Existence<CommitteeMemberBind>, Option<TermLimit>);
    fn resolve_cc_member(&self, credential: &StakeCredential) -> Self::CCMember {
        // Resolve a CC member across the volatile layers, precedence `current -> overlay (enactment) ->
        // draining`. A boundary add/remove sits above the closing epoch but below the new epoch's
        // blocks, mirroring pool reaping. `Unknown` means consult the stable store.
        let member = self.current.resolve_cc_member(credential).or_else(|| {
            self.overlay.committee_verdict(credential).or_else(|| self.draining.resolve_cc_member(credential))
        });

        let term_limit = self.overlay.pending_committee_term(credential);

        (member, term_limit)
    }

    // ----------------------------------------------------------------------------------- Proposals
    type Proposal = Existence<()>;
    /// Resolve a governance proposal across the volatile layers, precedence `current -> overlay
    /// (pruning) -> draining`. A proposal pruned at the boundary is `Gone`; `Unknown` means consult
    /// the stable store.
    fn resolve_proposal(&self, id: &ComparableProposalId) -> Self::Proposal {
        if let Existence::Exists(proposal) = self.current.resolve_proposal(id) {
            Existence::Exists(proposal)
        } else if self.overlay.has_pruned_proposal(id) {
            Existence::Gone
        } else {
            self.draining.resolve_proposal(id)
        }
    }
}

impl VolatileSequence for VolatileDB {
    type Item = AnchoredVolatileFragment;

    fn is_empty(&self) -> bool {
        self.current.is_empty() && self.draining.is_empty()
    }

    fn len(&self) -> usize {
        self.current.len() + self.draining.len()
    }

    fn view_back(&self) -> Option<&Self::Item> {
        self.current.view_back().or(self.draining.view_back())
    }

    fn view_front(&self) -> Option<&Self::Item> {
        self.draining.view_front().or(self.current.view_front())
    }

    /// Check whether any fragment is anchored at the given point.
    fn has_point(&self, point: &Point) -> bool {
        self.current.has_point(point) || self.draining.has_point(point)
    }

    fn iter(&self) -> impl Iterator<Item = &Self::Item> {
        self.draining.iter().chain(self.current.iter())
    }

    fn into_iter(self) -> impl Iterator<Item = Self::Item> {
        self.draining.into_iter().chain(self.current.into_iter())
    }

    fn pop_front(&mut self) -> Option<Self::Item> {
        self.draining.pop_front().or_else(|| self.current.pop_front())
    }

    fn push_back(&mut self, item: Self::Item) {
        // FIXME:
        // Reset governance activity if a proposal is present.

        // By design, we should never be pushing to the back of the draining sequence
        self.current.push_back(item);
    }

    /// Rewind the volatile DB back to a given point, discarding everything that came after.
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

        let should_rollback = if self.draining.has_point(point) {
            // If we are rolling back to a point in the draining sequence, we need to
            // promote it as current while discarding the entire current series.
            self.current = std::mem::take(&mut self.draining);
            // We must also rollback the overlay since we are crossing the epoch boundary again.
            self.overlay.rollback();
            true
        } else {
            self.current.has_point(point)
        };

        if should_rollback {
            self.current.rollback_to(point)?;
            return Ok(());
        }

        Err(point)
    }

    fn clear(&mut self) {
        self.current.clear();
        if !self.draining.is_empty() {
            self.overlay.rollback();
        }
        self.draining.clear();
    }
}

impl VolatileDB {
    /// Construct an empty volatile DB whose overlay is anchored to the given epoch.
    pub fn new(epoch: Epoch, protocol_parameters: ProtocolParameters, governance_activity: GovernanceActivity) -> Self {
        Self {
            current: VolatileSeries::default(),
            draining: VolatileSeries::default(),
            overlay: StateOverlay::new(epoch),
            protocol_parameters,
            governance_activity,
        }
    }

    /// The epoch this volatile state is anchored to.
    pub fn epoch(&self) -> Epoch {
        self.overlay.epoch()
    }

    /// Get the most recent taken, by peaking at the files on disk or looking an in-memory cached
    /// value if available.
    pub fn most_recent_snapshot<HS: HistoricalStores>(&self, snapshots: &HS) -> Epoch {
        self.overlay.most_recent_snapshot(snapshots)
    }

    /// The protocol parameters carried by an in-flight epoch transition, if any.
    pub fn protocol_parameters(&self) -> &ProtocolParameters {
        self.overlay.pending_protocol_parameters().unwrap_or(&self.protocol_parameters)
    }

    /// Obtain the protocol parameters for a specific epoch; which can either be the *current* epoch
    /// as per the latest tip, or the previous one. This is useful when applying the last `k` blocks
    /// of an epoch.
    ///
    /// At this point, the tip has already transitioned, but we still need some of the protocol
    /// parameters *at the time of that block* during persistence; mostly because of branching logic
    /// that depends on protocol version.
    pub fn protocol_parameters_for(&self, epoch: Epoch) -> Option<&ProtocolParameters> {
        let current_epoch = self.epoch();
        if epoch == current_epoch {
            Some(self.protocol_parameters())
        } else if epoch + 1 == current_epoch {
            Some(&self.protocol_parameters)
        } else {
            None
        }
    }

    /// Obtain the latest governance activity, folding in any pending dormant-epoch bump from the
    /// volatile overlay.
    pub fn governance_activity(&self) -> GovernanceActivity {
        let mut governance_activity = self.governance_activity;

        if self.overlay.is_dormant_epoch() {
            governance_activity.consecutive_dormant_epochs += 1;
        }

        governance_activity
    }

    /// Similar to [`Self::protocol_parameters_for`], we need to hold onto the governance activity at
    /// the time of a block, and not the value at the tip (since we apply blocks with `k` blocks of
    /// delay).
    pub fn governance_activity_for(&self, epoch: Epoch) -> Option<GovernanceActivity> {
        let current_epoch = self.epoch();

        if epoch == current_epoch {
            Some(self.governance_activity())
        } else if epoch + 1 == current_epoch {
            Some(self.governance_activity)
        } else {
            None
        }
    }

    /// The governance roots, overlaying the pending boundary roots over the stable `base`.
    pub fn proposals_roots(&self) -> Option<&ProposalsRoots> {
        self.overlay.pending_proposals_roots()
    }

    /// Whether the rewards for the in-flight epoch are still to be computed.
    pub fn rewards_not_ready(&self) -> bool {
        matches!(self.overlay.rewards(), RewardsState::NotReady)
    }

    /// Take the rewards summary computed earlier in the epoch, marking the rewards as not-ready.
    pub fn take_computed_rewards(&mut self) -> Option<Rewards<Computed>> {
        self.overlay.take_computed_rewards()
    }

    /// Stash the freshly computed rewards summary, to be applied at the next epoch boundary.
    pub fn set_computed_rewards(&mut self, rewards: impl Into<Rewards<Computed>>) {
        *self.overlay.rewards_mut() = RewardsState::Computed(rewards.into());
    }

    /// Ensure that the 'draining' sequence is empty before we cross an epoch boundary. Note that
    /// this is a bandaid on the fact that the Haskell node (and thus Amaru) does not honour the
    /// Chain Growth property; so we can have situations where an epoch may contain less than `k`
    /// blocks overall which isn't enough time for the volatile db to fully drain the draining
    /// sequence. Yet, it must be empty and on-disk for the transition to happen.
    ///
    /// This means that momentarily, we are unable to rollback as far as we should. While this may
    /// seem like a big thing, it is *probably* less serious: if we end up in a situation where we
    /// have less than `k` blocks in an epoch, it probably means that there has been a problem with
    /// the chain and the network partitioned for some time, with one partition initially having
    /// less than the majority and eventually switched to it. That means, we have *already rolled
    /// back* over a long-range.
    pub fn epoch_tail(&mut self) -> Option<(usize, impl Iterator<Item = AnchoredVolatileFragment> + use<>)> {
        if !self.draining.is_empty() {
            Some((self.draining.len(), std::mem::take(&mut self.draining).into_iter()))
        } else {
            None
        }
    }

    pub fn transition(
        &mut self,
        effective_rewards: Option<Rewards<Effective>>,
        pools_updates: PoolsEpochTransitionUpdates,
        governance_updates: GovernanceUpdates,
    ) {
        // Mark the transition between two epochs by sealing the `current` series and turning it into
        // the `draining` series. This keeps each series epoch-homogeneous since, by the protocol
        // pre-condition, the `current` series holds only the closing epoch's blocks.
        //
        // No-op when `current` is empty: there is nothing to transition, `draining` stays `None`, and
        // homogeneity still holds because an empty `current` only ever takes new-epoch blocks.
        //
        // The `assert!` guards the design's load-bearing precondition: `epochLength` (~10k blocks) is
        // far larger than the volatile window `k` (2160 blocks at the time of writing), so at most one
        // epoch boundary is ever inside the window and any prior `draining` series has fully drained
        // long before the next boundary arrives. A violation would mean two boundaries inside the
        // window, impossible under the protocol. We `assert!` rather than `debug_assert!` because the
        // check is effectively free, and if some other bug ever broke the invariant, halting the node
        // is far safer than silently overwriting `draining` and losing volatile history.
        assert!(
            self.draining.is_empty(),
            "transitioning volatile series while a draining series is still present; two epoch boundaries inside the k-block window?"
        );
        self.draining = mem::take(&mut self.current);
        self.overlay.transition(effective_rewards, pools_updates, governance_updates);
    }

    /// Whether an epoch transition has been computed but not yet flushed to the stable store.
    pub fn is_epoch_transition_stable(&self, era_history: &EraHistory, global_parameters: &GlobalParameters) -> bool {
        !self.overlay.is_empty()
            && self.draining.view_front().is_none_or(|_| {
                let absolute_slot = self.current.view_back().map(|fragment| fragment.slot()).unwrap_or_default();
                let relative_slot = era_history.slot_in_epoch(absolute_slot, absolute_slot).unwrap_or_default();
                relative_slot >= global_parameters.stability_window()
            })
    }

    /// Flush the pending epoch transition to the stable store. Returns the freshly-enacted
    /// `(protocol_parameters, governance_activity)` when a governance transition was applied.
    pub fn apply_transition(&mut self, db: &impl Store) -> Result<(), StateError> {
        if let Some((protocol_parameters, governance_activity)) = self.overlay.apply(db)? {
            self.protocol_parameters = protocol_parameters;
            self.governance_activity = governance_activity;
        }

        Ok(())
    }
}

/// A type for capturing the latest rewards state of an account, in order to properly resolve its
/// rewards.
#[derive(Debug, PartialEq)]
pub enum RewardsAtTip {
    Reset,
    Replace(Lovelace),
    Add(Lovelace),
}

impl RewardsAtTip {
    /// Compute a current rewards balance from the latest known stable amount.
    pub fn into_balance(&self, base: Lovelace) -> Lovelace {
        match self {
            Self::Reset => 0,
            Self::Replace(credit) => *credit,
            Self::Add(credit) => base + credit,
        }
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
        epoch_transition::{Computed, Effective, GovernanceUpdates, PoolsEpochTransitionUpdates, Rewards},
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
        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));
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

        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));

        assert!(!db.draining.is_empty(), "draining should hold the transitioned series");
        assert_eq!(db.current.len(), 0, "current should be reset to empty");
        assert_eq!(db.len(), 2, "total length is unchanged by transitioning");
    }

    #[test]
    fn transition_is_a_noop_on_empty_current() {
        let mut db = VolatileDB::default();

        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));

        assert!(db.draining.is_empty(), "transitioning an empty current must not open a draining series");
    }

    #[test]
    #[should_panic(expected = "two epoch boundaries")]
    fn transition_panics_if_draining_already_present() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));

        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));
    }

    #[test]
    fn pop_front_drains_draining_then_nulls_it() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));
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
        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));
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
        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));
        db.push_back(current_block);

        assert_eq!(db.resolve_input(&input).is_some(), resolvable);
        assert_eq!(db.has_consumed_input(&input), consumed);
    }

    #[test]
    fn len_counts_both_series_until_draining_empties() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));
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
                    db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));
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
        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));
        if let Some(act) = current {
            db.push_back(account_block(20, act));
        }

        match db.resolve_account(&cred(1)) {
            (Existence::Exists(_), _) => Expect::Registered,
            (Existence::Gone, _) => Expect::Gone,
            (Existence::Unknown, _) => Expect::Unknown,
        }
    }

    #[test]
    fn reward_balance_is_zeroed_by_a_volatile_withdrawal() {
        // No withdrawal and no pending overlay credit: the base flows through untouched.
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        assert_eq!(db.resolve_account(&cred(1)).1, RewardsAtTip::Add(0));

        // A withdrawal in `current` (post-boundary) zeroes the balance.
        let mut db = VolatileDB::default();
        db.push_back(withdrawal_block(10));
        assert_eq!(db.resolve_account(&cred(1)).1, RewardsAtTip::Reset);

        // A withdrawal in `draining` (pre-boundary) with no pending credit also leaves nothing.
        let mut db = VolatileDB::default();
        db.push_back(withdrawal_block(10));
        db.transition(None, PoolsEpochTransitionUpdates::default(), committee_update(None));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        assert_eq!(db.resolve_account(&cred(1)).1, RewardsAtTip::Replace(0));
    }

    #[test]
    fn reward_balance_folds_in_the_pending_overlay_credit_during_the_straddle() {
        let mut db = VolatileDB::default();
        let computed = Rewards::<Computed>::new(0, 0, BTreeMap::from([(cred(1), 5_000_000)]));
        let effective = Rewards::<Effective>::new(computed, std::iter::once(cred(1)));

        // The pending boundary credit is added on top of the stable base.
        assert_eq!(db.resolve_account(&cred(1)).1, RewardsAtTip::Add(0));

        db.push_back(withdrawal_block(10));

        db.transition(Some(effective), PoolsEpochTransitionUpdates::default(), committee_update(None));

        // Withdrawn in `draining`, before the boundary credit: only the credit remains.
        assert_eq!(db.resolve_account(&cred(1)).1, RewardsAtTip::Replace(5_000_000));

        db.push_back(withdrawal_block(20));

        // Withdrawn again in `current`, after the boundary credit: nothing remains.
        assert_eq!(db.resolve_account(&cred(1)).1, RewardsAtTip::Reset);
    }

    #[test]
    fn reward_balance_folds_in_a_pending_governance_payout_during_the_straddle() {
        // A governance payout (proposal deposit refund or treasury withdrawal) destined for the
        // account at the boundary credits its withdrawable balance during the straddle, exactly
        // like a pool-deposit refund.
        let mut db = VolatileDB::default();
        let mut updates = committee_update(None);
        updates.payouts = BTreeMap::from([(cred(1), 3_000_000)]);
        db.transition(None, PoolsEpochTransitionUpdates::default(), updates);
        assert_eq!(db.resolve_account(&cred(1)).1, RewardsAtTip::Add(3_000_000));
    }

    #[test_case(None, Some(CommitteeAct::Auth) => Expect::Registered ; "hot-auth in current")]
    #[test_case(None, Some(CommitteeAct::Resign) => Expect::Gone ; "resigned in current")]
    #[test_case(Some(CommitteeAct::Auth), None => Expect::Registered ; "hot-auth in draining, untouched in current")]
    #[test_case(Some(CommitteeAct::Resign), None => Expect::Gone ; "resigned in draining shadows the stable store")]
    #[test_case(Some(CommitteeAct::Auth), Some(CommitteeAct::Resign) => Expect::Gone ; "current resignation overrides draining")]
    #[test_case(None, None => Expect::Unknown ; "untouched everywhere defers to the stable store")]
    fn resolve_committee_precedence(draining: Option<CommitteeAct>, current: Option<CommitteeAct>) -> Expect {
        let mut db = VolatileDB::default();
        let computed = Rewards::<Computed>::new(0, 0, BTreeMap::new());
        let effective = Rewards::<Effective>::new(computed, std::iter::empty());
        if let Some(act) = draining {
            db.push_back(committee_block(10, act));
        }
        db.transition(Some(effective), PoolsEpochTransitionUpdates::default(), committee_update(None));
        if let Some(act) = current {
            db.push_back(committee_block(20, act));
        }

        match db.resolve_cc_member(&cred(1)).0 {
            Existence::Exists(_) => Expect::Registered,
            Existence::Gone => Expect::Gone,
            Existence::Unknown => Expect::Unknown,
        }
    }

    #[test]
    fn resolve_committee_reflects_the_pending_boundary() {
        // Added at the boundary: a fresh member with the pending term, no stable row needed.
        let mut db = VolatileDB::default();
        let valid_until = Epoch::from(99);
        db.transition(
            None,
            PoolsEpochTransitionUpdates::default(),
            committee_update(Some(CommitteeUpdate::ChangeMembers {
                added: BTreeMap::from([(cred(1), valid_until)]),
                removed: BTreeSet::new(),
                threshold: SafeRatio::zero(),
            })),
        );
        let expected_term_limit = Some(valid_until);
        assert!(
            matches!(db.resolve_cc_member(&cred(1)), (Existence::Exists(_), Some(term_limit)) if term_limit == expected_term_limit)
        );

        // Removed at the boundary: a tombstone that shadows the stale stable entry.
        let mut db = VolatileDB::default();
        db.transition(
            None,
            PoolsEpochTransitionUpdates::default(),
            committee_update(Some(CommitteeUpdate::ChangeMembers {
                added: BTreeMap::new(),
                removed: BTreeSet::from([cred(1)]),
                threshold: SafeRatio::zero(),
            })),
        );
        assert!(matches!(db.resolve_cc_member(&cred(1)).0, Existence::Gone));

        // No-confidence keeps members, so membership defers down, but the term goes inactive.
        let mut db = VolatileDB::default();
        db.transition(
            None,
            PoolsEpochTransitionUpdates::default(),
            committee_update(Some(CommitteeUpdate::NoConfidence)),
        );
        assert!(matches!(db.resolve_cc_member(&cred(1)), (Existence::Unknown, Some(None))));
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
            CommitteeAct::Auth => block.fragment.committee.produce(cred(1), cred(2)),
            CommitteeAct::Resign => block.fragment.committee.consume(cred(1)),
        }
        block
    }

    fn committee_update(committee: Option<CommitteeUpdate>) -> GovernanceUpdates {
        GovernanceUpdates {
            roots: ProposalsRoots::default(),
            protocol_parameters: PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            pruned_proposals: BTreeMap::new(),
            payouts: BTreeMap::new(),
            is_dormant_epoch: false,
            constitutional_committee: committee,
            new_constitution: None,
        }
    }
}
