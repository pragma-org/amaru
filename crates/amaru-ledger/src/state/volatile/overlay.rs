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

use std::{cell::RefCell, collections::BTreeMap, mem, sync::Arc};

use amaru_kernel::{
    Constitution, ConstitutionalCommitteeUpdate, Epoch, Hash, Lovelace, PoolId, ProposalId, ProposalsRoots,
    ProtocolParameters, RatificationStatus, StakeCredential, TreasuryDelta, size::SCRIPT,
};
use amaru_observability::{debug, info_span};
use tracing::Span;

use crate::{
    epoch_transition::{
        Computed, Effective, GovernanceActivity, GovernanceUpdates, PoolsEpochTransitionUpdates, Rewards, RewardsState,
    },
    state::{
        StateError,
        volatile::{Bind, CommitteeMemberBind, Existence, Resettable},
    },
    store::{
        EpochTransitionProgress, HistoricalStores, Store, TransactionalContext, apply_governance_updates,
        pay_or_refund_accounts, pay_rewards, reset_blocks_count, reset_fees_and_donations,
        reset_recently_pruned_proposals, update_or_retire_pools,
    },
};

/// Represents the volatile, rollback-able bits of the epoch transition that aren't stable yet but
/// that still need to be accounted for for block validation. They are computed at each epoch
/// boundary, and flushed once we've reached the stability window of each epoch.
///
/// This lives inside the [`crate::state::volatile::VolatileDB`]: it is part of the volatile state
/// (it can be rolled back), and co-locating it with the two volatile series keeps reads and
/// rollback cohesive.
#[derive(Default, Debug)]
#[cfg_attr(feature = "test-utils", derive(Clone))]
pub struct StateOverlay {
    /// The last known epoch; or said differently, the epoch for which this overlay is valid.
    epoch: Epoch,

    /// The most recent snapshot taken, kept in memory to avoid repeated I/O.
    most_recent_snapshot: RefCell<Option<Epoch>>,

    /// The computed rewards summary to be applied on the next epoch boundary. This is computed
    /// once in the epoch, and held until the end where it is reset.
    ///
    /// It also contains the latest stake distribution computed from the previous epoch, which we
    /// hold onto the epoch boundary. In the epoch boundary, the stake distribution becomes
    /// available for the leader schedule verification, whereas the stake distribution previously
    /// used for leader schedule is moved as rewards stake.
    rewards: RewardsState,

    /// Computed pools updates that are pending application to the stable store. The value is only
    /// `Some` during the first `k` blocks of an epoch since this corresponds to the unstable part
    /// of an epoch.
    ///
    /// When present, they must be taken into account when creating the ledger validation context.
    ///
    /// Held behind an `Arc` for the same reason as `rewards`: capturing the overlay for rollback
    /// recovery is then a reference-count bump rather than a deep copy of the update maps. It is
    /// only ever replaced wholesale (never mutated in place), so sharing the value with an
    /// outstanding recovery is safe.
    pools_updates: Option<Arc<PoolsEpochTransitionUpdates>>,

    /// The result of an epoch boundary ratification, stashed temporarily until it is stable enough
    /// to persist in the stable storage. Held behind an `Arc` for the same reason as `pools_updates`.
    governance_updates: Option<Arc<GovernanceUpdates>>,

    /// The net change applied to the stable treasury when this overlay is flushed, computed once at
    /// the boundary. Held so the validation context can resolve the *new* epoch's treasury during
    /// the straddle window (before the boundary is flushed to disk), without recomputing per-account
    /// work on the hot path. Negative when enacted treasury withdrawals outweigh the incoming
    /// rewards tax, donations and refund leftovers. 0 when no boundary is pending.
    treasury_delta: TreasuryDelta,
}

impl StateOverlay {
    /// Construct a new default/empty overlay for the given epoch.
    pub fn new(epoch: Epoch) -> Self {
        Self {
            epoch,
            most_recent_snapshot: RefCell::new(None),
            rewards: RewardsState::NotReady,
            pools_updates: None,
            governance_updates: None,
            treasury_delta: TreasuryDelta::Zero,
        }
    }

    /// Capture the overlay so a fork switch can restore it if the switch later fails.
    ///
    /// This is deliberately not a `Clone` implementation: the overlay is part of the volatile state
    /// and duplicating it is only ever meaningful when snapshotting for rollback recovery. Keeping
    /// it a named method prevents accidental clones elsewhere and documents the single legitimate
    /// use case at every call site.
    ///
    /// The capture is cheap: `epoch` is `Copy`, and `rewards`/`pools_updates`/`governance_updates`
    /// share their payload with the live overlay through `Arc`, so this is a handful of
    /// reference-count bumps rather than a deep copy.
    pub(crate) fn snapshot(&self) -> StateOverlay {
        StateOverlay {
            epoch: self.epoch,
            most_recent_snapshot: RefCell::new(*self.most_recent_snapshot.borrow()),
            rewards: self.rewards.clone(),
            pools_updates: self.pools_updates.clone(),
            governance_updates: self.governance_updates.clone(),
            treasury_delta: self.treasury_delta,
        }
    }

    /// Get the most recent taken, by peaking at the files on disk or looking an in-memory cached
    /// value if available.
    pub fn most_recent_snapshot<HS: HistoricalStores>(&self, snapshots: &HS) -> Epoch {
        if let Some(epoch) = *self.most_recent_snapshot.borrow() {
            epoch
        } else {
            let epoch = snapshots.most_recent_snapshot();
            self.most_recent_snapshot.replace(Some(epoch));
            epoch
        }
    }

    /// Rollback an existing overlay, throwing away the epoch transition calculations.
    pub fn rollback(&mut self) {
        self.rewards = mem::take(&mut self.rewards).rollback();
        self.pools_updates = None;
        self.governance_updates = None;
        self.treasury_delta = TreasuryDelta::Zero;

        let to = self.epoch - 1;
        debug!(ledger::epoch_transition::ROLLBACK, from = self.epoch, to);
        self.epoch = to;
    }

    /// Record transition into a new epoch.
    pub fn transition(
        &mut self,
        effective_rewards: Option<Rewards<Effective>>,
        pools_updates: PoolsEpochTransitionUpdates,
        governance_updates: GovernanceUpdates,
        donations: Lovelace,
        account_exists: impl Fn(&StakeCredential) -> bool,
    ) {
        let to = self.epoch + 1;
        debug!(ledger::epoch_transition::RECORD, from = self.epoch, to);

        self.epoch = to;

        // NOTE: treasury as of the *new* epoch
        //
        // The treasury only changes at the boundary, and the whole change is knowable here. It
        // mirrors exactly what `StateOverlay::apply` writes when the boundary is later flushed.
        // We stash it on the overlay so the validation context can resolve the new epoch's
        // treasury during the straddle window, instead of redoing this work on the hot path.
        //
        // Deposit refunds and treasury withdrawals whose target account is gone cannot be paid,
        // and are absorbed by the treasury instead.
        let unpayable = std::iter::empty()
            .chain(pools_updates.refunds())
            .chain(governance_updates.deposit_refunds.iter())
            .chain(governance_updates.treasury_withdrawals.iter())
            .fold(
                0,
                |unpayable, (credential, amount)| {
                    if account_exists(credential) { unpayable } else { unpayable + amount }
                },
            );

        let total_withdrawn = governance_updates.treasury_withdrawals.values().sum::<Lovelace>();

        self.treasury_delta = TreasuryDelta::Debit(total_withdrawn)
            + effective_rewards.as_ref().map(|rewards| rewards.delta_treasury()).unwrap_or(0)
            + donations
            + unpayable;

        self.rewards =
            effective_rewards.map(|r| RewardsState::Effective(Arc::new(r))).unwrap_or(RewardsState::NotReady);

        self.pools_updates = Some(Arc::new(pools_updates));

        self.governance_updates = Some(Arc::new(governance_updates));
    }

    /// Flush an overlay to disk.
    ///
    /// Returns the freshly-enacted `(protocol_parameters, governance_activity, guardrail_script)`
    /// when a governance transition was applied, so the caller can refresh its cached copies.
    /// Returns `None` when there was no governance update to apply, in which case the cached values
    /// are left untouched.
    pub fn apply(
        &mut self,
        db: &impl Store,
    ) -> Result<Option<(ProtocolParameters, GovernanceActivity, Option<Hash<SCRIPT>>)>, StateError> {
        let updated = info_span!(ledger::epoch_transition::APPLY, epoch = self.epoch).in_scope(|| {
            use EpochTransitionProgress::*;

            // NOTE: 3-step epoch transition
            //
            // Why are 3 db transactions needed here?
            //
            // Two transactions can be explained with a few points:
            //
            //  - Calculations that depend on historical data. A snapshot must be taken precisely
            //    after paying out the rewards, but before ratifying governance actions or pruning
            //    stake pools. So there's a precise split between these moment and it's nice
            //    (albeit strictly unnecessary) to make it apparent.
            //
            //  - Another compelling reason for the split is the bootstrapping of Amaru. Snapshots
            //    we produce from the Haskell node correspond precisely to our definition of
            //    snapshot here (i.e. data at the end of the epoch, after rewards payments but
            //    before next epoch start). Hence, we must be able to resume from a snapshot
            //    mid-transaction, and instead of making a special case for the bootstrapping, we
            //    make it our default behaviour.
            //
            // Why the third split then? Because while RocksDB allows for checkpointing in the
            // middle of a transaction, it does not rollback checkpoints on failures.
            //
            // So if anything goes wrong during the transaction but after the checkpoint/snapshot
            // was taken, we end up in an awkward spot where we need to re-apply the end of epoch,
            // skip the snapshot, and re-apply the beginning.
            //
            // Yet, this puts us in an inconsistent state where a snapshot exists (and other logic
            // may infer a bunch of decisions from it!), while the actual database state still lives
            // in the past. So to prevent this, we introduce a third split between the moment the
            // epoch ended (and was persisted to disk) and the moment the snapshot was taken.
            //
            // This way, if anything goes wrong:
            //
            // - before the epoch transition or during the epoch-ended transaction: the database
            //   remains unmodified, and no snapshot was generated.
            //
            // - after the epoch-ended or during the snapshot being taken: then we may or may not
            //   have a snapshot. If we do have a snapshot, we may still report a progress as
            //   "EpochEnded", which would only cause the snapshot to be taken _again_. Since this
            //   is an idempotent and rather quick operation, that's not a big problem. If we don't
            //   have a snapshot, then the progress is also still at "EpochEnded" and the snapshot
            //   will normally happen on restart.
            //
            // - during the epoch start: then we have a stable snapshot and the database is still in
            //   a correct state because the epoch progress still indicates "SnapshotTaken".

            // ---------------------------------------------------------------------------- End of epoch
            db.with_transaction::<_, StateError>(|batch| {
                let should_end_epoch = batch.try_epoch_transition(None, Some(EpochEnded))?;

                Span::current().record("should_end_epoch", should_end_epoch);

                if should_end_epoch {
                    if let RewardsState::Effective(effective_rewards) = mem::take(&mut self.rewards) {
                        pay_rewards(batch, &effective_rewards)?;
                        reset_recently_pruned_proposals(batch, self.pruned_proposals())?;
                    } else {
                        return Err(StateError::NoEffectiveRewards);
                    }
                } else {
                    mem::take(&mut self.rewards);
                }

                Ok(())
            })?;

            // ------------------------------------------------------------------------------ Snapshot
            db.with_transaction::<_, StateError>(|batch| {
                let should_snapshot = batch.try_epoch_transition(Some(EpochEnded), Some(SnapshotTaken))?;

                Span::current().record("should_snapshot", should_snapshot);

                if should_snapshot {
                    db.next_snapshot(self.epoch - 1)?;
                    self.most_recent_snapshot.replace(Some(self.epoch - 1));
                }

                Ok(())
            })?;

            // -------------------------------------------------------------------------- Start of epoch
            db.with_transaction::<_, StateError>(|batch| {
                let should_begin_epoch = batch.try_epoch_transition(Some(SnapshotTaken), None)?;

                Span::current().record("should_begin_epoch", should_begin_epoch);

                let updated = if should_begin_epoch {
                    batch.prune_recently_unregistered_accounts(self.epoch)?;
                    reset_blocks_count(batch)?;
                    reset_fees_and_donations(batch)?;

                    if let Some(pools_updates) = mem::take(&mut self.pools_updates) {
                        update_or_retire_pools(batch, pools_updates.updated(), pools_updates.retired())?;
                        pay_or_refund_accounts(batch, pools_updates.refunds())?;
                    } else {
                        debug!(ledger::overlay::NO_POOLS_UPDATES);
                    }

                    if let Some(governance_updates) = mem::take(&mut self.governance_updates) {
                        Some(apply_governance_updates(batch, &governance_updates)?)
                    } else {
                        debug!(ledger::overlay::NO_GOVERNANCE_UPDATES);
                        None
                    }
                } else {
                    mem::take(&mut self.pools_updates);
                    mem::take(&mut self.governance_updates);
                    None
                };

                Ok(updated)
            })
        })?;

        // The stashed delta has now been folded into the stable treasury by the store operations
        // above (rewards tax, donations, deposit-refund leftovers), so the straddle window is over.
        self.treasury_delta = TreasuryDelta::Zero;

        assert!(matches!(self.rewards, RewardsState::NotReady), "rewards leftovers after flushing overlay?");
        assert!(self.governance_updates.is_none(), "governance updates leftovers after flushing overlay?");
        assert!(self.pools_updates.is_none(), "pools updates leftovers after flushing overlay?");

        Ok(updated)
    }
}

impl StateOverlay {
    /// Check whether the overlay has unapplied state
    pub fn is_empty(&self) -> bool {
        matches!(&self.rewards, RewardsState::NotReady | RewardsState::Computed(..))
            && self.pools_updates.is_none()
            && self.governance_updates.is_none()
    }

    /// The last known epoch; or said differently, the epoch for which this overlay is valid.
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// The pending protocol parameters carried by an in-flight governance transition, if any.
    pub fn pending_protocol_parameters(&self) -> Option<&ProtocolParameters> {
        self.governance_updates.as_ref().map(|update| &update.protocol_parameters)
    }

    /// The constitution enacted by an in-flight epoch transition, if that transition enacts one.
    pub fn pending_constitution(&self) -> Option<&Constitution> {
        self.governance_updates.as_ref().and_then(|update| update.new_constitution.as_ref())
    }

    /// Whether the in-flight governance transition (if any) corresponds to a dormant epoch; used to
    /// bump the cached governance activity held by `State`.
    pub fn is_dormant_epoch(&self) -> bool {
        self.governance_updates.as_ref().is_some_and(|updates| updates.is_dormant_epoch)
    }

    /// Whether the given pool is reaped by the pending epoch-boundary transition. A reaped pool no
    /// longer exists for the *new* epoch, even though the stable store still holds its (now stale)
    /// entry until this overlay is flushed `k` blocks later. Pool-existence reads must therefore
    /// short-circuit on this *before* falling back to the stable store, or they'd resolve a reaped
    /// pool as still-existing.
    pub fn is_pool_retired(&self, pool_id: PoolId) -> bool {
        self.pools_updates.as_ref().is_some_and(|updates| updates.retired().contains(&pool_id))
    }

    /// The cold credentials this pending boundary transition can resolve to a member for, that is,
    /// the ones it elects. A removal short-circuits to `Gone`, so naming it here would only yield a
    /// candidate to discard.
    pub fn cc_members<'a>(&'a self) -> impl Iterator<Item = (&'a StakeCredential, Existence<CommitteeMemberBind<'a>>)> {
        match self.governance_updates.as_ref().and_then(|updates| updates.constitutional_committee.as_ref()) {
            Some(ConstitutionalCommitteeUpdate::ChangeMembers { added, removed, .. }) => Some(
                std::iter::empty()
                    .chain(added.iter().map(|(cold_credential, valid_until)| {
                        (
                            cold_credential,
                            // NOTE: newly elected member preserve hot credential delegations (resp. resignations)
                            //
                            // It is important for `left` to NOT be `Resettable::Reset` here, as it
                            // would invalidate a delegation (resp. resignation) registered ahead of
                            // time, as allowed by the ledger rules.
                            Existence::Exists(Bind { right: Resettable::Set(valid_until), ..Bind::default() }),
                        )
                    }))
                    .chain(removed.iter().map(|cold_credential| (cold_credential, Existence::Gone))),
            ),
            Some(ConstitutionalCommitteeUpdate::NoConfidence) | None => None,
        }
        .into_iter()
        .flatten()
    }

    /// Whether the proposal is pruned by the pending boundary transition (ratified, expired, or
    /// dropped). Like pool reaping, this short-circuits before the stale stable entry.
    pub fn has_pruned_proposal(&self, id: &ProposalId) -> bool {
        self.governance_updates.as_ref().is_some_and(|updates| updates.pruned_proposals.contains_key(id))
    }

    /// The set of all pruned proposals from the epoch boundary (because they expired, were
    /// ratified, or evicted due to another ratification).
    pub fn pruned_proposals(&self) -> BTreeMap<&ProposalId, RatificationStatus> {
        self.governance_updates
            .as_ref()
            .map(|updates| updates.pruned_proposals.iter().map(|(k, v)| (k, *v)).collect())
            .unwrap_or_default()
    }

    /// The pending governance roots from the boundary transition, if any.
    pub fn pending_proposals_roots(&self) -> Option<&ProposalsRoots> {
        self.governance_updates.as_ref().map(|updates| &updates.roots)
    }

    /// The account's pending reward credit at the not-yet-flushed epoch boundary: its effective
    /// reward, plus any pool-deposit refund, plus any governance payout (proposal deposit refund or
    /// treasury withdrawal). `0` outside the straddle window.
    pub fn pending_reward_credit(&self, credential: &StakeCredential) -> Lovelace {
        let reward = match &self.rewards {
            RewardsState::Effective(effective) => effective.reward_of(credential),
            RewardsState::NotReady | RewardsState::Computed(..) => 0,
        };
        let refund = self.pools_updates.as_ref().map(|updates| updates.refund(credential)).unwrap_or(0);
        let governance_payout = self.governance_updates.as_ref().map(|updates| updates.payout(credential)).unwrap_or(0);
        reward + refund + governance_payout
    }

    /// The net treasury change pending at the not-yet-flushed epoch boundary.
    pub fn treasury_delta(&self) -> &TreasuryDelta {
        &self.treasury_delta
    }

    /// A read-only handle on the rewards state.
    pub fn rewards(&self) -> &RewardsState {
        &self.rewards
    }

    pub fn take_computed_rewards(&mut self) -> Option<Rewards<Computed>> {
        self.rewards.take_computed_rewards()
    }
}

#[cfg(test)]
mod test {
    use std::collections::{BTreeMap, BTreeSet};

    use amaru_kernel::{Hash, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, TreasuryDelta};

    use super::*;
    use crate::{AccountState, epoch_transition::Computed};

    #[test]
    fn test_rollback() {
        let epoch = Epoch::from(300);
        let mut overlay = StateOverlay {
            epoch,
            most_recent_snapshot: RefCell::new(None),
            rewards: RewardsState::Effective(Arc::new(effective_rewards())),
            pools_updates: Some(Arc::new(PoolsEpochTransitionUpdates::default())),
            governance_updates: Some(Arc::new(GovernanceUpdates::default(PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone()))),
            treasury_delta: TreasuryDelta::Zero,
        };

        overlay.rollback();

        // Rolling back steps the epoch back, turns the effective rewards into computed ones, and
        // drops the pending pools and governance updates.
        assert_eq!(overlay.epoch, epoch - 1);
        assert!(matches!(overlay.rewards, RewardsState::Computed(_)), "rewards should be computed after rollback");
        assert!(overlay.pools_updates.is_none(), "pending pools updates should be dropped on rollback");
        assert!(overlay.governance_updates.is_none(), "pending governance updates should be dropped on rollback");
    }

    #[test]
    fn rollback_leaves_non_effective_rewards_untouched() {
        let epoch = Epoch::from(300);
        let mut overlay = StateOverlay::new(epoch);

        overlay.rollback();
        assert_eq!(overlay.epoch, epoch - 1);
        assert!(matches!(overlay.rewards, RewardsState::NotReady));
        assert!(overlay.is_empty());
    }

    // HELPERS

    fn credential(tag: u8) -> StakeCredential {
        StakeCredential::AddrKeyhash(Hash::new([tag; 28]))
    }

    /// Effective rewards where `credential(1)` is still registered while `credential(2)` unregistered during
    /// the epoch, so its rewards are unclaimed and returned to the treasury.
    fn effective_rewards() -> Rewards<Effective> {
        let computed = Rewards::<Computed>::new(
            1_000,
            7,
            142,
            BTreeMap::from([
                (credential(1), AccountState::default().with_rewards(100)),
                (credential(2), AccountState::default().with_rewards(42)),
            ])
            .into(),
            Default::default(),
        );
        Rewards::<Effective>::new(computed, BTreeSet::from([credential(2)]))
    }
}
