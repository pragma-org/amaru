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

use std::{
    collections::{BTreeMap, btree_map::Entry},
    mem,
};

use amaru_kernel::{
    Epoch, EraHistory, GlobalParameters, Hash, Lovelace, MemoizedTransactionOutput,
    PREPROD_DEFAULT_PROTOCOL_PARAMETERS, Point, PoolId, Pots, ProposalId, ProposalKind, ProposalsRoots,
    ProtocolParameters, StakeCredential, TransactionInput, size::SCRIPT,
};

use crate::{
    epoch_transition::{
        Computed, Effective, GovernanceActivity, GovernanceUpdates, PoolsEpochTransitionUpdates, Rewards, RewardsState,
    },
    state::{
        AnchoredVolatileFragment, StateError,
        volatile::{
            AccountBind, CommitteeMemberBind, DRepBind, Existence, RollbackGuard, VolatileDBRecovery, VolatileSequence,
            VolatileSeries, VolatileState, overlay::StateOverlay,
        },
    },
    store::{HistoricalStores, Store},
};

#[derive(Debug)]
#[cfg_attr(feature = "test-utils", derive(Clone))]
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

    /// Cached guardrails script of the enacted constitution; `None` when the constitution names
    /// none. Same lifecycle as `protocol_parameters`, and read through [`Self::guardrail_script`]
    /// so that a constitution enacted at a boundary is seen by the blocks validated before the
    /// overlay is flushed.
    guardrail_script: Option<Hash<SCRIPT>>,
}

impl Default for VolatileDB {
    fn default() -> Self {
        Self::new(Epoch::default(), PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(), GovernanceActivity::default(), None)
    }
}

impl VolatileState for VolatileDB {
    // --------------------------------------------------------------------------------------- UTxOs
    type TransactionOutput<'a> = Existence<&'a MemoizedTransactionOutput>;
    fn resolve_input<'a>(&'a self, input: &TransactionInput) -> Self::TransactionOutput<'a> {
        self.current.resolve_input(input).or_else(|| self.draining.resolve_input(input))
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
    type Account<'a> = (Existence<AccountBind<'a>>, RewardsAtTip);
    fn resolve_account<'a>(&'a self, credential: &StakeCredential) -> Self::Account<'a> {
        // Resolve a stake account across the volatile layers, precedence `current -> draining`. A `Gone`
        // from `current` short-circuits; a fresh re-registration supersedes the closing epoch, a
        // bind-only update layers over it.
        let account = self.current.resolve_account(credential).chain(|| self.draining.resolve_account(credential));

        let rewards_at_tip = if self.current.has_withdrawal(credential) {
            // rewards withdrawn after the boundary credit
            RewardsAtTip::Reset
        } else {
            let credit = self.overlay.pending_reward_credit(credential);
            if self.draining.has_withdrawal(credential) {
                // rewards withdrawn before the boundary credit
                RewardsAtTip::Replace(credit)
            } else {
                // rewards not withdrawn, full bonus
                RewardsAtTip::Add(credit)
            }
        };

        (account, rewards_at_tip)
    }

    fn has_withdrawal(&self, credential: &StakeCredential) -> bool {
        self.current.has_withdrawal(credential) || self.draining.has_withdrawal(credential)
    }

    // --------------------------------------------------------------------------------------- DReps
    type DRep<'a> = Existence<DRepBind<'a>>;
    fn resolve_drep<'a>(&'a self, credential: &StakeCredential) -> Self::DRep<'a> {
        // Resolve a DRep across the volatile layers, precedence `current -> draining`. A `Gone`
        // from `current` short-circuits; a fresh re-registration supersedes the closing epoch, an
        // anchor-only update layers over the registration it finds below.
        self.current.resolve_drep(credential).chain(|| self.draining.resolve_drep(credential))
    }

    // ----------------------------------------------------------------------------------- CCMembers
    type CCMembers<'a> = BTreeMap<&'a StakeCredential, Existence<CommitteeMemberBind<'a>>>;
    fn resolve_cc_members<'a>(&'a self) -> Self::CCMembers<'a> {
        let mut cc_members: BTreeMap<&'a StakeCredential, Vec<Existence<CommitteeMemberBind<'a>>>> = BTreeMap::new();

        let current = self.current.resolve_cc_members();
        let overlay = self.overlay.cc_members();
        let drainin = self.draining.resolve_cc_members();

        // Re-aggregate the binds per cold-credential
        for (cold_credential, cc_member) in current.chain(overlay).chain(drainin) {
            match cc_members.entry(cold_credential) {
                Entry::Vacant(entry) => {
                    entry.insert(vec![cc_member]);
                }
                Entry::Occupied(mut entry) => {
                    entry.get_mut().push(cc_member);
                }
            }
        }

        // Return the folded result for each member
        cc_members
            .into_iter()
            .map(|(cold_credential, binds)| (cold_credential, Existence::fold(binds.into_iter())))
            .collect()
    }

    // ----------------------------------------------------------------------------------- Proposals
    type Proposal = Existence<ProposalKind>;
    /// Resolve a governance proposal across the volatile layers, precedence `current -> overlay
    /// (pruning) -> draining`. A proposal pruned at the boundary is `Gone`; `Unknown` means consult
    /// the stable store.
    fn resolve_proposal(&self, id: &ProposalId) -> Self::Proposal {
        if let Existence::Exists(proposal) = self.current.resolve_proposal(id) {
            Existence::Exists(proposal)
        } else if self.overlay.has_pruned_proposal(id) {
            Existence::Gone
        } else {
            self.draining.resolve_proposal(id)
        }
    }

    // ---------------------------------------------------------------------------------------- Pots
    /// The treasury at the current tip. During the straddle window (a boundary transition computed
    /// but not yet flushed), the stable pot still holds the *closing* epoch's value, so we fold in
    /// the boundary delta stashed on the overlay. The delta was computed once at the boundary and
    /// matches exactly what apply will write, so the value does not jump when the
    /// overlay is eventually flushed. Outside the straddle window the overlay is empty and the
    /// stable value flows through untouched.
    fn resolve_treasury(&self, pots: &Pots) -> Lovelace {
        let treasury = self.overlay.treasury_delta() + pots.treasury;
        treasury.as_credit().unwrap_or_else(|| unreachable!("treasury is negative: {treasury} ?!!"))
    }

    fn resolve_donations(&self) -> Lovelace {
        self.current.resolve_donations() + self.draining.resolve_donations()
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

    fn iter(&self) -> impl DoubleEndedIterator<Item = &Self::Item> {
        self.draining.iter().chain(self.current.iter())
    }

    fn into_iter(self) -> impl DoubleEndedIterator<Item = Self::Item> {
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
}

impl VolatileDB {
    /// Construct an empty volatile DB whose overlay is anchored to the given epoch.
    pub fn new(
        epoch: Epoch,
        protocol_parameters: ProtocolParameters,
        governance_activity: GovernanceActivity,
        guardrail_script: Option<Hash<SCRIPT>>,
    ) -> Self {
        Self {
            current: VolatileSeries::default(),
            draining: VolatileSeries::default(),
            overlay: StateOverlay::new(epoch),
            protocol_parameters,
            governance_activity,
            guardrail_script,
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

    /// The guardrails script every proposal's policy must name, preferring the constitution
    /// enacted by an in-flight epoch transition over the cached base.
    pub fn guardrail_script(&self) -> Option<Hash<SCRIPT>> {
        self.overlay.pending_constitution().map_or(self.guardrail_script, |constitution| constitution.guardrail_script)
    }

    /// The governance roots, overlaying the pending boundary roots over the stable `base`.
    pub fn proposals_roots(&self) -> Option<&ProposalsRoots> {
        self.overlay.pending_proposals_roots()
    }

    /// Whether the rewards for the in-flight epoch are still to be computed.
    pub fn rewards_not_ready(&self) -> bool {
        matches!(self.overlay.rewards(), RewardsState::NotReady)
    }

    pub fn take_computed_rewards(&mut self) -> Option<Rewards<Computed>> {
        self.overlay.take_computed_rewards()
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
        donations: Lovelace,
        account_exists: impl Fn(&StakeCredential) -> bool,
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
        self.overlay.transition(effective_rewards, pools_updates, governance_updates, donations, account_exists);
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

    /// Flush the pending epoch transition to the stable store, refreshing the cached globals from
    /// whatever that transition enacted.
    pub fn apply_transition(&mut self, db: &impl Store) -> Result<(), StateError> {
        if let Some((protocol_parameters, governance_activity, guardrail_script)) = self.overlay.apply(db)? {
            self.protocol_parameters = protocol_parameters;
            self.governance_activity = governance_activity;
            self.guardrail_script = guardrail_script;
        }

        Ok(())
    }

    /// Empty the volatile window and return its previous contents, leaving behind an empty window.
    /// The current metadata (epoch, protocol parameters, governance activity) is kept rather than
    /// reset to defaults. The returned `VolatileDB` can be used to fully restore the volatile state
    /// through the whole-volatile recovery path if switching to the new fork fails.
    pub fn clear(&mut self) -> Self {
        let current = self.current.clear();

        let draining = self.draining.clear();
        let overlay = self.overlay.snapshot();
        if !draining.is_empty() {
            self.overlay.rollback();
        }

        Self {
            current,
            draining,
            overlay,
            protocol_parameters: self.protocol_parameters.clone(),
            governance_activity: self.governance_activity,
            guardrail_script: self.guardrail_script,
        }
    }

    /// Rewind the volatile DB back to a given point, discarding everything that came after.
    ///
    /// Returns a [`VolatileDBRecovery`] capturing what was discarded, so a failed fork switch can be
    /// undone via [`Self::undo_rollback`].
    pub fn rollback_to<'a>(&mut self, point: &'a Point) -> Result<RollbackGuard<'a>, String> {
        Ok(RollbackGuard {
            fork_point: point,
            recovery: if self.draining.has_point(point) {
                // If we are rolling back to a point in the draining sequence, we need to
                // promote it as current while discarding the entire current series.
                let old_current = mem::take(&mut self.current);
                self.current = mem::take(&mut self.draining);
                let drained = self.current.rollback_to(point)?;

                // We must also rollback the overlay since we are crossing the epoch boundary again.
                let overlay = self.overlay.snapshot();
                self.overlay.rollback();
                VolatileDBRecovery::RecoverAcrossEpoch { old_current, drained, overlay }
            } else {
                let discarded = self.current.rollback_to(point)?;
                let overlay = self.overlay.snapshot();
                VolatileDBRecovery::RecoverInEpoch { discarded, overlay }
            },
        })
    }

    /// Restore the volatile DB to its pre-rollback state, undoing both the rollback and any
    /// roll-forwards replayed since (a fork switch replays blocks before it may recover).
    pub fn undo_rollback(&mut self, RollbackGuard { fork_point, recovery }: RollbackGuard<'_>) {
        match recovery {
            VolatileDBRecovery::RecoverInEpoch { discarded, overlay } => {
                // While the rollback was in epoch, the attempt to switch fork could have pushed
                // block through an epoch transition. So the old 'current' may now be draining
                // and we must recover it back.
                if self.draining.has_point(fork_point) {
                    self.current = mem::take(&mut self.draining);
                }
                self.current.undo_rollback(fork_point, discarded);
                self.overlay = overlay;
            }
            VolatileDBRecovery::RecoverAcrossEpoch { old_current, drained, overlay } => {
                // Similarly, we could rollback across an epoch, but end up in one of two scenarios:
                //
                // 1. The new fork did not cross the epoch again, and the fork point is still in
                //    current.
                // 2. The new fork also crossed the epoch and has moved back the fork point to
                //    draining.
                if !self.draining.has_point(fork_point) {
                    self.draining = mem::replace(&mut self.current, old_current);
                } else {
                    self.current = old_current;
                }
                self.draining.undo_rollback(fork_point, drained);
                self.overlay = overlay;
            }
        }
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

    use amaru_kernel::{
        ConstitutionalCommitteeUpdate, Epoch, Hash, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, Point, SafeRatio, Slot,
        StakeCredential, any_modern_output, any_transaction_input, utils::tests::run_strategy,
    };
    use num::Zero;
    use test_case::test_case;

    use super::*;
    use crate::{
        epoch_transition::{Computed, Effective, GovernanceUpdates, PoolsEpochTransitionUpdates, Rewards},
        state::volatile::{Bind, Resettable},
    };

    /// Define a type with various `From` instance to ease the notations below and avoid repetition
    /// which requires annoying maintenance.
    struct EpochTransition {
        effective_rewards: Option<Rewards<Effective>>,
        pools_updates: PoolsEpochTransitionUpdates,
        governance_updates: GovernanceUpdates,
        donations: Lovelace,
    }

    impl EpochTransition {
        fn default() -> Self {
            Self {
                effective_rewards: Default::default(),
                pools_updates: Default::default(),
                governance_updates: GovernanceUpdates::default(PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone()),
                donations: Default::default(),
            }
        }

        fn transition(self, db: &mut VolatileDB) {
            db.transition(self.effective_rewards, self.pools_updates, self.governance_updates, self.donations, |_| true)
        }
    }

    impl From<()> for EpochTransition {
        fn from((): ()) -> Self {
            Self::default()
        }
    }

    impl From<Rewards<Effective>> for EpochTransition {
        fn from(rewards: Rewards<Effective>) -> Self {
            Self { effective_rewards: Some(rewards), ..Self::default() }
        }
    }

    impl From<PoolsEpochTransitionUpdates> for EpochTransition {
        fn from(pools_updates: PoolsEpochTransitionUpdates) -> Self {
            Self { pools_updates, ..Self::default() }
        }
    }

    impl From<GovernanceUpdates> for EpochTransition {
        fn from(governance_updates: GovernanceUpdates) -> Self {
            Self { governance_updates, ..Self::default() }
        }
    }

    impl From<Lovelace> for EpochTransition {
        fn from(donations: Lovelace) -> Self {
            Self { donations, ..Self::default() }
        }
    }

    impl VolatileDB {
        fn simple_transition(&mut self, epoch_transition: impl Into<EpochTransition>) {
            epoch_transition.into().transition(self)
        }
    }

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
    fn test_rollback_to_missing_slot_fails() {
        // Create a VolatileDB with three fragments at slots 10, 20, 30
        let mut db = VolatileDB::fixture();

        // Rollback to slot 25 (between 20 and 30)
        let rollback_point = Point::Specific(Slot::from(25), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point);

        assert!(result.is_err());
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn recover_across_epoch_restores_the_full_db_with_a_draining_sequence() {
        let mut db = VolatileDB::default();

        // Two blocks in the closing epoch
        let block1 = AnchoredVolatileFragment::fixture(10, 1);
        let block2 = AnchoredVolatileFragment::fixture(20, 2);
        let point1 = block1.point();
        db.push_back(block1);
        db.push_back(block2);

        // Then cross the epoch boundary and add two more in the opening epoch.
        db.simple_transition(());
        let block3 = AnchoredVolatileFragment::fixture(30, 3);
        let block4 = AnchoredVolatileFragment::fixture(40, 4);
        db.push_back(block3);
        db.push_back(block4);

        let slots_before = db.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>();
        let epoch_before = db.epoch();

        // Rolling back across the boundary and immediately recovering must restore the full window.
        let recovery = db.rollback_to(&point1).expect("rollback across the epoch boundary should succeed");
        db.undo_rollback(recovery);

        assert_eq!(
            db.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>(),
            slots_before,
            "recover must restore every block, including the retained draining prefix"
        );
        assert_eq!(db.len(), slots_before.len());
        assert_eq!(db.epoch(), epoch_before, "recover must restore the pre-rollback epoch anchor");
    }

    #[test]
    fn recover_restores_the_pre_rollback_state_after_roll_forwards() {
        // A cross-epoch window: draining [10, 20] in the closing epoch, current [30, 40] in the
        // opening epoch.
        let mut db = VolatileDB::default();
        let block1 = AnchoredVolatileFragment::fixture(10, 1);
        let block2 = AnchoredVolatileFragment::fixture(20, 2);
        let point1 = block1.point();
        db.push_back(block1);
        db.push_back(block2);
        // Give the boundary transition a non-trivial overlay (effective rewards crediting an
        // account) so we exercise recovery of the overlay, not just the block series.
        db.simple_transition(effective_reward(cred(1), 5_000_000));
        let block3 = AnchoredVolatileFragment::fixture(30, 3);
        let block4 = AnchoredVolatileFragment::fixture(40, 4);
        db.push_back(block3);
        db.push_back(block4);

        let current_before = db.current.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>();
        let draining_before = db.draining.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>();
        let epoch_before = db.epoch();
        // The overlay's pending boundary reward credit, observed through the account balance.
        let reward_credit_before = db.resolve_account(&cred(1)).1;
        assert_eq!(
            reward_credit_before,
            RewardsAtTip::Add(5_000_000),
            "sanity: the boundary credit is visible pre-rollback"
        );

        // Roll back across the boundary
        let recovery = db.rollback_to(&point1).expect("rollback across the epoch boundary should succeed");

        // Then roll a few blocks forward onto the rolled-back state, exactly as `switch_to_fork`
        // does before it may hit an invalid block. This is the case the recovery must survive: by
        // now `current` no longer holds the retained draining prefix.
        let block5 = AnchoredVolatileFragment::fixture(50, 5);
        let block6 = AnchoredVolatileFragment::fixture(60, 6);
        db.push_back(block5);
        db.push_back(block6);

        // Recovering must restore the exact pre-rollback state, discarding the roll-forwards.
        db.undo_rollback(recovery);

        assert_eq!(
            db.current.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>(),
            current_before,
            "recover must restore the pre-rollback `current`, not the rolled-forward one"
        );
        assert_eq!(
            db.draining.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>(),
            draining_before,
            "recover must restore the pre-rollback `draining`"
        );
        assert_eq!(db.epoch(), epoch_before, "recover must restore the pre-rollback epoch anchor");
        assert_eq!(
            db.resolve_account(&cred(1)).1,
            reward_credit_before,
            "recover must restore the overlay's pending reward credit (undoing the rollback's rewind)"
        );
    }

    #[test]
    fn recover_in_epoch_after_roll_forwards() {
        // A single-epoch window: current [10, 20, 30], no draining.
        let mut db = VolatileDB::default();
        let block1 = AnchoredVolatileFragment::fixture(10, 1);
        db.push_back(block1);
        let block2 = AnchoredVolatileFragment::fixture(20, 2);
        let point2 = block2.point();
        db.push_back(block2);
        let block3 = AnchoredVolatileFragment::fixture(30, 3);
        db.push_back(block3);

        let current_before = db.current.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>();
        let epoch_before = db.epoch();

        // Roll back within the epoch, then replay a couple of fork blocks before recovering.
        let recovery = db.rollback_to(&point2).expect("in-epoch rollback should succeed");
        let block4 = AnchoredVolatileFragment::fixture(25, 4);
        let block5 = AnchoredVolatileFragment::fixture(28, 5);
        db.push_back(block4);
        db.push_back(block5);

        db.undo_rollback(recovery);

        assert_eq!(
            db.current.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>(),
            current_before,
            "recover must restore the pre-rollback `current`, discarding the roll-forwards"
        );
        assert!(db.draining.is_empty(), "draining stays empty across an in-epoch rollback and recovery");
        assert_eq!(db.epoch(), epoch_before, "recover must restore the pre-rollback epoch anchor");
    }

    #[test]
    fn recover_in_epoch_survives_a_fork_that_crosses_the_epoch_boundary() {
        // A single-epoch window right before an epoch boundary: current [10, 20, 30], no
        // draining.
        //
        // The rollback is therefore in-epoch, yet the fork replayed on top of it will cross
        // the boundary.
        let mut db = VolatileDB::default();

        let block1 = AnchoredVolatileFragment::fixture(10, 1);
        let block2 = AnchoredVolatileFragment::fixture(20, 2);
        let block3 = AnchoredVolatileFragment::fixture(30, 3);

        let point2 = block2.point();

        db.push_back(block1);
        db.push_back(block2);
        db.push_back(block3);

        let current_before = db.current.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>();
        let epoch_before = db.epoch();
        let reward_credit_before = db.resolve_account(&cred(1)).1;

        // Roll back within the epoch. `draining` is empty, so this is a `RecoverInEpoch`.
        let recovery = db.rollback_to(&point2).expect("in-epoch rollback should succeed");

        // Replay a fork that crosses the boundary: one more closing-epoch block, then a transition
        // installing a boundary reward credit, then an opening-epoch block.
        let block4 = AnchoredVolatileFragment::fixture(25, 4);
        db.push_back(block4);

        db.simple_transition(effective_reward(cred(1), 7_000_000));

        let block5 = AnchoredVolatileFragment::fixture(35, 5);
        db.push_back(block5);
        assert_eq!(db.epoch(), epoch_before + 1, "replay crossed into the next epoch");
        assert!(!db.draining.is_empty(), "the transition moved the fork point into `draining`");
        assert_eq!(
            db.resolve_account(&cred(1)).1,
            RewardsAtTip::Add(7_000_000),
            "the fork installed a boundary reward credit before recovery"
        );

        // Recovering must undo the replay entirely, including the transition it triggered.
        db.undo_rollback(recovery);

        assert_eq!(
            db.current.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>(),
            current_before,
            "recover must restore the pre-rollback `current`, discarding the boundary-crossing replay"
        );
        assert!(db.draining.is_empty(), "recover must clear the `draining` created by the replay");
        assert_eq!(db.epoch(), epoch_before, "recover must restore the pre-rollback epoch anchor");
        assert_eq!(
            db.resolve_account(&cred(1)).1,
            reward_credit_before,
            "recover must undo the overlay transition installed by the replay"
        );
    }

    // #[test]
    // fn recover_across_epoch_survives_a_fork_remains_in_same_epoch() {
    // }

    #[test]
    fn recover_across_epoch_survives_a_fork_that_crosses_the_epoch_boundary_again() {
        // Pre-rollback: draining [10, 20] (closing epoch), current [30, 40] (opening epoch).
        let mut db = VolatileDB::default();

        let block1 = AnchoredVolatileFragment::fixture(10, 1);
        let block2 = AnchoredVolatileFragment::fixture(20, 2);
        let block3 = AnchoredVolatileFragment::fixture(30, 3);
        let block4 = AnchoredVolatileFragment::fixture(40, 4);

        let point1 = block1.point();

        db.push_back(block1);
        db.push_back(block2);

        db.simple_transition(effective_reward(cred(1), 5_000_000));

        db.push_back(block3);
        db.push_back(block4);

        let current_before = db.current.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>();
        let draining_before = db.draining.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>();
        let epoch_before = db.epoch();
        let reward_credit_before = db.resolve_account(&cred(1)).1;
        assert_eq!(
            reward_credit_before,
            RewardsAtTip::Add(5_000_000),
            "sanity: the original boundary credit is visible"
        );

        // Roll back across the boundary to the first closing-epoch block.
        let recovery = db.rollback_to(&point1).expect("cross-epoch rollback should succeed");

        // Replay a fork that itself re-crosses the epoch boundary before failing, installing a
        // *different* overlay (a different boundary credit for cred(1)).
        db.push_back(AnchoredVolatileFragment::fixture(15, 5));

        db.simple_transition(effective_reward(cred(1), 9_000_000));

        db.push_back(AnchoredVolatileFragment::fixture(35, 6));
        assert_eq!(
            db.resolve_account(&cred(1)).1,
            RewardsAtTip::Add(9_000_000),
            "sanity: the fork installed a different overlay before recovery"
        );

        db.undo_rollback(recovery);

        assert_eq!(
            db.current.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>(),
            current_before,
            "recover must restore the pre-rollback `current` even after the replay transitioned"
        );
        assert_eq!(
            db.draining.iter().map(|fragment| fragment.slot()).collect::<Vec<_>>(),
            draining_before,
            "recover must restore the pre-rollback `draining`"
        );
        assert_eq!(db.epoch(), epoch_before, "recover must restore the pre-rollback epoch anchor");
        assert_eq!(
            db.resolve_account(&cred(1)).1,
            reward_credit_before,
            "recover must restore the pre-rollback overlay (the original boundary credit), not the fork's"
        );
    }

    #[test]
    fn clear_empties_the_window_but_keeps_the_epoch_anchor() {
        let epoch = Epoch::from(42);
        let mut db =
            VolatileDB::new(epoch, PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(), GovernanceActivity::default(), None);
        let block1 = AnchoredVolatileFragment::fixture(10, 1);
        let block2 = AnchoredVolatileFragment::fixture(20, 2);
        db.push_back(block1);
        db.push_back(block2);

        let snapshot = db.clear();

        // The snapshot holds the full previous window
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot.epoch(), epoch);

        // While the current window is empty but still anchored to the same epoch, rather than
        // being reset to the default epoch.
        assert!(db.is_empty());
        assert_eq!(db.epoch(), epoch);
    }

    #[test]
    fn clear_rewinds_the_retained_window_across_an_epoch_boundary() {
        let epoch = Epoch::from(42);
        let mut db =
            VolatileDB::new(epoch, PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(), GovernanceActivity::default(), None);
        let block1 = AnchoredVolatileFragment::fixture(10, 1);
        let block2 = AnchoredVolatileFragment::fixture(20, 2);
        db.push_back(block1);
        db.simple_transition(());
        db.push_back(block2);
        assert_eq!(db.epoch(), epoch + 1, "transition opened the next epoch");

        let snapshot = db.clear();

        // The snapshot keeps the full window and its post-transition anchor
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot.epoch(), epoch + 1);

        // While the retained window is empty and rewound across the boundary
        assert!(db.is_empty());
        assert_eq!(db.epoch(), epoch);
    }

    #[test]
    fn test_consumed_input_is_tracked() {
        let input = run_strategy(any_transaction_input());
        let mut anchored = AnchoredVolatileFragment::fixture(10, 1);
        anchored.fragment.utxo.consume(input);

        let mut db = VolatileDB::default();

        db.push_back(anchored);
        assert_eq!(db.resolve_input(&input), Existence::Gone);
    }

    #[test]
    fn test_rollback_removes_consumed_input_from_cache() {
        let input = run_strategy(any_transaction_input());
        let mut db = VolatileDB::default();
        let first = AnchoredVolatileFragment::fixture(10, 1);
        let first_point = first.point();
        db.push_back(first);

        let mut second = AnchoredVolatileFragment::fixture(20, 2);
        second.fragment.utxo.consume(input);

        db.push_back(second);
        assert_eq!(db.resolve_input(&input), Existence::Gone);

        db.rollback_to(&first_point).unwrap();
        assert_eq!(db.resolve_input(&input), Existence::Unknown);
    }

    #[test]
    fn transition_opens_draining_and_resets_current() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));

        db.simple_transition(());

        assert!(!db.draining.is_empty(), "draining should hold the transitioned series");
        assert_eq!(db.current.len(), 0, "current should be reset to empty");
        assert_eq!(db.len(), 2, "total length is unchanged by transitioning");
    }

    #[test]
    fn transition_is_a_noop_on_empty_current() {
        let mut db = VolatileDB::default();

        db.simple_transition(());

        assert!(db.draining.is_empty(), "transitioning an empty current must not open a draining series");
    }

    #[test]
    #[should_panic(expected = "two epoch boundaries")]
    fn transition_panics_if_draining_already_present() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.simple_transition(());

        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.simple_transition(());
    }

    #[test]
    fn pop_front_drains_draining_then_nulls_it() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.simple_transition(());
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
        db.simple_transition(());
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
        let input = run_strategy(any_transaction_input());
        let mut draining_block = AnchoredVolatileFragment::fixture(10, 1);
        let mut current_block = AnchoredVolatileFragment::fixture(20, 2);

        if let Some(layer) = produce_in {
            let block = match layer {
                Where::Draining => &mut draining_block,
                Where::Current => &mut current_block,
            };
            block.fragment.utxo.produce(input, Arc::new(run_strategy(any_modern_output())));
        }

        if let Some(layer) = consume_in {
            let block = match layer {
                Where::Draining => &mut draining_block,
                Where::Current => &mut current_block,
            };
            block.fragment.utxo.consume(input);
        }

        let mut db = VolatileDB::default();
        db.push_back(draining_block);
        db.simple_transition(());
        db.push_back(current_block);

        if resolvable {
            assert!(matches!(dbg!(db.resolve_input(&input)), Existence::Exists(..)))
        } else if consumed {
            assert_eq!(db.resolve_input(&input), Existence::Gone)
        } else {
            assert_eq!(db.resolve_input(&input), Existence::Unknown)
        }
    }

    #[test]
    fn len_counts_both_series_until_draining_empties() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        db.simple_transition(());
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
        db.simple_transition(());
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
        db.simple_transition(());
        db.push_back(AnchoredVolatileFragment::fixture(20, 2));
        assert_eq!(db.resolve_account(&cred(1)).1, RewardsAtTip::Replace(0));
    }

    #[test]
    fn reward_balance_folds_in_the_pending_overlay_credit_during_the_straddle() {
        let mut db = VolatileDB::default();
        let accounts = BTreeMap::from([(cred(1), 5_000_000)]);
        let computed = Rewards::<Computed>::new(0, 0, accounts.values().sum(), accounts, Default::default());
        let effective = Rewards::<Effective>::new(computed, BTreeSet::new());

        // The pending boundary credit is added on top of the stable base.
        assert_eq!(db.resolve_account(&cred(1)).1, RewardsAtTip::Add(0));

        db.push_back(withdrawal_block(10));

        db.simple_transition(effective);

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

        let mut governance_updates = committee_update(None);
        governance_updates.deposit_refunds = BTreeMap::from([(cred(1), 3_000_000)]);

        db.simple_transition(governance_updates);

        assert_eq!(db.resolve_account(&cred(1)).1, RewardsAtTip::Add(3_000_000));
    }

    #[test]
    fn resolve_treasury_passes_through_the_stable_pot_without_a_pending_boundary() {
        let pots = Pots::default().with_treasury(100_000_000);
        assert_eq!(VolatileDB::default().resolve_treasury(&pots), pots.treasury);
    }

    #[test]
    fn resolve_treasury_folds_in_the_boundary_credit_during_the_straddle() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.simple_transition(42_000);

        assert_eq!(db.resolve_treasury(&Pots::default().with_treasury(100_000_000)), 100_042_000,);
    }

    #[test]
    fn resolve_treasury_folds_in_the_boundary_debit_during_the_straddle() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.simple_transition(GovernanceUpdates {
            treasury_withdrawals: BTreeMap::from([(cred(1), 1_000_000)]),
            ..GovernanceUpdates::default(PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone())
        });

        assert_eq!(db.resolve_treasury(&Pots::default().with_treasury(100_000_000)), 99_000_000);
    }

    #[test]
    fn resolve_treasury_does_not_account_for_pending_donations() {
        let mut db = VolatileDB::default();
        db.push_back(donation_block(10, 42_000));
        assert_eq!(db.resolve_treasury(&Pots::default()), Pots::default().treasury);
    }

    #[test]
    fn resolve_donations_sums_both_series_across_a_boundary() {
        let mut db = VolatileDB::default();
        db.push_back(donation_block(10, 1_000_000));
        db.simple_transition(());
        db.push_back(donation_block(20, 300_000));

        assert_eq!(db.resolve_donations(), 1_300_000, "draining and current both count");
    }

    #[test]
    fn resolve_donations_retracts_what_stabilization_and_rollback_discard() {
        let mut db = VolatileDB::default();
        db.push_back(donation_block(10, 1_000_000));
        db.push_back(donation_block(20, 300_000));
        db.push_back(donation_block(30, 70_000));

        db.pop_front();
        assert_eq!(db.resolve_donations(), 370_000, "a stabilized block's donation reached the stable pot");

        db.rollback_to(&Point::Specific(Slot::from(20), Hash::new([0u8; 32]))).unwrap();
        assert_eq!(db.resolve_donations(), 300_000, "the rolled-back block's donation is discarded");
    }

    #[test]
    fn resolve_treasury_drops_the_delta_on_boundary_rollback() {
        let mut db = VolatileDB::default();
        db.push_back(AnchoredVolatileFragment::fixture(10, 1));
        db.simple_transition(7_000_000);
        assert_eq!(db.resolve_treasury(&Pots::default()), Pots::default().treasury + 7_000_000);
        db.rollback_to(&Point::Specific(Slot::from(10), Hash::new([0u8; 32]))).unwrap();
        assert_eq!(db.resolve_treasury(&Pots::default()), Pots::default().treasury);
    }

    #[test_case(None, Some(CommitteeAct::Auth) => Existence::Exists(true); "hot-auth in current")]
    #[test_case(None, Some(CommitteeAct::Resign) => Existence::Exists(false); "resigned in current")]
    #[test_case(Some(CommitteeAct::Auth), None => Existence::Exists(true); "hot-auth in draining, untouched in current")]
    #[test_case(Some(CommitteeAct::Resign), None => Existence::Exists(false); "resigned in draining shadows the stable store")]
    #[test_case(Some(CommitteeAct::Auth), Some(CommitteeAct::Resign) => Existence::Exists(false); "current resignation overrides draining")]
    #[test_case(None, None => Existence::Unknown; "untouched everywhere defers to the stable store")]
    fn resolve_committee_precedence(draining: Option<CommitteeAct>, current: Option<CommitteeAct>) -> Existence<bool> {
        let mut db = VolatileDB::default();
        let computed = Rewards::<Computed>::new(0, 0, 0, BTreeMap::new(), Default::default());
        let effective = Rewards::<Effective>::new(computed, BTreeSet::new());
        if let Some(act) = draining {
            db.push_back(committee_block(10, act));
        }
        db.simple_transition(effective);

        if let Some(act) = current {
            db.push_back(committee_block(20, act));
        }

        match db.resolve_cc_members().get(&cred(1)) {
            Some(Existence::Exists(Bind { left, .. })) => Existence::Exists(matches!(left, Resettable::Set { .. })),
            Some(Existence::Gone) => Existence::Gone,
            Some(Existence::Unknown) | None => Existence::Unknown,
        }
    }

    #[test]
    fn resolve_committee_reflects_the_pending_boundary() {
        // Added at the boundary: a fresh member with the pending term, no stable row needed.
        let mut db = VolatileDB::default();
        let expected_term_limit = Epoch::from(99);

        db.simple_transition(committee_update(Some(ConstitutionalCommitteeUpdate::ChangeMembers {
            added: BTreeMap::from([(cred(1), expected_term_limit)]),
            removed: BTreeSet::new(),
            threshold: SafeRatio::zero(),
        })));

        assert!(matches!(
            db.resolve_cc_members().get(&cred(1)),
            Some(Existence::Exists(Bind { right: Resettable::Set(term_limit), .. })) if **term_limit == expected_term_limit
        ));

        // Removed at the boundary: a tombstone that shadows the stale stable entry.
        let mut db = VolatileDB::default();
        db.simple_transition(committee_update(Some(ConstitutionalCommitteeUpdate::ChangeMembers {
            added: BTreeMap::new(),
            removed: BTreeSet::from([cred(1)]),
            threshold: SafeRatio::zero(),
        })));
        assert!(matches!(db.resolve_cc_members().get(&cred(1)), Some(Existence::Gone)));

        // No-confidence keeps members, so membership defers down, but the term goes inactive.
        let mut db = VolatileDB::default();
        db.simple_transition(committee_update(Some(ConstitutionalCommitteeUpdate::NoConfidence)));
        assert!(!db.resolve_cc_members().contains_key(&cred(1)));
    }

    /// A credential named in a pending `UpdateCommittee` may authorize a hot key before enactment.
    #[test]
    fn resolve_committee_keeps_a_hot_key_declared_before_the_electing_boundary() {
        let mut db = VolatileDB::default();
        let expected_term_limit = Epoch::from(99);

        db.push_back(committee_block(10, CommitteeAct::Auth));
        db.simple_transition(committee_update(Some(ConstitutionalCommitteeUpdate::ChangeMembers {
            added: BTreeMap::from([(cred(1), expected_term_limit)]),
            removed: BTreeSet::new(),
            threshold: SafeRatio::zero(),
        })));

        let cc_members = db.resolve_cc_members();

        let Some(Existence::Exists(Bind { left, right, .. })) = cc_members.get(&cred(1)) else {
            panic!("elected member resolved to nothing");
        };

        assert_eq!(left, &Resettable::Set(&cred(2)), "the hot key declared while the proposal was pending");
        assert_eq!(right, &Resettable::Set(&expected_term_limit), "the term the boundary granted");
    }

    #[test]
    fn resolve_committee_lets_a_resignation_clear_a_hot_key_across_the_boundary() {
        let mut db = VolatileDB::default();

        db.push_back(committee_block(10, CommitteeAct::Auth));
        db.simple_transition(committee_update(Some(ConstitutionalCommitteeUpdate::ChangeMembers {
            added: BTreeMap::from([(cred(1), Epoch::from(99))]),
            removed: BTreeSet::new(),
            threshold: SafeRatio::zero(),
        })));
        db.push_back(committee_block(20, CommitteeAct::Resign));

        let cc_members = db.resolve_cc_members();

        let Some(Existence::Exists(Bind { left, right, .. })) = cc_members.get(&cred(1)) else {
            panic!("resigned member resolved to nothing; the seat outlives the authorization");
        };

        assert_eq!(left, &Resettable::Reset);
        assert_eq!(right, &Resettable::Set(&Epoch::from(99)));
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

    /// Effective boundary rewards crediting a single account, to give the overlay non-trivial,
    /// observable state (its pending reward credit surfaces through `resolve_account`).
    fn effective_reward(credential: StakeCredential, amount: u64) -> Rewards<Effective> {
        let computed =
            Rewards::<Computed>::new(0, 0, amount, BTreeMap::from([(credential, amount)]), Default::default());
        Rewards::<Effective>::new(computed, BTreeSet::new())
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
            CommitteeAct::Auth => block.fragment.committee.bind_left(cred(1), Some(cred(2))),
            CommitteeAct::Resign => block.fragment.committee.bind_left(cred(1), None),
        }
        .unwrap();
        block
    }

    fn donation_block(slot: u64, donation: Lovelace) -> AnchoredVolatileFragment {
        let mut block = AnchoredVolatileFragment::fixture(slot, slot as u8);
        block.fragment.donations = donation;
        block
    }

    fn committee_update(committee: Option<ConstitutionalCommitteeUpdate>) -> GovernanceUpdates {
        GovernanceUpdates {
            constitutional_committee: committee,
            ..GovernanceUpdates::default(PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone())
        }
    }
}
