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

use std::{cell::RefCell, collections::BTreeMap, mem};

use amaru_kernel::{
    ComparableProposalId, Epoch, Lovelace, PoolId, ProtocolParameters, RatificationStatus, StakeCredential, TermLimit,
};
use amaru_observability::info_span;
use tracing::Span;

use crate::{
    debug,
    epoch_transition::{
        Computed, Effective, GovernanceActivity, GovernanceUpdates, PoolsEpochTransitionUpdates, Rewards, RewardsState,
    },
    governance::ratification::{CommitteeUpdate, ProposalsRoots},
    state::{
        StateError,
        diff_bind::{Bind, Empty, Resettable},
        volatile::{CommitteeMemberBind, Existence},
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
#[derive(Default)]
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
    pools_updates: Option<PoolsEpochTransitionUpdates>,

    /// The result of an epoch boundary ratification, stashed temporarily until it is stable enough
    /// to persist in the stable storage.
    governance_updates: Option<GovernanceUpdates>,
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
        let to = self.epoch - 1;
        debug!("epoch_transition.rollback", from = %self.epoch, %to);
        self.epoch = to;
        self.rewards = match mem::take(&mut self.rewards) {
            st @ RewardsState::NotReady | st @ RewardsState::Computed(..) => st,
            RewardsState::Effective(effective) => RewardsState::Computed(effective.into()),
        };
        self.pools_updates = None;
        self.governance_updates = None;
    }

    /// Record transition into a new epoch.
    pub fn transition(
        &mut self,
        effective_rewards: Option<Rewards<Effective>>,
        pools_updates: PoolsEpochTransitionUpdates,
        governance_updates: GovernanceUpdates,
    ) {
        let to = self.epoch + 1;
        debug!("epoch_transition.record", from = %self.epoch, %to);
        self.epoch = to;
        self.rewards = effective_rewards.map(RewardsState::Effective).unwrap_or(RewardsState::NotReady);
        self.pools_updates = Some(pools_updates);
        self.governance_updates = Some(governance_updates);
    }

    /// Flush an overlay to disk.
    ///
    /// Returns the freshly-enacted `(protocol_parameters, governance_activity)` when a governance
    /// transition was applied, so the caller can refresh its cached copy. Returns `None` when there
    /// was no governance update to apply, in which case the cached values are left untouched.
    pub fn apply(&mut self, db: &impl Store) -> Result<Option<(ProtocolParameters, GovernanceActivity)>, StateError> {
        let updated = info_span!(ledger::epoch_transition::APPLY, epoch = u64::from(self.epoch)).in_scope(|| {
            use EpochTransitionProgress::*;

            // ---------------------------------------------------------------------------- End of epoch
            db.with_transaction::<_, StateError>(|batch| {
                let should_end_epoch = batch.try_epoch_transition(None, Some(EpochEnded))?;

                Span::current().record("should_end_epoch", should_end_epoch);

                if should_end_epoch {
                    if let RewardsState::Effective(effective_rewards) = mem::take(&mut self.rewards) {
                        pay_rewards(batch, effective_rewards)?;
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
                    reset_blocks_count(batch)?;

                    reset_fees_and_donations(batch)?;

                    if let Some(mut pools_updates) = mem::take(&mut self.pools_updates) {
                        update_or_retire_pools(batch, pools_updates.take_updated(), pools_updates.take_retired())?;
                        pay_or_refund_accounts(batch, pools_updates.refunds())?;
                    } else {
                        debug!("overlay.no_pools_updates");
                    }

                    if let Some(governance_updates) = mem::take(&mut self.governance_updates) {
                        Some(apply_governance_updates(batch, governance_updates)?)
                    } else {
                        debug!("overlay.no_governance_updates");
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

    /// The committee membership verdict from the pending boundary transition. `ChangeMembers` adds
    /// (a fresh member, no stable row yet) and removes (a tombstone); `NoConfidence` keeps members,
    /// so it defers to the layers below. `Unknown` outside the straddle window.
    pub fn committee_verdict(&self, credential: &StakeCredential) -> Existence<CommitteeMemberBind> {
        match self.governance_updates.as_ref().and_then(|updates| updates.constitutional_committee.as_ref()) {
            Some(CommitteeUpdate::ChangeMembers { added, removed, .. }) => {
                if removed.contains(credential) {
                    Existence::Gone
                } else if added.contains_key(credential) {
                    // freshly elected; no hot key yet and no stable row to fall back to
                    Existence::Exists(Bind {
                        left: Resettable::Unchanged,
                        right: Resettable::Unchanged,
                        value: Some(Empty),
                    })
                } else {
                    Existence::Unknown
                }
            }
            Some(CommitteeUpdate::NoConfidence) | None => Existence::Unknown,
        }
    }

    /// A CC member's term at the pending boundary, if the transition sets one: `Some(term)` for a
    /// newly added member, `Some(None)` under no-confidence (members go inactive), `None` when the
    /// boundary leaves this member's term untouched.
    pub fn pending_committee_term(&self, credential: &StakeCredential) -> Option<TermLimit> {
        match self.governance_updates.as_ref().and_then(|updates| updates.constitutional_committee.as_ref())? {
            CommitteeUpdate::ChangeMembers { added, .. } => added.get(credential).map(|epoch| Some(*epoch)),
            CommitteeUpdate::NoConfidence => Some(None),
        }
    }

    /// Whether the proposal is pruned by the pending boundary transition (ratified, expired, or
    /// dropped). Like pool reaping, this short-circuits before the stale stable entry.
    pub fn has_pruned_proposal(&self, id: &ComparableProposalId) -> bool {
        self.governance_updates.as_ref().is_some_and(|updates| updates.pruned_proposals.contains_key(id))
    }

    /// The set of all pruned proposals from the epoch boundary (because they expired, were
    /// ratified, or evicted due to another ratification).
    pub fn pruned_proposals(&self) -> BTreeMap<&ComparableProposalId, RatificationStatus> {
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
            RewardsState::Effective(effective) => effective.accounts().get(credential).copied().unwrap_or(0),
            RewardsState::NotReady | RewardsState::Computed(..) => 0,
        };
        let refund = self.pools_updates.as_ref().map(|updates| updates.refund(credential)).unwrap_or(0);
        let governance_payout = self.governance_updates.as_ref().map(|updates| updates.payout(credential)).unwrap_or(0);
        reward + refund + governance_payout
    }

    /// A read-only handle on the rewards state.
    pub fn rewards(&self) -> &RewardsState {
        &self.rewards
    }

    /// A mut handle on the rewards state. Use with care to replace rewards.
    pub fn rewards_mut(&mut self) -> &mut RewardsState {
        &mut self.rewards
    }

    /// Consume a computed summary from a previous computation and mark the rewards as 'NotReady'.
    pub fn take_computed_rewards(&mut self) -> Option<Rewards<Computed>> {
        self.rewards.take_computed_rewards()
    }
}
